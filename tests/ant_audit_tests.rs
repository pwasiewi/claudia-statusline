//! Wave 0 integration-test scaffold for the `sk-ant-` leak audit (ANT-33).
//!
//! One test is ACTIVE now: `key_pattern_matches_both_families` pins the shared
//! [`statusline::ant::audit::KEY_PATTERN`] against both the `api` and `admin01`
//! key families (Pitfall 5) plus a non-key. The remaining tests — proving that
//! written cache artifacts contain no key and that `.claudeignore` covers the
//! cache globs — are deferred to 09-04 and `#[ignore]`d so the suite stays green
//! now.
//!
//! Conventions (copied from `ant_invariant_tests.rs`): `#![cfg(unix)]`, every
//! PATH/env/XDG-mutating test is `#[serial]`, assertions are on recorded files /
//! exit status (no shell pipelines).

#![cfg(unix)]

use serial_test::serial;

/// ACTIVE (ANT-33): the shared leak pattern must catch BOTH key families and
/// reject a non-key string, so a planted Admin key cannot pass undetected.
#[test]
fn key_pattern_matches_both_families() {
    let re = regex::Regex::new(statusline::ant::audit::KEY_PATTERN).expect("KEY_PATTERN is valid");
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

#[test]
#[serial]
#[ignore = "filled by 09-04"]
fn written_cache_artifacts_contain_no_key() {
    // 09-04: run a sync against a fake exec under isolated XDG, then assert
    // scan_artifacts_for_keys() reports matched: false for every written file.
    unimplemented!("ANT-33 written-artifact scan is implemented in Plan 09-04");
}

#[test]
#[ignore = "filled by 09-04"]
fn claudeignore_covers_cache_globs() {
    // 09-04: read .claudeignore and assert the **/claudia-statusline/ant/ cache
    // globs are present.
    unimplemented!("ANT-33 .claudeignore coverage is implemented in Plan 09-04");
}
