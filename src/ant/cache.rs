//! Versioned, atomic model-metadata cache for the `ant` enrichment feature.
//!
//! This module owns the on-disk format and IO for the model-metadata cache that
//! lives at `${XDG_CACHE_HOME:-~/.cache}/claudia-statusline/ant/models.json`.
//! It is the shared substrate that the render-side lookup (Plan 02) and the
//! out-of-band fetch/CLI (Plan 03) build on.
//!
//! # Critical contracts
//!
//! - **Render must not mutate the filesystem (D-16).** Path resolution
//!   ([`models_cache_path`]) is pure path math and NEVER creates a directory.
//!   Only [`ensure_cache_dir`] (the writer) creates the cache directory. The
//!   render-side reader ([`read_models_cache`]) uses [`models_cache_path`] only.
//! - **The read path is total.** [`read_models_cache`] collapses every error —
//!   missing, unreadable, corrupt, or schema-version mismatch — to `None`. It
//!   never panics, never spawns a process, never opens a socket, and never
//!   creates a directory.
//! - **No secrets on disk (D-17 / ANT-04).** Neither [`ModelsCache`] nor
//!   [`ModelEntry`] carries an API key or any credential field.
//! - **Atomic, versioned writes (D-14).** [`write_models_cache`] writes to a
//!   temp file then `rename`s it into place (same-FS atomic), and stamps the
//!   payload with `schema_version` and `fetched_at`.

// Forward-declared public API: the render-side lookup (Plan 02) and the
// out-of-band fetch/CLI (Plan 03) are the in-binary consumers of this module.
// Until those land, the binary crate sees these items as unused; the cache
// contract is exercised by `tests/ant_cache_tests.rs`. Mirrors the codebase's
// existing `#[allow(dead_code)]` on forward public API (e.g. `StatuslineError`).
#![allow(dead_code)]

use crate::error::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Current on-disk schema version for the models cache.
///
/// [`read_models_cache`] returns `None` for any cache whose `schema_version`
/// does not exactly equal this constant, so a versioned cache that pre-dates a
/// format change is treated as absent (graceful fall-through) rather than
/// misinterpreted.
pub const MODELS_CACHE_SCHEMA_VERSION: u32 = 1;

/// A single model's cached metadata.
///
/// Intentionally minimal in this foundation plan: only the context-window size
/// is stored. `0` (or a missing entry) means "unknown" and the caller falls
/// through to the next source (D-06). There is deliberately **no** key/secret
/// field (D-17 / ANT-04).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    /// Maximum input (context-window) tokens for the model. `0` => unknown,
    /// caller falls through to the next source.
    pub max_input_tokens: u64,
}

/// The versioned model-metadata cache as serialized to `models.json`.
///
/// Carries a `schema_version` (validated on read) and a `fetched_at` timestamp
/// (RFC3339 via chrono's serde support). The `models` map is keyed by canonical
/// model id. There is deliberately **no** key/secret field (D-17 / ANT-04).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsCache {
    /// On-disk schema version; must equal [`MODELS_CACHE_SCHEMA_VERSION`] to be
    /// accepted on read.
    pub schema_version: u32,
    /// When the cache was last fetched/written (UTC, serialized as RFC3339).
    pub fetched_at: DateTime<Utc>,
    /// Canonical-model-id => cached metadata.
    pub models: HashMap<String, ModelEntry>,
}

/// Resolve the models cache file path **without touching the filesystem**.
///
/// Returns `${XDG_CACHE_HOME:-~/.cache}/claudia-statusline/ant/models.json`.
/// This is PURE path math: it MUST NOT create any directory. The render-side
/// reader relies on this non-creating contract (D-16) so that a render against
/// a non-existent cache mutates nothing on disk.
pub fn models_cache_path() -> Result<PathBuf> {
    let path = dirs::cache_dir()
        .ok_or_else(|| {
            crate::error::StatuslineError::Config("Cannot determine cache directory".to_string())
        })?
        .join("claudia-statusline")
        .join("ant")
        .join("models.json");
    Ok(path)
}

/// Create the ant cache directory (`.../claudia-statusline/ant`) and return it.
///
/// This is the ONLY function in this module that creates a directory. On Unix
/// the directory is created with `0o700` (owner-only) permissions, mirroring
/// `session_state::get_cache_dir`. Only the writer (and tests) call this — the
/// render path NEVER does (D-16).
pub fn ensure_cache_dir() -> Result<PathBuf> {
    let dir = dirs::cache_dir()
        .ok_or_else(|| {
            crate::error::StatuslineError::Config("Cannot determine cache directory".to_string())
        })?
        .join("claudia-statusline")
        .join("ant");

    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(&dir)?;
    }

    #[cfg(not(unix))]
    {
        fs::create_dir_all(&dir)?;
    }

    Ok(dir)
}

/// Atomically write the models cache to disk.
///
/// Creates the cache directory via [`ensure_cache_dir`], serializes `cache` as
/// pretty JSON, writes it to a `*.tmp` sibling, then `rename`s it into place —
/// a same-filesystem atomic swap so a concurrent reader sees either the old or
/// the new whole file, never a torn one (D-14, mirrors
/// `session_state::write_state`).
pub fn write_models_cache(cache: &ModelsCache) -> Result<()> {
    // Create the directory (writer-only) before computing the file path.
    ensure_cache_dir()?;
    let path = models_cache_path()?;

    let json = serde_json::to_string_pretty(cache)?;

    let temp_path = path.with_extension("json.tmp");
    fs::write(&temp_path, json)?;
    fs::rename(&temp_path, &path)?;

    Ok(())
}

/// Read and validate the models cache, returning `None` on ANY failure.
///
/// This is the render-side reader and is therefore total and side-effect-free:
/// it resolves the path with the NON-creating [`models_cache_path`] (never
/// [`ensure_cache_dir`]), and collapses a missing file, an unreadable file, a
/// corrupt/garbage file, and a `schema_version` mismatch all to `None`. It never
/// panics (no `.unwrap()`), never spawns a process, never opens a socket, and
/// never creates a directory (D-16 / ANT-02).
pub fn read_models_cache() -> Option<ModelsCache> {
    let path = models_cache_path().ok()?;
    let content = fs::read_to_string(&path).ok()?;
    let cache: ModelsCache = serde_json::from_str(&content).ok()?;
    // A versioned cache that accepts every version defeats versioning: reject
    // anything that is not exactly the current schema (treat as absent).
    if cache.schema_version != MODELS_CACHE_SCHEMA_VERSION {
        return None;
    }
    Some(cache)
}
