//! Wave 0 integration-test scaffold for the `--max-age` sync throttle (ANT-30).
//!
//! These tests prove the self-throttle short-circuit added in Plan 09-02: with
//! `--max-age` set and a cache younger than the threshold, `ant sync-models` /
//! `ant sync-usage` must NOT invoke the fetch (no fake-exec marker); with
//! `--max-age` omitted it must always fetch. The fake-exec / isolated-HOME /
//! PATH-shaping harness is the same one used by `ant_usage_cli_tests.rs`; it is
//! deliberately deferred to 09-02 so this scaffold stays a thin failing target.
//!
//! Conventions (copied from the existing CLI harnesses): `#![cfg(unix)]`, every
//! PATH/env/XDG-mutating test is `#[serial]`, assertions are on recorded files /
//! exit status (no `cmd | tail` pipeline). The placeholder bodies below are
//! `#[ignore]`d so the suite stays green until 09-02 fills them in.

#![cfg(unix)]

use serial_test::serial;

#[test]
#[serial]
#[ignore = "filled by 09-02"]
fn skip_if_fresh_does_not_invoke_fetch() {
    // 09-02: place a fake `ant`/`curl` first on PATH that drops a marker file,
    // write a cache younger than --max-age, run `ant sync-models --max-age 10m`,
    // assert the marker is ABSENT (the throttle short-circuited before fetch).
    unimplemented!("ANT-30 throttle behavior is implemented in Plan 09-02");
}

#[test]
#[serial]
#[ignore = "filled by 09-02"]
fn omitted_max_age_always_fetches() {
    // 09-02: with --max-age omitted, the fetch ALWAYS runs even when a fresh
    // cache exists (the marker is PRESENT).
    unimplemented!("ANT-30 throttle behavior is implemented in Plan 09-02");
}
