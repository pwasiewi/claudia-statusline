//! Wave 0 integration-test scaffold for `ant doctor` (ANT-32).
//!
//! These tests prove the diagnostics command added in Plan 09-03: `--json`
//! emits a stable schema; a passive (non-`--probe`) run executes NO credential
//! command (the fake `admin_key_command` marker is ABSENT, mirroring the
//! no-spawn proof in `ant_invariant_tests.rs`); and the human report shows
//! credential SOURCE labels without ever printing an `sk-ant-` secret. The
//! isolated-env + fake-exec harness is deferred to 09-03 so this scaffold stays
//! a thin failing target.
//!
//! Conventions (copied from the existing CLI harnesses): `#![cfg(unix)]`, every
//! PATH/env/XDG-mutating test is `#[serial]`, assertions are on recorded files /
//! exit status (no `cmd | tail` pipeline). The placeholder bodies below are
//! `#[ignore]`d so the suite stays green until 09-03 fills them in.

#![cfg(unix)]

use serial_test::serial;

#[test]
#[serial]
#[ignore = "filled by 09-03"]
fn doctor_json_emits_expected_schema() {
    // 09-03: run `ant doctor --json`, parse stdout as JSON, assert the expected
    // top-level keys (ant_on_path, active_account, caches, credentials, audit).
    unimplemented!("ANT-32 doctor is implemented in Plan 09-03");
}

#[test]
#[serial]
#[ignore = "filled by 09-03"]
fn doctor_passive_does_not_exec_credential_command() {
    // 09-03: a passive (no --probe) run must NOT spawn the account's
    // admin_key_command (its fake-exec marker is ABSENT).
    unimplemented!("ANT-32 doctor is implemented in Plan 09-03");
}

#[test]
#[serial]
#[ignore = "filled by 09-03"]
fn doctor_reports_source_labels_without_secrets() {
    // 09-03: the report shows credential SOURCE labels (e.g. "ANTHROPIC_API_KEY
    // (env)") with no `sk-ant-` substring anywhere in the output.
    unimplemented!("ANT-32 doctor is implemented in Plan 09-03");
}
