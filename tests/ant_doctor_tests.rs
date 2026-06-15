//! ANT-32 integration tests for `statusline ant doctor` (Plan 09-03).
//!
//! These prove the diagnostics command's three load-bearing guarantees:
//!
//! 1. **`--json` schema** — `ant doctor --json` emits valid JSON carrying the
//!    documented top-level keys (`ant_on_path`, `caches`, `credentials`,
//!    `audit`, ...).
//! 2. **Passive == no credential exec** (D-13 / T-09-01) — a passive (no
//!    `--probe`) run with a configured account whose `admin_key_command` would
//!    drop a marker file does NOT create the marker: the credential command is
//!    never spawned. Mirrors the no-spawn marker proof in
//!    `ant_invariant_tests.rs`.
//! 3. **No secret in output** (D-13 / T-09-D-01) — neither the human nor the
//!    JSON report contains an `sk-ant-` substring, and the credential SOURCE
//!    labels are present.
//!
//! Conventions (copied from `ant_usage_cli_tests.rs`): `#![cfg(unix)]`, every
//! PATH/env/XDG-mutating test is `#[serial]`, assertions are on captured output /
//! recorded files (no `cmd | tail` pipeline).

#![cfg(unix)]

mod test_support;

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serial_test::serial;
use tempfile::TempDir;

/// A fully isolated environment for one `ant doctor` invocation: an isolated
/// HOME + XDG dirs (so caches/config never touch the host), a config file with a
/// chosen `[ant]` section, and a `bin` dir we control for the fake
/// `admin_key_command` (whose only job is to drop a marker file IF run).
struct DoctorEnv {
    home: TempDir,
    bin: TempDir,
    /// The marker a fake `admin_key_command` writes IF (and only if) it is run.
    marker: PathBuf,
    config_path: PathBuf,
}

impl DoctorEnv {
    fn new(ant_section: &str) -> Self {
        let home = TempDir::new().expect("home temp dir");
        let bin = TempDir::new().expect("bin temp dir");
        let marker = home.path().join("cred_was_run.marker");

        let config_path = home.path().join("statusline.toml");
        let mut f = fs::File::create(&config_path).expect("write config");
        write!(f, "{}", ant_section).expect("write config body");

        DoctorEnv {
            home,
            bin,
            marker,
            config_path,
        }
    }

    /// Install a fake `admin_key_command` target: an executable that, IF run,
    /// drops the marker file AND prints a (fake) key on stdout. A passive doctor
    /// must NEVER spawn it (so the marker stays absent).
    fn install_fake_admin_key(&self, name: &str) {
        let exe = self.bin.path().join(name);
        let body = format!(
            "#!/bin/sh\n\
             touch \"{marker}\"\n\
             printf '%s' 'sk-ant-admin01-FAKE'\n\
             exit 0\n",
            marker = self.marker.display(),
        );
        fs::write(&exe, body).expect("write fake admin key cmd");
        let mut perms = fs::metadata(&exe).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&exe, perms).expect("chmod +x");
    }

    /// Run `statusline ant doctor [--json]` with the given optional active
    /// account. Returns `(success, stdout, stderr)`. PATH contains ONLY our
    /// controlled bin dir so the only resolvable `admin_key_command` is the fake.
    fn run(&self, json: bool, account: Option<&str>) -> (bool, String, String) {
        let mut cmd = Command::new(test_support::test_binary());
        cmd.arg("ant").arg("doctor");
        if json {
            cmd.arg("--json");
        }
        cmd.env("HOME", self.home.path())
            .env("XDG_CACHE_HOME", self.home.path().join("cache"))
            .env("XDG_CONFIG_HOME", self.home.path().join("config"))
            .env("XDG_DATA_HOME", self.home.path().join("data"))
            .env("STATUSLINE_CONFIG_PATH", &self.config_path)
            .env("PATH", self.bin.path())
            .env("NO_COLOR", "1")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("STATUSLINE_ANT_ACCOUNT")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(a) = account {
            cmd.env("STATUSLINE_ANT_ACCOUNT", a);
        }
        let out = cmd.output().expect("spawn statusline ant doctor");
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }
}

/// An `[ant]` config mapping account `name` to a fake `admin_key_command`.
fn ant_config_with_account(name: &str, key_cmd: &str) -> String {
    format!("[ant]\nenabled = true\n\n[ant.accounts.{name}]\nadmin_key_command = [\"{key_cmd}\"]\n")
}

// ---------------------------------------------------------------------------
// 1) --json emits the documented schema.
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn doctor_json_emits_expected_schema() {
    let env = DoctorEnv::new("[ant]\nenabled = true\n");
    let (ok, stdout, stderr) = env.run(true, None);
    assert!(ok, "doctor --json must exit 0; stderr={stderr}");

    let v: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("doctor --json must emit valid JSON");
    let obj = v.as_object().expect("top-level JSON object");

    for key in [
        "ant_on_path",
        "active_account",
        "caches",
        "credentials",
        "audit",
    ] {
        assert!(
            obj.contains_key(key),
            "missing top-level key {key}: {stdout}"
        );
    }
    // caches has models + usage sub-objects.
    let caches = obj["caches"].as_object().expect("caches object");
    assert!(caches.contains_key("models"), "caches.models present");
    assert!(caches.contains_key("usage"), "caches.usage present");
    // audit reports a clean flag + a scanned count.
    let audit = obj["audit"].as_object().expect("audit object");
    assert!(audit.contains_key("clean"), "audit.clean present");
    assert!(audit.contains_key("scanned"), "audit.scanned present");
}

// ---------------------------------------------------------------------------
// 2) A passive (no --probe) run NEVER executes the credential command.
//    The fake admin_key_command would drop a marker file if run; it stays absent.
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn doctor_passive_does_not_exec_credential_command() {
    let env = DoctorEnv::new(&ant_config_with_account("work", "fake-admin-key-cmd"));
    env.install_fake_admin_key("fake-admin-key-cmd");

    // Passive run (no --probe) with the account active.
    let (ok, stdout, stderr) = env.run(false, Some("work"));
    assert!(ok, "passive doctor must exit 0; stderr={stderr}");

    assert!(
        !env.marker.exists(),
        "passive doctor must NOT run admin_key_command (marker present): {stdout}"
    );
    // And it should still report the usage credential SOURCE label (no exec).
    assert!(
        stdout.contains("admin_key_command (credential command)"),
        "doctor must report the usage credential source label: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// 3) The report (human + JSON) shows a credential SOURCE label and NEVER an
//    sk-ant- secret.
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn doctor_reports_source_labels_without_secrets() {
    let env = DoctorEnv::new(&ant_config_with_account("work", "fake-admin-key-cmd"));
    env.install_fake_admin_key("fake-admin-key-cmd");

    // Human report.
    let (ok_h, human, err_h) = env.run(false, Some("work"));
    assert!(ok_h, "human doctor must exit 0; stderr={err_h}");
    assert!(
        human.contains("Credentials") && human.contains("admin_key_command (credential command)"),
        "human report must show a credential SOURCE label: {human}"
    );
    assert!(
        !human.contains("sk-ant-"),
        "human report must contain NO sk-ant- secret: {human}"
    );

    // JSON report.
    let (ok_j, jsonout, err_j) = env.run(true, Some("work"));
    assert!(ok_j, "json doctor must exit 0; stderr={err_j}");
    assert!(
        jsonout.contains("admin_key_command (credential command)"),
        "json report must show a credential SOURCE label: {jsonout}"
    );
    assert!(
        !jsonout.contains("sk-ant-"),
        "json report must contain NO sk-ant- secret: {jsonout}"
    );
}
