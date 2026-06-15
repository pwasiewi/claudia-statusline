//! Integration tests for the `sk-ant-` leak audit (ANT-33).
//!
//! Three guarantees, all ACTIVE:
//!
//! 1. **`key_pattern_matches_both_families`** — the shared
//!    [`statusline::ant::audit::KEY_PATTERN`] catches BOTH the `api` and
//!    `admin01` key families (Pitfall 5) and rejects a non-key, so a planted
//!    Admin key cannot slip past an api-only pattern.
//! 2. **`written_cache_artifacts_contain_no_key`** — after the REAL cache writers
//!    persist representative (key-free) artifacts under an isolated cache root,
//!    [`statusline::ant::audit::scan_artifacts_for_keys`] reports `matched ==
//!    false` for every written artifact (D-17/D-18). This is the permanent
//!    regression guard that the persisted cache schema carries no credential.
//! 3. **`claudeignore_covers_cache_globs`** — the REPO `.claudeignore` (read via
//!    `CARGO_MANIFEST_DIR`, not a fixture) contains a glob covering
//!    `**/claudia-statusline/ant/`, so the recursive ignore also covers
//!    `usage/`.
//!
//! Conventions (copied from `ant_invariant_tests.rs` / `ant_usage_cli_tests.rs`):
//! `#![cfg(unix)]`, every PATH/env/XDG-mutating test is `#[serial]`, assertions
//! are on recorded files / values directly (no shell pipelines). Per Pitfall 6 we
//! NEVER write a real-looking key to a persistent path — the artifacts written
//! here are deliberately key-free, and the cache root is an isolated `tempfile`.

#![cfg(unix)]

use std::collections::HashMap;
use std::fs;

use chrono::Utc;
use serial_test::serial;
use tempfile::TempDir;

use statusline::ant::audit::{scan_artifacts_for_keys, KEY_PATTERN};
use statusline::ant::cache::{
    models_cache_path, write_models_cache, ModelEntry, ModelsCache, MODELS_CACHE_SCHEMA_VERSION,
};

/// ACTIVE (ANT-33): the shared leak pattern must catch BOTH key families and
/// reject a non-key string, so a planted Admin key cannot pass undetected.
#[test]
fn key_pattern_matches_both_families() {
    let re = regex::Regex::new(KEY_PATTERN).expect("KEY_PATTERN is valid");
    assert!(
        re.is_match("sk-ant-api03-AbC_123"),
        "api03 key family must match"
    );
    assert!(
        re.is_match("sk-ant-admin01-XyZ-789"),
        "admin01 key family must match"
    );
    assert!(!re.is_match("not-a-key"), "a non-key must not match");
}

/// Point `dirs::cache_dir()` at `root` for BOTH platforms: `XDG_CACHE_HOME`
/// drives it on Linux, while on macOS `dirs` 5.x resolves `~/Library/Caches`
/// from `HOME`. Setting both isolates the in-process scan regardless of OS.
fn isolate_cache_dir(root: &std::path::Path) {
    std::env::set_var("HOME", root);
    std::env::set_var("XDG_CACHE_HOME", root);
}

/// ACTIVE (ANT-33 / D-17 / D-18): after the real cache writers persist
/// representative artifacts under an isolated cache root, the shared scanner
/// must report NO key match on any written file. Pitfall 6: isolate the cache
/// dir with `tempfile` + `#[serial]`, and write only KEY-FREE payloads — never a
/// real-looking key to a persistent path.
#[test]
#[serial]
fn written_cache_artifacts_contain_no_key() {
    let tmp = TempDir::new().expect("cache root temp dir");
    isolate_cache_dir(tmp.path());

    // 1) Write a real models cache via the production writer (atomic, versioned).
    //    The cache struct carries NO key field by construction (D-17) — the
    //    payload below is the model-metadata it actually persists.
    let mut models = HashMap::new();
    models.insert(
        "claude-opus-4".to_string(),
        ModelEntry {
            max_input_tokens: 200_000,
        },
    );
    let cache = ModelsCache {
        schema_version: MODELS_CACHE_SCHEMA_VERSION,
        fetched_at: Utc::now(),
        models,
    };
    write_models_cache(&cache).expect("write models cache");
    let models_path = models_cache_path().expect("resolve models cache path");
    assert!(models_path.exists(), "models cache must be written");

    // The scanner anchors usage/ and the debug log at the OS cache dir, which is
    // NOT necessarily `tmp` itself (on macOS `dirs::cache_dir()` resolves
    // `$HOME/Library/Caches`). Derive the same root the scanner uses so our
    // written artifacts land exactly where it looks.
    let cache_root = dirs::cache_dir().expect("resolve OS cache dir");

    // 2) Write a representative per-account usage cache directly at the path the
    //    scanner enumerates (`.../ant/usage/<account>.json`). The on-disk usage
    //    writer is `pub(crate)`, so we materialize the same key-free shape here:
    //    only numeric totals + labels, never a credential (D-17 / T-08-KEY-CACHE).
    let usage_dir = cache_root
        .join("claudia-statusline")
        .join("ant")
        .join("usage");
    fs::create_dir_all(&usage_dir).expect("create usage dir");
    let usage_json = r#"{
  "schema_version": 1,
  "fetched_at": "2026-06-15T00:00:00Z",
  "account": "work",
  "today_usd": 5.0,
  "mtd_usd": 42.5,
  "tz": "UTC",
  "tokens_by_model": { "claude-opus-4": { "uncached_input": 100, "output": 40 } }
}"#;
    let usage_path = usage_dir.join("work.json");
    fs::write(&usage_path, usage_json).expect("write usage cache");

    // 3) Write a representative debug log at the cache ROOT (the scanner reads
    //    `dirs::cache_dir().join("statusline-debug.log")`). Sync code redacts
    //    credentials, so a real debug log carries no key — model that here.
    let debug_log = cache_root.join("statusline-debug.log");
    fs::write(
        &debug_log,
        "DEBUG sync-models: fetched 1 model, wrote cache (no secrets logged)\n",
    )
    .expect("write debug log");

    // Scan in-process: every written artifact must be reported key-free.
    let findings = scan_artifacts_for_keys();
    assert!(
        !findings.is_empty(),
        "the scanner must report the artifacts we just wrote (it found none)"
    );
    for f in &findings {
        assert!(
            !f.matched,
            "written artifact must contain no sk-ant- key: {:?}",
            f.path
        );
    }

    // The three artifacts we wrote must each be among the scanned findings.
    let scanned: Vec<_> = findings.iter().map(|f| f.path.clone()).collect();
    assert!(
        scanned.contains(&models_path),
        "models cache must be scanned: {scanned:?}"
    );
    assert!(
        scanned.contains(&usage_path),
        "usage cache must be scanned: {scanned:?}"
    );
    assert!(
        scanned.contains(&debug_log),
        "debug log must be scanned: {scanned:?}"
    );
}

/// ACTIVE (ANT-33 / D-19): the repository `.claudeignore` must carry a glob
/// covering `**/claudia-statusline/ant/` so any in-tree cache (and `usage/`
/// recursively) is excluded from agent reads. Read the REAL repo file via
/// `CARGO_MANIFEST_DIR` (not a fixture) so drift in the shipped ignore is caught.
#[test]
fn claudeignore_covers_cache_globs() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let claudeignore = std::path::Path::new(manifest_dir).join(".claudeignore");
    let body = fs::read_to_string(&claudeignore)
        .unwrap_or_else(|e| panic!("repo .claudeignore must exist at {claudeignore:?}: {e}"));

    let covers = body.lines().any(|line| {
        let l = line.trim();
        !l.starts_with('#') && l.contains("claudia-statusline/ant")
    });
    assert!(
        covers,
        ".claudeignore must contain a glob covering claudia-statusline/ant (recursive => usage/): \n{body}"
    );
}
