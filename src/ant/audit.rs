//! Shared `sk-ant-` leak scanner over on-disk `ant` artifacts (D-16 / D-18).
//!
//! This is the single source of truth for "does any persisted artifact contain
//! a Claude API key?", reused by BOTH the CI leak test (ANT-33) and the
//! `ant doctor` security self-audit (Plan 09-03). It scans ONLY on-disk
//! artifacts — the model cache, per-account usage caches, and the debug log —
//! and **never reads the process environment** (D-18): a key passed via
//! `ANTHROPIC_API_KEY` is in-memory-only by design and is explicitly out of
//! scope for this scanner.
//!
//! Every error collapses to "skip this artifact" (mirroring the total
//! error->None contract of [`crate::ant::cache::read_models_cache`]): the
//! scanner never panics, spawns a process, opens a socket, or creates a
//! directory.

/// Regex matching BOTH key families: `sk-ant-api03-…` AND `sk-ant-admin01-…`.
///
/// Deliberately broad (`sk-ant-` + the base64url-ish key alphabet) so a planted
/// Admin key cannot slip past a narrower `api`-only pattern (Pitfall 5).
pub const KEY_PATTERN: &str = r"sk-ant-[A-Za-z0-9_-]+";

/// One scanned artifact and whether it contained a key-shaped string.
///
/// Only existing, readable artifacts produce a `LeakFinding`; missing files are
/// omitted entirely (no entry, no panic).
pub struct LeakFinding {
    /// The artifact path that was scanned.
    pub path: std::path::PathBuf,
    /// `true` if [`KEY_PATTERN`] matched anywhere in the file's contents.
    pub matched: bool,
}

/// Scan the on-disk `ant` artifacts for `sk-ant-` key strings.
///
/// Scans, in order: the model cache (if its path resolves), every file in the
/// per-account usage cache directory, and the cache-root debug log
/// (`statusline-debug.log`). Missing/unreadable files are skipped. This function
/// NEVER reads `std::env` (D-18), never spawns, never networks.
pub fn scan_artifacts_for_keys() -> Vec<LeakFinding> {
    let re = regex::Regex::new(KEY_PATTERN).expect("static KEY_PATTERN is a valid regex");
    let mut out = Vec::new();

    // Local closure: read a path totally; on success record a finding. Any error
    // (missing/unreadable/non-UTF8) is silently skipped — mirrors the cache
    // total-read contract.
    let mut check = |p: std::path::PathBuf| {
        if let Ok(content) = std::fs::read_to_string(&p) {
            out.push(LeakFinding {
                matched: re.is_match(&content),
                path: p,
            });
        }
    };

    // 1) The model cache (path may not resolve in a headless env -> skip).
    if let Ok(p) = crate::ant::cache::models_cache_path() {
        check(p);
    }

    // 2) Per-account usage caches + 3) the cache-root debug log.
    //    The debug log lives at the cache ROOT (VERIFIED toggle-debug.sh path),
    //    NOT under ant/.
    if let Some(dir) = dirs::cache_dir() {
        let usage_dir = dir.join("claudia-statusline").join("ant").join("usage");
        if let Ok(rd) = std::fs::read_dir(&usage_dir) {
            for entry in rd.flatten() {
                check(entry.path());
            }
        }
        check(dir.join("statusline-debug.log"));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_pattern_compiles_and_matches_both_families() {
        let re = regex::Regex::new(KEY_PATTERN).expect("valid regex");
        assert!(re.is_match("sk-ant-api03-AbC_123"), "api family must match");
        assert!(
            re.is_match("sk-ant-admin01-XyZ-789"),
            "admin01 family must match"
        );
        assert!(!re.is_match("not-a-key"), "a non-key must not match");
    }

    #[test]
    fn scan_never_panics() {
        // The scan is total: even with no artifacts present it returns cleanly
        // (the result may be empty or contain pre-existing debug-log/cache
        // entries from the dev environment — we assert only that it does not
        // panic).
        let _ = scan_artifacts_for_keys();
    }
}
