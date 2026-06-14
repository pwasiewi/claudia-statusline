//! Wave 0 integration-test scaffold for `statusline ant sync-usage` (Phase 08).
//!
//! Cloned from the `SyncEnv` harness in `ant_sync_cli_tests.rs` (renamed
//! `UsageEnv`) so 08-02 can fill in the assertions without re-deriving the
//! fake-exec / isolated-HOME / PATH-shaping plumbing. Per VALIDATION.md
//! §"Per-Task Verification Map", the Wave 0 integration test fns declared here
//! are `sync_usage_writes_after_both`, `degrades_401_403_differentiated`,
//! `per_account_no_bleed`, and `no_key_in_argv`. They are wired against the
//! harness but their assertions are `todo!("implemented in 08-02")` behind
//! `#[ignore]` so the file compiles and the suite stays green until 08-02.
//!
//! Like the models harness, all PATH/env/XDG-mutating tests are `#[serial]` and
//! assert on exit status / recorded files directly (no `cmd | tail` pipeline).

#![cfg(unix)]

mod test_support;

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serial_test::serial;
use tempfile::TempDir;

/// A fully isolated environment for one `ant sync-usage` invocation: an isolated
/// HOME + XDG dirs (so the per-account usage cache lands somewhere predictable
/// and never touches the host), a config file with a chosen `[ant]` section, and
/// a `bin` dir we control to shape PATH (fake `admin_key_command` + fake `curl`).
#[allow(dead_code)]
struct UsageEnv {
    home: TempDir,
    bin: TempDir,
    /// Where a fake exec records its argv/env (one line per invocation).
    record: PathBuf,
    config_path: PathBuf,
}

#[allow(dead_code)]
impl UsageEnv {
    fn new(ant_section: &str) -> Self {
        let home = TempDir::new().expect("home temp dir");
        let bin = TempDir::new().expect("bin temp dir");
        let record = home.path().join("record.log");

        let config_path = home.path().join("statusline.toml");
        let mut f = fs::File::create(&config_path).expect("write config");
        write!(f, "{}", ant_section).expect("write config body");

        UsageEnv {
            home,
            bin,
            record,
            config_path,
        }
    }

    /// PATH that contains ONLY our controlled bin dir (no real curl/ant, and no
    /// system tools — used for the "no fetch tool at all" failure branches).
    fn isolated_path(&self) -> String {
        self.bin.path().display().to_string()
    }

    /// PATH with our bin dir first, then a curated set of real system dirs so the
    /// fake shell scripts can still call `cat`/`env`/`touch`. We deliberately do
    /// NOT include any dir that contains a real `ant`, so the fetch resolves only
    /// a fake (or nothing) — keeping the test deterministic on machines that DO
    /// have `ant` installed.
    fn system_path(&self) -> String {
        let mut dirs = vec![self.bin.path().display().to_string()];
        for d in ["/bin", "/usr/bin"] {
            if !Path::new(d).join("ant").exists() {
                dirs.push(d.to_string());
            }
        }
        dirs.join(":")
    }

    /// Install a fake executable named `name` running the given `/bin/sh` body.
    fn install_fake(&self, name: &str, body: &str) {
        let exe = self.bin.path().join(name);
        let script = format!("#!/bin/sh\n{}\n", body);
        fs::write(&exe, script).expect("write fake exec");
        let mut perms = fs::metadata(&exe).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&exe, perms).expect("chmod +x");
    }

    /// Install a fake `admin_key_command` target: an executable that prints a
    /// canned org-admin key on stdout (so the sync can resolve a credential
    /// without a real keychain). 08-02 points the config's `admin_key_command`
    /// at this script.
    fn install_fake_admin_key(&self, name: &str, key: &str) {
        let body = format!("printf '%s' '{key}'\nexit 0\n");
        self.install_fake(name, &body);
    }

    /// Install a fake `curl` that records argv, slurps its stdin config to
    /// `stdin_capture`, and emits the given JSON body on stdout.
    fn install_fake_curl(&self, stdin_capture: &Path, json_body: &str) {
        let body = format!(
            "echo \"ARGV: $@\" >> \"{rec}\"\n\
             cat >> \"{cap}\"\n\
             echo '{json}'\n\
             exit 0\n",
            rec = self.record.display(),
            cap = stdin_capture.display(),
            json = json_body,
        );
        self.install_fake("curl", &body);
    }

    /// Run `statusline ant sync-usage [--quiet]` with the given PATH, optional
    /// `ANTHROPIC_API_KEY`, and optional `STATUSLINE_ANT_ACCOUNT`. Returns
    /// (status_success, stdout, stderr).
    fn run(
        &self,
        path: &str,
        key: Option<&str>,
        account: Option<&str>,
        quiet: bool,
    ) -> (bool, String, String) {
        let mut cmd = Command::new(test_support::test_binary());
        cmd.arg("ant").arg("sync-usage");
        if quiet {
            cmd.arg("--quiet");
        }
        cmd.env("HOME", self.home.path())
            .env("XDG_CACHE_HOME", self.home.path().join("cache"))
            .env("XDG_CONFIG_HOME", self.home.path().join("config"))
            .env("XDG_DATA_HOME", self.home.path().join("data"))
            .env("STATUSLINE_CONFIG_PATH", &self.config_path)
            .env("PATH", path)
            .env("NO_COLOR", "1")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("STATUSLINE_ANT_ACCOUNT")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(k) = key {
            cmd.env("ANTHROPIC_API_KEY", k);
        }
        if let Some(a) = account {
            cmd.env("STATUSLINE_ANT_ACCOUNT", a);
        }
        let out = cmd.output().expect("spawn statusline ant sync-usage");
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// Find a written per-account `usage/<account>.json` (if any) under HOME.
    fn find_usage_cache(&self) -> Option<PathBuf> {
        find_usage_json(self.home.path())
    }

    fn record_contents(&self) -> String {
        fs::read_to_string(&self.record).unwrap_or_default()
    }
}

/// Recursively search for any `*.json` under an `ant/usage/` directory below `root`.
fn find_usage_json(root: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(found) = find_usage_json(&p) {
                return Some(found);
            }
        } else if p.extension().map(|e| e == "json").unwrap_or(false)
            && p.parent()
                .and_then(|d| d.file_name())
                .map(|n| n == "usage")
                .unwrap_or(false)
        {
            return Some(p);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// STUB (08-02): both endpoints succeed -> the per-account usage cache is written
// (all-or-nothing two-endpoint gate). VALIDATION.md row `sync_usage_writes_after_both`.
// ---------------------------------------------------------------------------
#[test]
#[serial]
#[ignore = "wave 0 stub — 08-02"]
fn sync_usage_writes_after_both() {
    let _env = UsageEnv::new("[ant]\nenabled = true\n");
    todo!("implemented in 08-02");
}

// ---------------------------------------------------------------------------
// STUB (08-02): 401 (bad key) vs 403 (non-admin/unsupported account) produce
// DIFFERENTIATED, key-free messages + non-zero exit + no cache.
// VALIDATION.md row `degrades_401_403_differentiated`.
// ---------------------------------------------------------------------------
#[test]
#[serial]
#[ignore = "wave 0 stub — 08-02"]
fn degrades_401_403_differentiated() {
    let _env = UsageEnv::new("[ant]\nenabled = true\n");
    todo!("implemented in 08-02");
}

// ---------------------------------------------------------------------------
// STUB (08-02): syncing one account writes ONLY its own usage/<name>.json and
// never clobbers another account's slice (D-10 single-file write).
// VALIDATION.md row `per_account_no_bleed`.
// ---------------------------------------------------------------------------
#[test]
#[serial]
#[ignore = "wave 0 stub — 08-02"]
fn per_account_no_bleed() {
    let _env = UsageEnv::new("[ant]\nenabled = true\n");
    todo!("implemented in 08-02");
}

// ---------------------------------------------------------------------------
// STUB (08-02): the org-admin key never appears in curl argv (it arrives via the
// leak-free STDIN config). VALIDATION.md row `no_key_in_argv` / T-08-KEY.
// ---------------------------------------------------------------------------
#[test]
#[serial]
#[ignore = "wave 0 stub — 08-02"]
fn no_key_in_argv() {
    let _env = UsageEnv::new("[ant]\nenabled = true\n");
    todo!("implemented in 08-02");
}
