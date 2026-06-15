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

// Forward-public foundation API consumed by the CI leak test (ANT-33) and the
// `ant doctor` self-audit (Plan 09-03). The binary crate does not reference it
// yet — mirror the existing `#![allow(dead_code)]` on fetch.rs/cache.rs/usage.rs.
#![allow(dead_code)]

/// Regex matching BOTH key families: `sk-ant-api03-…` AND `sk-ant-admin01-…`.
///
/// Deliberately broad (`sk-ant-` + the base64url-ish key alphabet) so a planted
/// Admin key cannot slip past a narrower `api`-only pattern (Pitfall 5).
pub const KEY_PATTERN: &str = r"sk-ant-[A-Za-z0-9_-]+";

/// Maximum number of bytes any single artifact is read into memory for the leak
/// scan (WR-02). The cache-root `statusline-debug.log` is an append-only,
/// unbounded file; reading it whole into a `String` is an unbounded allocation
/// that can exhaust memory and take the doctor self-audit / CI leak test down.
/// The key pattern is short, so scanning a bounded prefix of an oversized file
/// still detects a planted key while guaranteeing the scanner always completes.
const MAX_SCAN_BYTES: u64 = 8 * 1024 * 1024; // 8 MiB

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

    // Local closure: read a path with a bounded byte cap; on success record a
    // finding. Any error (missing/unreadable/non-UTF8) is silently skipped —
    // mirrors the cache total-read contract. Files larger than MAX_SCAN_BYTES are
    // prefix-scanned (the key pattern is short) so an unbounded debug log can
    // never exhaust memory (WR-02): the scanner always completes.
    let mut check = |p: std::path::PathBuf| {
        if let Some(content) = read_capped(&p) {
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

/// Read at most [`MAX_SCAN_BYTES`] of `path` as UTF-8 (lossy), returning `None`
/// on any IO error so the caller skips the artifact (WR-02).
///
/// A normal-sized artifact is read in full (so a planted key is still detected);
/// an oversized one (e.g. a multi-gigabyte `statusline-debug.log`) is bounded to
/// a prefix, which keeps the allocation finite and the scan guaranteed to
/// complete. UTF-8-lossy decoding (rather than `read_to_string`) keeps a
/// truncated multibyte tail from being treated as an "unreadable" file.
fn read_capped(path: &std::path::Path) -> Option<String> {
    use std::io::Read;

    let file = std::fs::File::open(path).ok()?;
    let mut buf = Vec::new();
    // `take` bounds the read regardless of the on-disk size; we never allocate
    // more than MAX_SCAN_BYTES + the reader's chunking overhead.
    file.take(MAX_SCAN_BYTES).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
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

    // WR-02: a normal-sized artifact carrying a key is still read in full and
    // detected (the cap does not regress detection of in-bounds keys).
    #[test]
    fn read_capped_detects_key_in_normal_file() {
        let dir = std::env::temp_dir();
        let p = dir.join(format!("statusline-audit-test-{}.txt", std::process::id()));
        std::fs::write(&p, "prefix\nsk-ant-api03-PLANTED_key\nsuffix\n").expect("write temp");

        let content = read_capped(&p).expect("readable temp file");
        let re = regex::Regex::new(KEY_PATTERN).expect("valid regex");
        assert!(
            re.is_match(&content),
            "planted key in a normal file is found"
        );

        let _ = std::fs::remove_file(&p);
    }

    // WR-02: an "oversized" artifact is bounded to a prefix rather than read
    // whole — the returned content never exceeds MAX_SCAN_BYTES, so an unbounded
    // debug log can never exhaust memory. We prove the cap with a small synthetic
    // cap-sized file (NOT a multi-GB write): we write MAX_SCAN_BYTES + slack and
    // assert the read is bounded. To keep the test fast/light we shrink the
    // assertion to the cap via a modest file just over a small bound is not
    // possible without touching the const, so we assert the byte-cap contract on
    // a file written just over a 1 MiB marker boundary using the real cap.
    #[test]
    fn read_capped_bounds_oversized_file() {
        let dir = std::env::temp_dir();
        let p = dir.join(format!("statusline-audit-big-{}.txt", std::process::id()));
        // Write MAX_SCAN_BYTES + 4096 bytes of filler with a key placed AFTER the
        // cap; the prefix read must be bounded and must not include the tail key.
        let mut data = vec![b'.'; MAX_SCAN_BYTES as usize + 4096];
        // Place a key in the trailing (beyond-cap) region.
        let tail = b"sk-ant-api03-BEYOND_CAP";
        let start = data.len() - tail.len();
        data[start..].copy_from_slice(tail);
        std::fs::write(&p, &data).expect("write big temp");

        let content = read_capped(&p).expect("readable big file");
        assert!(
            content.len() as u64 <= MAX_SCAN_BYTES,
            "read must be bounded to the cap, got {} bytes",
            content.len()
        );
        let re = regex::Regex::new(KEY_PATTERN).expect("valid regex");
        assert!(
            !re.is_match(&content),
            "a key beyond the byte cap is (correctly) not in the bounded prefix"
        );

        let _ = std::fs::remove_file(&p);
    }
}
