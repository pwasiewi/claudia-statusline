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

impl ModelsCache {
    /// Age of this cache relative to now (`Utc::now() - fetched_at`).
    ///
    /// Pure: no IO. May be negative if `fetched_at` is slightly in the future
    /// (clock skew); callers humanize via [`crate::ant::duration::humanize_age`],
    /// which clamps negatives to `<1m`.
    pub fn age(&self) -> chrono::Duration {
        chrono::Utc::now() - self.fetched_at
    }
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

// ===========================================================================
// Per-account usage cache (Phase 08, ANT-20 / ANT-24)
// ---------------------------------------------------------------------------
// Mirrors the models_* IO above but is PER ACCOUNT: each account's spend/token
// slice lives in its own file at `.../ant/usage/<account>.json`, written
// all-or-nothing (one file, no read-modify-merge of a shared map — D-10) so a
// sync of the active account can never clobber another account's slice. The
// account label is untrusted input that feeds a filesystem path, so it is
// sanitized (`sanitize_account_name`) in BOTH the writer and the reader before
// any `join` (T-08-PT / RESEARCH Pitfall 5). The struct carries NO key/secret
// field — only numeric spend/token totals + labels (D-17 / T-08-KEY-CACHE).
// ===========================================================================

/// Current on-disk schema version for the per-account usage cache.
///
/// [`read_usage_cache`] returns `None` for any cache whose `schema_version`
/// does not exactly equal this constant, so a cache that pre-dates a format
/// change is treated as absent (graceful fall-through) rather than
/// misinterpreted (D-16).
pub const USAGE_CACHE_SCHEMA_VERSION: u32 = 1;

/// Maximum accepted account-name length (defense-in-depth path-length cap).
const MAX_ACCOUNT_NAME_LEN: usize = 128;

/// Per-model token usage broken down by Anthropic's billed token types (D-13).
///
/// All five fields are cached so a downstream renderer can present either the
/// per-type detail or the per-model **total** (the sum of the five fields).
/// Defaults to all-zero so a partial API response degrades to zeros rather than
/// failing to deserialize.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenBreakdown {
    /// Uncached input tokens.
    pub uncached_input: u64,
    /// Cache-read input tokens.
    pub cache_read_input: u64,
    /// 1-hour cache-creation input tokens.
    pub cache_creation_1h: u64,
    /// 5-minute cache-creation input tokens.
    pub cache_creation_5m: u64,
    /// Output tokens.
    pub output: u64,
}

impl TokenBreakdown {
    /// Per-model TOTAL: the sum of all five token-type fields (saturating, so a
    /// pathological response can never overflow). This is the value the render
    /// builder (08-03) humanizes into a `1.2M`-style figure.
    pub fn total(&self) -> u64 {
        self.uncached_input
            .saturating_add(self.cache_read_input)
            .saturating_add(self.cache_creation_1h)
            .saturating_add(self.cache_creation_5m)
            .saturating_add(self.output)
    }
}

/// The versioned per-account usage/cost cache serialized to `usage/<account>.json`.
///
/// Carries a `schema_version` (validated on read) and a `fetched_at` timestamp.
/// Spend is stored as already-converted USD (`today_usd`/`mtd_usd`); the `tz`
/// label records which timezone the "today"/MTD windows were computed in (so the
/// render can label it — ANT-23). There is deliberately **no** key/secret field
/// (D-17 / T-08-KEY-CACHE): only numeric totals + labels live on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageCache {
    /// On-disk schema version; must equal [`USAGE_CACHE_SCHEMA_VERSION`] to be
    /// accepted on read.
    pub schema_version: u32,
    /// When the cache was last fetched/written (UTC, serialized as RFC3339).
    pub fetched_at: DateTime<Utc>,
    /// The (sanitized) account label this slice belongs to.
    pub account: String,
    /// Today's spend in USD.
    pub today_usd: f64,
    /// Month-to-date spend in USD.
    pub mtd_usd: f64,
    /// Timezone label the today/MTD windows were computed in (e.g. `"UTC"`).
    pub tz: String,
    /// Canonical-model-id => per-type token breakdown.
    pub tokens_by_model: HashMap<String, TokenBreakdown>,
}

impl UsageCache {
    /// Age of this cache relative to now (`Utc::now() - fetched_at`).
    ///
    /// Pure: no IO. May be negative under clock skew; humanize via
    /// [`crate::ant::duration::humanize_age`], which clamps negatives to `<1m`.
    pub fn age(&self) -> chrono::Duration {
        chrono::Utc::now() - self.fetched_at
    }
}

/// Validate and return an account label safe to use as a path component.
///
/// The account name is untrusted (it comes from user TOML / `STATUSLINE_ANT_ACCOUNT`)
/// and is interpolated into a filesystem path, so it is confined to
/// `[A-Za-z0-9._-]` and `empty` / `.` / `..` / anything containing a path
/// separator or over-length is **rejected** (never silently rewritten — so the
/// writer and reader always agree on the path; RESEARCH Pitfall 5 / T-08-PT).
/// Returns the validated owned name on success.
///
/// `pub` (not `pub(crate)`) so the external integration test
/// `tests/ant_usage_tests.rs::sanitize_account_name` — a separate crate — can
/// exercise it directly, mirroring the existing `pub` `models_*` cache surface.
pub fn sanitize_account_name(name: &str) -> Result<String> {
    use crate::error::StatuslineError;

    if name.is_empty() {
        return Err(StatuslineError::Config(
            "ant account name must not be empty".to_string(),
        ));
    }
    if name.len() > MAX_ACCOUNT_NAME_LEN {
        return Err(StatuslineError::Config(format!(
            "ant account name exceeds {MAX_ACCOUNT_NAME_LEN} chars"
        )));
    }
    // Reject the special directory entries explicitly (they are otherwise made of
    // allowed chars) — these are the classic traversal footguns.
    if name == "." || name == ".." {
        return Err(StatuslineError::Config(
            "ant account name must not be '.' or '..'".to_string(),
        ));
    }
    // Confine to a conservative alphabet. This also rejects '/', '\\', and NUL,
    // so no path separator can sneak through before the join.
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        return Err(StatuslineError::Config(format!(
            "ant account name '{name}' contains disallowed characters (allowed: A-Za-z0-9._-)"
        )));
    }
    Ok(name.to_string())
}

/// Resolve a per-account usage cache file path **without touching the filesystem**.
///
/// Returns `${XDG_CACHE_HOME:-~/.cache}/claudia-statusline/ant/usage/<account>.json`.
/// The account name is sanitized via [`sanitize_account_name`] BEFORE the join
/// (T-08-PT), and this is PURE path math: it MUST NOT create any directory. The
/// render-side reader relies on this non-creating contract (D-16).
pub(crate) fn usage_cache_path(account: &str) -> Result<PathBuf> {
    let safe = sanitize_account_name(account)?;
    let path = dirs::cache_dir()
        .ok_or_else(|| {
            crate::error::StatuslineError::Config("Cannot determine cache directory".to_string())
        })?
        .join("claudia-statusline")
        .join("ant")
        .join("usage")
        .join(format!("{safe}.json"));
    Ok(path)
}

/// Create the per-account usage cache directory (`.../ant/usage`) and return it.
///
/// Like [`ensure_cache_dir`], this is a writer-only directory-creating helper
/// (0o700 on Unix). The render path NEVER calls it (D-16).
fn ensure_usage_cache_dir() -> Result<PathBuf> {
    let dir = dirs::cache_dir()
        .ok_or_else(|| {
            crate::error::StatuslineError::Config("Cannot determine cache directory".to_string())
        })?
        .join("claudia-statusline")
        .join("ant")
        .join("usage");

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

/// Atomically write ONE account's usage slice to `usage/<account>.json`.
///
/// Creates the usage subdir via [`ensure_usage_cache_dir`], serializes `cache`
/// as pretty JSON, writes it to a `*.tmp` sibling, then `rename`s it into place
/// (same-FS atomic swap). Writes exactly one file — there is NO read-modify-merge
/// of a shared map (D-10), so syncing one account can never clobber another's
/// slice. The account name is sanitized via [`usage_cache_path`].
pub(crate) fn write_usage_cache(cache: &UsageCache) -> Result<()> {
    ensure_usage_cache_dir()?;
    let path = usage_cache_path(&cache.account)?;

    let json = serde_json::to_string_pretty(cache)?;

    let temp_path = path.with_extension("json.tmp");
    fs::write(&temp_path, json)?;
    fs::rename(&temp_path, &path)?;

    Ok(())
}

/// Read and validate one account's usage cache, returning `None` on ANY failure.
///
/// Total and side-effect-free (the render-side reader): it sanitizes the account
/// name, resolves the path with the NON-creating [`usage_cache_path`] (never
/// [`ensure_usage_cache_dir`]), and collapses a bad name, a missing file, an
/// unreadable file, a corrupt file, and a `schema_version` mismatch ALL to
/// `None`. Never panics, never spawns a process, never opens a socket, never
/// creates a directory (D-16 / ANT-02).
pub(crate) fn read_usage_cache(account: &str) -> Option<UsageCache> {
    let path = usage_cache_path(account).ok()?;
    let content = fs::read_to_string(&path).ok()?;
    let cache: UsageCache = serde_json::from_str(&content).ok()?;
    if cache.schema_version != USAGE_CACHE_SCHEMA_VERSION {
        return None;
    }
    Some(cache)
}

#[cfg(test)]
mod usage_tests {
    use super::*;

    #[test]
    fn sanitize_rejects_traversal_and_separators() {
        for bad in ["../../etc/foo", "a/b", "", ".", "..", "a\\b", "a b", "a\0b"] {
            assert!(
                sanitize_account_name(bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
        assert!(sanitize_account_name(&"a".repeat(MAX_ACCOUNT_NAME_LEN + 1)).is_err());
    }

    #[test]
    fn sanitize_accepts_safe_names() {
        for ok in ["work", "team-1", "acct.2", "A_b.C-9"] {
            assert_eq!(sanitize_account_name(ok).unwrap(), ok);
        }
    }

    #[test]
    fn usage_path_is_under_usage_subdir_and_has_no_traversal() {
        let p = usage_cache_path("work").expect("safe name resolves");
        assert!(p.ends_with("ant/usage/work.json"), "got {p:?}");
        assert!(
            !p.to_string_lossy().contains(".."),
            "no traversal segment in {p:?}"
        );
        // Path resolution must NOT have created the directory (D-16).
        assert!(!p.exists(), "usage_cache_path must not create the file/dir");
    }

    #[test]
    fn token_breakdown_total_sums_all_fields() {
        let tb = TokenBreakdown {
            uncached_input: 1,
            cache_read_input: 2,
            cache_creation_1h: 4,
            cache_creation_5m: 8,
            output: 16,
        };
        assert_eq!(tb.total(), 31);
    }

    #[test]
    fn read_missing_account_is_none() {
        // A name that is valid but has no file on disk reads as None (total).
        assert!(read_usage_cache("definitely-absent-account").is_none());
        // A name that fails sanitization also collapses to None (never errs).
        assert!(read_usage_cache("../escape").is_none());
    }
}
