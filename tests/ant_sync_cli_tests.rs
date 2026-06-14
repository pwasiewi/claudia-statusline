//! CLI failure-mode + fake-ant/fake-curl integration tests for
//! `statusline ant sync-models` (Plan 03, Task 3).
//!
//! These tests prove the out-of-band sync's security and failure contract WITHOUT
//! any live network:
//!
//! - **Failure modes** exit non-zero with a clear DIFFERENTIATED, key-free stderr
//!   message and write NO cache (ANT-05 / D-13).
//! - **No key in argv** (T-07-08): a fake `ant` / `curl` records its real argv +
//!   env; the recorded argv never contains the key value.
//! - **Profile shadowing removed** (T-07-09 / D-09): the fake `ant` child env has
//!   `ANT_PROFILE` set and `ANTHROPIC_API_KEY` ABSENT on the profile path.
//! - **Pagination** (review MUST-FIX #6): a two-page fake response yields a cache
//!   with models from BOTH pages.
//! - **Leak-free curl** (review MUST-FIX #7): the fake `curl`'s argv lacks the key
//!   and the config (with `x-api-key`) arrives via STDIN.
//! - **`--quiet`** silences stdout on the failure branch.
//!
//! All PATH/env/XDG-mutating tests are `#[serial]` (the repo's global test lock,
//! review MUST-FIX #13) and use direct exit-status assertions (no
//! `cmd | tail; echo $?`).

#![cfg(unix)]

mod test_support;

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serial_test::serial;
use tempfile::TempDir;

/// A fully isolated environment for one `ant sync-models` invocation: an isolated
/// HOME + XDG dirs (so the cache lands somewhere predictable and never touches the
/// host), a config file with a chosen `[ant]` section, and a `bin` dir we control
/// to shape PATH.
struct SyncEnv {
    home: TempDir,
    bin: TempDir,
    /// Where a fake exec records its argv/env (one line per invocation).
    record: PathBuf,
    config_path: PathBuf,
}

impl SyncEnv {
    fn new(ant_section: &str) -> Self {
        let home = TempDir::new().expect("home temp dir");
        let bin = TempDir::new().expect("bin temp dir");
        let record = home.path().join("record.log");

        let config_path = home.path().join("statusline.toml");
        let mut f = fs::File::create(&config_path).expect("write config");
        write!(f, "{}", ant_section).expect("write config body");

        SyncEnv {
            home,
            bin,
            record,
            config_path,
        }
    }

    /// PATH that contains ONLY our controlled bin dir (no real ant/curl, and no
    /// system tools — used for the "no fetch tool at all" failure branches).
    fn isolated_path(&self) -> String {
        self.bin.path().display().to_string()
    }

    /// PATH with our bin dir first, then a curated set of real system dirs so the
    /// fake shell scripts can still call `cat`/`env`/`touch`. We deliberately do
    /// NOT include any dir that contains a real `ant`, so the fetch's
    /// `tool_on_path("ant")` resolves only a fake (or nothing) — keeping the test
    /// deterministic on machines that DO have `ant` installed.
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

    /// Run `statusline ant sync-models [--quiet]` with the given PATH and optional
    /// `ANTHROPIC_API_KEY`. Returns (status_success, stdout, stderr).
    fn run(&self, path: &str, key: Option<&str>, quiet: bool) -> (bool, String, String) {
        let mut cmd = Command::new(test_support::test_binary());
        cmd.arg("ant").arg("sync-models");
        if quiet {
            cmd.arg("--quiet");
        }
        cmd.env("HOME", self.home.path())
            // Isolate the cache + config search to this HOME on both platforms.
            .env("XDG_CACHE_HOME", self.home.path().join("cache"))
            .env("XDG_CONFIG_HOME", self.home.path().join("config"))
            .env("XDG_DATA_HOME", self.home.path().join("data"))
            .env("STATUSLINE_CONFIG_PATH", &self.config_path)
            .env("PATH", path)
            .env("NO_COLOR", "1")
            .env_remove("ANTHROPIC_API_KEY")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(k) = key {
            cmd.env("ANTHROPIC_API_KEY", k);
        }
        let out = cmd.output().expect("spawn statusline ant sync-models");
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// Find the written `models.json` (if any) under the isolated HOME tree.
    fn find_cache(&self) -> Option<PathBuf> {
        find_models_json(self.home.path())
    }

    fn record_contents(&self) -> String {
        fs::read_to_string(&self.record).unwrap_or_default()
    }
}

/// Recursively search for a `models.json` under `root`.
fn find_models_json(root: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(found) = find_models_json(&p) {
                return Some(found);
            }
        } else if p.file_name().map(|n| n == "models.json").unwrap_or(false) {
            return Some(p);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// (a) No credential + no ant: differentiated no-credential/no-tool, no cache.
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn no_credential_no_ant_fails_differentiated_no_cache() {
    let env = SyncEnv::new("[ant]\nenabled = true\nprofile = \"\"\n");
    // PATH has neither ant nor curl.
    let (ok, stdout, stderr) = env.run(&env.isolated_path(), None, false);
    assert!(!ok, "must exit non-zero with no credential and no ant");
    assert!(
        stderr.to_lowercase().contains("credential") || stderr.to_lowercase().contains("ant"),
        "stderr must carry a clear no-credential/no-tool message, got: {stderr}"
    );
    assert!(!stderr.contains("sk-ant-"), "no key token in stderr");
    assert!(
        env.find_cache().is_none(),
        "no cache must be written on failure"
    );
    let _ = stdout;
}

// ---------------------------------------------------------------------------
// (b) No ant and no curl, but a key set: fails cleanly (no panic), no cache.
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn no_ant_no_curl_with_key_fails_cleanly_no_cache() {
    let env = SyncEnv::new("[ant]\nenabled = true\nprofile = \"\"\n");
    let (ok, _stdout, stderr) = env.run(&env.isolated_path(), Some("sk-ant-dummy"), false);
    assert!(!ok, "must exit non-zero when no fetch tool is available");
    assert!(
        stderr.to_lowercase().contains("tool") || stderr.to_lowercase().contains("curl"),
        "stderr must explain no fetch tool is available, got: {stderr}"
    );
    assert!(!stderr.to_lowercase().contains("panic"), "must not panic");
    assert!(!stderr.contains("sk-ant-"), "no key token in stderr");
    assert!(env.find_cache().is_none(), "no cache on failure");
}

// ---------------------------------------------------------------------------
// (c) FAKE-ANT SUCCESS, profile mode (i): records real argv/env; assert no key
//     in argv, ANT_PROFILE set + ANTHROPIC_API_KEY removed, cache written, the
//     summary prints the profile label and no key token.
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn fake_ant_profile_mode_no_key_in_argv_profile_set_key_removed() {
    let env = SyncEnv::new("[ant]\nenabled = true\nprofile = \"work\"\n");
    // Fake `ant` records argv + env, then emits a single page and exits 0.
    let body = format!(
        "echo \"ARGV: $@\" >> \"{rec}\"\n\
         env >> \"{rec}\"\n\
         echo '{{\"data\":[{{\"id\":\"claude-x\",\"max_input_tokens\":200000}}],\"has_more\":false}}'\n\
         exit 0\n",
        rec = env.record.display()
    );
    env.install_fake("ant", &body);

    let (ok, stdout, stderr) = env.run(&env.system_path(), Some("sk-ant-shadow"), false);
    assert!(ok, "fake-ant success must exit 0; stderr={stderr}");

    let rec = env.record_contents();
    assert!(
        !rec.contains("sk-ant-shadow"),
        "key value must NOT be in argv/env: {rec}"
    );
    assert!(
        rec.contains("ANT_PROFILE=work"),
        "ANT_PROFILE must be set in child env"
    );
    assert!(
        !rec.lines().any(|l| l.starts_with("ANTHROPIC_API_KEY=")),
        "ANTHROPIC_API_KEY must be removed from the profile child env"
    );

    assert!(
        env.find_cache().is_some(),
        "cache must be written on success"
    );
    assert!(
        stdout.contains("ant profile 'work'"),
        "summary must show the profile label"
    );
    assert!(!stdout.contains("sk-ant-"), "no key token in summary");
}

// ---------------------------------------------------------------------------
// (c2) FAKE-ANT SUCCESS, env-key mode (ii): the env key is inherited (present in
//      the recorded child env) and the label is "ANTHROPIC_API_KEY (env)".
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn fake_ant_env_key_mode_inherits_key_and_labels_env() {
    let env = SyncEnv::new("[ant]\nenabled = true\nprofile = \"\"\n");
    let body = format!(
        "env >> \"{rec}\"\n\
         echo '{{\"data\":[{{\"id\":\"claude-y\",\"max_input_tokens\":100000}}],\"has_more\":false}}'\n\
         exit 0\n",
        rec = env.record.display()
    );
    env.install_fake("ant", &body);

    let (ok, stdout, stderr) = env.run(&env.system_path(), Some("sk-ant-envkey"), false);
    assert!(ok, "fake-ant env-key success must exit 0; stderr={stderr}");

    let rec = env.record_contents();
    assert!(
        rec.contains("ANTHROPIC_API_KEY=sk-ant-envkey"),
        "env-key mode must inherit the key into the child env: {rec}"
    );
    assert!(
        stdout.contains("ANTHROPIC_API_KEY (env)"),
        "summary must show the env-key label, got: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// (d) FAKE-ANT PAGINATION: page 1 has_more=true,last_id=X; page 2 has_more=false.
//     The written cache contains models from BOTH pages.
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn fake_ant_pagination_accumulates_both_pages() {
    let env = SyncEnv::new("[ant]\nenabled = true\nprofile = \"\"\n");
    let counter = env.home.path().join("count");
    // First invocation (no --after-id): emit page 1; second: page 2. We key off a
    // counter file so we don't depend on argv parsing in the shell.
    let body = format!(
        "if [ -f \"{cnt}\" ]; then\n\
         echo '{{\"data\":[{{\"id\":\"model-b\",\"max_input_tokens\":200}}],\"has_more\":false}}'\n\
         else\n\
         touch \"{cnt}\"\n\
         echo '{{\"data\":[{{\"id\":\"model-a\",\"max_input_tokens\":100}}],\"has_more\":true,\"last_id\":\"model-a\"}}'\n\
         fi\n\
         exit 0\n",
        cnt = counter.display()
    );
    env.install_fake("ant", &body);

    let (ok, _stdout, stderr) = env.run(&env.system_path(), Some("sk-ant-x"), false);
    assert!(ok, "paginated fake-ant must exit 0; stderr={stderr}");

    let cache_path = env.find_cache().expect("cache must be written");
    let body = fs::read_to_string(&cache_path).expect("read cache");
    assert!(
        body.contains("model-a"),
        "page 1 model must be present: {body}"
    );
    assert!(
        body.contains("model-b"),
        "page 2 model must be present: {body}"
    );
}

// ---------------------------------------------------------------------------
// (e) FAKE-CURL fallback (mode ii): NO ant on PATH, a fake curl records argv +
//     reads its stdin config and emits JSON. Assert no key in curl argv, the
//     config (x-api-key) arrived via stdin, the cache is written, and no key
//     token leaks to output.
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn fake_curl_fallback_leakfree_stdin_config() {
    let env = SyncEnv::new("[ant]\nenabled = true\nprofile = \"\"\n");
    let stdin_capture = env.home.path().join("curl_stdin.log");
    // Fake `curl`: record argv, slurp stdin (the config) to a file, emit JSON.
    let body = format!(
        "echo \"ARGV: $@\" >> \"{rec}\"\n\
         cat >> \"{cap}\"\n\
         echo '{{\"data\":[{{\"id\":\"curl-model\",\"max_input_tokens\":150000}}],\"has_more\":false}}'\n\
         exit 0\n",
        rec = env.record.display(),
        cap = stdin_capture.display()
    );
    env.install_fake("curl", &body);
    // NOTE: no fake `ant` installed and isolated PATH excludes the real ant, so
    // the EnvKey-mode fetch must fall back to curl.

    let (ok, stdout, stderr) = env.run(&env.system_path(), Some("sk-ant-curlkey"), false);
    assert!(ok, "fake-curl fallback must exit 0; stderr={stderr}");

    let rec = env.record_contents();
    assert!(
        rec.starts_with("ARGV:"),
        "curl must have been invoked: {rec}"
    );
    assert!(
        !rec.contains("sk-ant-curlkey"),
        "key must NOT be in curl argv: {rec}"
    );

    let stdin_cfg = fs::read_to_string(&stdin_capture).expect("read curl stdin capture");
    assert!(
        stdin_cfg.contains("x-api-key: sk-ant-curlkey"),
        "the x-api-key config must arrive via curl STDIN: {stdin_cfg}"
    );

    assert!(
        env.find_cache().is_some(),
        "cache must be written via curl fallback"
    );
    assert!(!stdout.contains("sk-ant-"), "no key token in summary");
    assert!(!stderr.contains("sk-ant-"), "no key token in stderr");
}

// ---------------------------------------------------------------------------
// (f) --quiet suppresses the summary on the failure branch (stdout empty, stderr
//     still carries the differentiated error).
// ---------------------------------------------------------------------------
#[test]
#[serial]
fn quiet_suppresses_stdout_on_failure_branch() {
    let env = SyncEnv::new("[ant]\nenabled = true\nprofile = \"\"\n");
    let (ok, stdout, stderr) = env.run(&env.isolated_path(), None, true);
    assert!(!ok, "failure branch must exit non-zero");
    assert!(
        stdout.is_empty(),
        "--quiet must produce empty stdout, got: {stdout}"
    );
    assert!(!stderr.is_empty(), "stderr must still carry the error");
}
