//! Integration tests for the `ant` versioned model-metadata cache.
//!
//! Cache IO is routed away from the real user-home cache by
//! `test_support::init()`, which isolates `HOME` and the `XDG_*` dirs to a
//! per-process temp directory. (`dirs::cache_dir()` — used by the cache module —
//! honors `XDG_CACHE_HOME` on Linux and `~/Library/Caches` (i.e. the isolated
//! `HOME`) on macOS, so isolation holds on both platforms.)
//!
//! Because these tests mutate / depend on process-global env state and a shared
//! on-disk cache location, every test is annotated `#[serial]` (the repo's
//! global test lock for env-mutating tests). Each test first removes any
//! pre-existing ant cache dir so it starts from a known-empty state.

mod test_support;

use std::collections::HashMap;
use std::path::PathBuf;

use serial_test::serial;
use statusline::ant::cache::{
    models_cache_path, read_models_cache, write_models_cache, ModelEntry, ModelsCache,
    MODELS_CACHE_SCHEMA_VERSION,
};

/// Establish isolation (HOME / XDG_* -> temp) and return a freshly-empty ant
/// cache dir under that isolated location. Removes any leftover cache dir from a
/// prior serial test so each test starts known-empty.
fn fresh_isolated_cache() -> PathBuf {
    let _guard = test_support::init();
    let dir = ant_dir();
    // Remove any leftover cache directory from a previous test in this process.
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn sample_cache() -> ModelsCache {
    let mut models = HashMap::new();
    models.insert(
        "claude-sonnet-4".to_string(),
        ModelEntry {
            max_input_tokens: 200_000,
        },
    );
    models.insert(
        "claude-opus-4".to_string(),
        ModelEntry {
            max_input_tokens: 1_000_000,
        },
    );
    ModelsCache {
        schema_version: MODELS_CACHE_SCHEMA_VERSION,
        fetched_at: chrono::Utc::now(),
        models,
    }
}

/// The ant cache dir (`.../claudia-statusline/ant`) under the resolved cache
/// root — derived from the same non-creating path resolver the code uses.
fn ant_dir() -> PathBuf {
    models_cache_path()
        .expect("path resolves")
        .parent()
        .expect("has parent")
        .to_path_buf()
}

#[test]
#[serial]
fn round_trip_write_then_read_preserves_models() {
    let _dir = fresh_isolated_cache();

    let cache = sample_cache();
    write_models_cache(&cache).expect("write succeeds");

    let read = read_models_cache().expect("read returns Some after write");
    assert_eq!(read.schema_version, MODELS_CACHE_SCHEMA_VERSION);
    assert_eq!(read.models.len(), 2);
    assert_eq!(
        read.models.get("claude-opus-4").unwrap().max_input_tokens,
        1_000_000
    );
    assert_eq!(
        read.models.get("claude-sonnet-4").unwrap().max_input_tokens,
        200_000
    );
}

#[test]
#[serial]
fn written_file_contains_schema_version_and_fetched_at() {
    let _dir = fresh_isolated_cache();

    write_models_cache(&sample_cache()).expect("write succeeds");

    let path = models_cache_path().expect("path resolves");
    let raw = std::fs::read_to_string(&path).expect("file readable");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON");
    assert!(
        value.get("schema_version").is_some(),
        "serialized cache must contain schema_version"
    );
    assert!(
        value.get("fetched_at").is_some(),
        "serialized cache must contain fetched_at"
    );
    // No secret/key field leaked into the serialized cache (D-17 / ANT-04).
    let lower = raw.to_lowercase();
    assert!(
        !lower.contains("api_key") && !lower.contains("\"key\""),
        "serialized cache must not contain any key/secret field"
    );
}

#[test]
#[serial]
fn write_lands_under_resolved_cache_dir() {
    let dir = fresh_isolated_cache();

    write_models_cache(&sample_cache()).expect("write succeeds");

    let expected = dir.join("models.json");
    assert!(
        expected.exists(),
        "cache file must be written under the resolved cache dir, expected {:?}",
        expected
    );
    assert_eq!(models_cache_path().unwrap(), expected);
    // The path lives under .../claudia-statusline/ant/ (honoring the cache root,
    // which is XDG_CACHE_HOME on Linux and the isolated HOME on macOS).
    assert!(expected
        .to_string_lossy()
        .contains("claudia-statusline/ant"));
}

#[test]
#[serial]
fn reading_missing_file_returns_none() {
    let _dir = fresh_isolated_cache();
    // Nothing written yet.
    assert!(read_models_cache().is_none());
}

#[test]
#[serial]
fn reading_corrupt_file_returns_none() {
    let _dir = fresh_isolated_cache();

    // Create the dir and write garbage to models.json.
    let path = models_cache_path().expect("path resolves");
    std::fs::create_dir_all(path.parent().unwrap()).expect("create dir");
    std::fs::write(&path, "this is not json {{{").expect("write garbage");

    assert!(
        read_models_cache().is_none(),
        "a corrupt cache file must yield None, not panic"
    );
}

#[test]
#[serial]
fn reading_version_mismatch_returns_none() {
    let _dir = fresh_isolated_cache();

    // Write a valid-shaped cache JSON with a future schema_version.
    let path = models_cache_path().expect("path resolves");
    std::fs::create_dir_all(path.parent().unwrap()).expect("create dir");
    let body = serde_json::json!({
        "schema_version": 999u32,
        "fetched_at": chrono::Utc::now(),
        "models": { "claude-opus-4": { "max_input_tokens": 1_000_000u64 } }
    });
    std::fs::write(&path, serde_json::to_string_pretty(&body).unwrap()).expect("write");

    assert!(
        read_models_cache().is_none(),
        "a schema_version mismatch must yield None (versioning must be enforced)"
    );
}

#[test]
#[serial]
fn read_does_not_create_cache_dir() {
    let dir = fresh_isolated_cache();

    assert!(
        !dir.exists(),
        "precondition: ant cache dir must not exist yet"
    );

    let result = read_models_cache();
    assert!(result.is_none(), "read of absent cache returns None");
    assert!(
        !dir.exists(),
        "read_models_cache must NOT create the claudia-statusline/ant directory (D-16)"
    );
}
