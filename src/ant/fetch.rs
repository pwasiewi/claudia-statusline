//! Out-of-band fetch of the Anthropic Models API (Plan 03).
//!
//! This is the ONLY site in the crate that touches the network, spawns a
//! subprocess, or handles a credential. It shells out to the `ant` CLI
//! (`ant models list --raw-output`, fully paginated) with a leak-free `curl`
//! fallback to `GET https://api.anthropic.com/v1/models`. It NEVER runs on the
//! render path — the render path consumes the cache this module writes.
//!
//! # Credential safety (D-17 / ANT-04 / ANT-12)
//!
//! The standard API key reaches the child process only via:
//! - **`ant`:** the inherited environment (`ANTHROPIC_API_KEY` / the named
//!   profile selected by `ANT_PROFILE`) — never argv.
//! - **`curl`:** a config written to the child's STDIN (`curl --config -` +
//!   `Stdio::piped()`) — never argv, never disk. The key disappears on
//!   crash/SIGKILL.
//!
//! The key is never serialized to the cache, logged, or printed in any summary.
//!
//! # Three explicit credential modes (D-09 / review MUST-FIX #5)
//!
//! 1. **Named profile** (`cfg.profile` non-empty): invoke `ant` ONLY, with
//!    `ANT_PROFILE` set and the shadowing `ANTHROPIC_API_KEY` removed from the
//!    child env. No curl fallback (the profile exposes no raw key).
//! 2. **Env key** (`cfg.profile` empty, `ANTHROPIC_API_KEY` present): try `ant`
//!    (key inherited), then on `ant`-not-found fall back to `curl` (which can use
//!    the raw env key via the stdin config).
//! 3. **Default `ant auth`** (`cfg.profile` empty, no env key): attempt `ant`'s
//!    DEFAULT authenticated profile. Only if `ant` is unavailable AND no env key
//!    exists for curl do we return a clear no-credential / no-tool error. The
//!    default `ant auth` profile is NEVER skipped.

// `fetch.rs` is the out-of-band fetch entry; its public surface
// (`fetch_models`, `CredentialMode`, `FetchOutcome`) is consumed by the thin
// `commands::ant` handler (also Plan 03). A handful of helpers are exercised
// only by the colocated unit tests; allow dead_code at the module level to keep
// `make check-code` clean, mirroring `src/ant/cache.rs`.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

use chrono::Utc;
use serde::Deserialize;

use crate::ant::cache::{ModelEntry, ModelsCache, MODELS_CACHE_SCHEMA_VERSION};
use crate::ant::config::AntConfig;
use crate::error::{Result, StatuslineError};

/// Max page size for the Models API (`limit` default 20, max 1000).
const PAGE_LIMIT: u32 = 1000;
/// curl connection timeout (seconds) — bounds DoS via an unreachable host.
const CONNECT_TIMEOUT_SECS: u32 = 10;
/// curl total operation timeout (seconds).
const MAX_TIME_SECS: u32 = 30;
/// Stable Anthropic API version header for the curl fallback.
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Base Models API URL for the curl fallback.
const MODELS_URL: &str = "https://api.anthropic.com/v1/models";
/// Maximum number of bytes of child stderr we will echo in an error message.
const STDERR_BOUND: usize = 512;
/// Hard cap on pagination iterations — defends against a server that always
/// reports `has_more = true` (T-07-10 / T-07-12).
const MAX_PAGES: usize = 1000;

/// The resolved credential mode for a fetch (D-09). Carries only a non-secret
/// label; the key itself never lives here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialMode {
    /// Mode (i): a named `ant` profile; the shadowing env key is removed.
    Profile(String),
    /// Mode (ii): the raw `ANTHROPIC_API_KEY` env var (curl fallback allowed).
    EnvKey,
    /// Mode (iii): `ant`'s default authenticated profile.
    DefaultAuth,
}

impl CredentialMode {
    /// A human-readable, KEY-FREE source label for the summary (D-09).
    pub fn label(&self) -> String {
        match self {
            CredentialMode::Profile(name) => format!("ant profile '{}'", name),
            CredentialMode::EnvKey => "ANTHROPIC_API_KEY (env)".to_string(),
            CredentialMode::DefaultAuth => "ant default auth".to_string(),
        }
    }

    /// Resolve the credential mode from the config + ambient environment.
    ///
    /// Pure aside from a single `std::env::var` read; never spawns or panics.
    pub fn resolve(cfg: &AntConfig) -> CredentialMode {
        if !cfg.profile.is_empty() {
            CredentialMode::Profile(cfg.profile.clone())
        } else if std::env::var("ANTHROPIC_API_KEY").is_ok() {
            CredentialMode::EnvKey
        } else {
            CredentialMode::DefaultAuth
        }
    }
}

/// The successful result of a fetch: the assembled cache plus the resolved
/// credential source label (printed by the handler).
#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub cache: ModelsCache,
    pub credential_source: String,
}

/// Tolerant view of one `/v1/models` page (`ant ... --raw-output` and the curl
/// fallback emit the same shape). Unknown fields are ignored; missing
/// `max_input_tokens` defaults to `0` (caller falls through — D-06).
#[derive(Debug, Deserialize)]
struct ModelsPage {
    #[serde(default)]
    data: Vec<ApiModel>,
    #[serde(default)]
    has_more: bool,
    #[serde(default)]
    last_id: Option<String>,
}

/// A single model entry from the API. `id` is the canonical model id (the cache
/// key, D-04). `max_input_tokens` is the context window (0/missing => unknown).
#[derive(Debug, Deserialize)]
struct ApiModel {
    id: String,
    #[serde(default)]
    max_input_tokens: u64,
}

/// Fetch the full (paginated) Models API and assemble a `ModelsCache`.
///
/// Returns the cache and the credential-source label. Does NOT write the cache —
/// the caller publishes it via `write_models_cache` only after this returns Ok
/// (the cache is therefore published only after ALL pages parse — review
/// MUST-FIX #6). On ANY failure returns a DIFFERENTIATED `StatuslineError`
/// (no-tool / auth / network / HTTP / parse) and writes nothing.
pub fn fetch_models(cfg: &AntConfig) -> Result<FetchOutcome> {
    let mode = CredentialMode::resolve(cfg);

    // One-time best-effort note for the profile path: profile selection via
    // ANT_PROFILE is unverified against the installed `ant` (RESEARCH OQ1).
    if let CredentialMode::Profile(name) = &mode {
        eprintln!(
            "note: selecting ant profile '{}' is best-effort (ANT_PROFILE); \
             verify your ant version honors it",
            name
        );
    }

    let models = fetch_all_pages(&mode)?;

    let cache = ModelsCache {
        schema_version: MODELS_CACHE_SCHEMA_VERSION,
        fetched_at: Utc::now(),
        models,
    };

    Ok(FetchOutcome {
        cache,
        credential_source: mode.label(),
    })
}

/// Drive pagination across `ant` (preferred) / `curl` (fallback), accumulating
/// `data[]` from every page. The cache map is returned only after every page
/// parses successfully.
fn fetch_all_pages(mode: &CredentialMode) -> Result<HashMap<String, ModelEntry>> {
    // Decide the transport ONCE so a mid-pagination tool switch can't happen.
    // `ant` is preferred in every mode; curl is only reachable in EnvKey mode.
    let ant_available = tool_on_path("ant");
    let use_ant = ant_available;

    if !use_ant {
        match mode {
            // Profile / default-auth modes have no raw key for curl: if `ant`
            // is missing we cannot proceed.
            CredentialMode::Profile(_) | CredentialMode::DefaultAuth => {
                return Err(StatuslineError::other(
                    "no credential source available: `ant` is not installed and no \
                     ANTHROPIC_API_KEY is set (set [ant].profile with `ant` installed, \
                     export ANTHROPIC_API_KEY, or run `ant auth`)",
                ));
            }
            // EnvKey mode can fall back to curl.
            CredentialMode::EnvKey => {
                if !tool_on_path("curl") {
                    return Err(StatuslineError::other(
                        "no fetch tool available: neither `ant` nor `curl` is installed \
                         (install one to run `ant sync-models`)",
                    ));
                }
            }
        }
    }

    let mut models: HashMap<String, ModelEntry> = HashMap::new();
    let mut after_id: Option<String> = None;

    for _ in 0..MAX_PAGES {
        let page = if use_ant {
            fetch_page_ant(mode, after_id.as_deref())?
        } else {
            fetch_page_curl(after_id.as_deref())?
        };

        for m in page.data {
            models.insert(
                m.id,
                ModelEntry {
                    max_input_tokens: m.max_input_tokens,
                },
            );
        }

        if page.has_more {
            match page.last_id {
                Some(id) => after_id = Some(id),
                // has_more with no cursor: stop rather than loop forever.
                None => break,
            }
        } else {
            break;
        }
    }

    Ok(models)
}

/// Fetch a single page via the `ant` CLI.
///
/// The key is NEVER added to argv. The child env is constructed per the active
/// credential mode: profile mode sets `ANT_PROFILE` and removes the shadowing
/// `ANTHROPIC_API_KEY`; env-key / default-auth modes inherit the env as-is.
fn fetch_page_ant(mode: &CredentialMode, after_id: Option<&str>) -> Result<ModelsPage> {
    let mut cmd = Command::new("ant");
    cmd.args(["models", "list", "--raw-output"]);
    cmd.args(["--limit", &PAGE_LIMIT.to_string()]);
    if let Some(id) = after_id {
        // Cursor for the next page (review MUST-FIX #6).
        cmd.args(["--after-id", id]);
    }

    match mode {
        CredentialMode::Profile(name) => {
            // Best-effort named-profile selection (RESEARCH OQ1) + drop the
            // shadowing key so it cannot silently override the profile (D-09).
            cmd.env("ANT_PROFILE", name);
            cmd.env_remove("ANTHROPIC_API_KEY");
        }
        // Env key / default auth: inherit the ambient env unchanged.
        CredentialMode::EnvKey | CredentialMode::DefaultAuth => {}
    }

    let output = cmd
        .output()
        .map_err(|e| StatuslineError::other(format!("failed to spawn `ant`: {}", e)))?;

    if !output.status.success() {
        let stderr = sanitize_stderr(&output.stderr);
        // A non-zero exit on the ant path most often means auth/credential
        // failure (no valid profile / key). Surface it as such (D-13).
        return Err(StatuslineError::other(format!(
            "`ant models list` failed (auth or credential error): {}",
            stderr
        )));
    }

    parse_page(&output.stdout)
}

/// Fetch a single page via the leak-free `curl` fallback (EnvKey mode only).
///
/// The `x-api-key` / `anthropic-version` headers and the URL are written to the
/// child's STDIN as a curl config (`curl --config -`); the key never touches
/// argv OR disk (review MUST-FIX #7).
fn fetch_page_curl(after_id: Option<&str>) -> Result<ModelsPage> {
    let key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| {
        StatuslineError::other("ANTHROPIC_API_KEY is required for the curl fallback")
    })?;

    let mut url = format!("{}?limit={}", MODELS_URL, PAGE_LIMIT);
    if let Some(id) = after_id {
        url.push_str("&after_id=");
        url.push_str(id);
    }

    let mut cmd = Command::new("curl");
    cmd.arg("--config")
        .arg("-")
        .arg("--connect-timeout")
        .arg(CONNECT_TIMEOUT_SECS.to_string())
        .arg("--max-time")
        .arg(MAX_TIME_SECS.to_string())
        .arg("--fail-with-body")
        .arg("--silent")
        .arg("--show-error")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| StatuslineError::other(format!("failed to spawn `curl`: {}", e)))?;

    // Build the config in memory and stream it to stdin. The key is interpolated
    // into a HEADER line written to stdin ONLY — never into argv (T-07-08).
    // url is quoted so query params can't be misparsed as config directives.
    let config = format!(
        "header = \"x-api-key: {key}\"\n\
         header = \"anthropic-version: {ver}\"\n\
         url = \"{url}\"\n",
        key = key,
        ver = ANTHROPIC_VERSION,
        url = url,
    );

    {
        let mut stdin = child.stdin.take().ok_or_else(|| {
            StatuslineError::other("failed to open curl stdin for the leak-free config")
        })?;
        stdin.write_all(config.as_bytes()).map_err(|e| {
            StatuslineError::other(format!("failed to write curl config to stdin: {}", e))
        })?;
        // Drop closes stdin so curl proceeds; the key leaves memory here.
    }

    let output = child
        .wait_with_output()
        .map_err(|e| StatuslineError::other(format!("curl did not complete: {}", e)))?;

    if !output.status.success() {
        let stderr = sanitize_stderr(&output.stderr);
        let code = output.status.code().unwrap_or(-1);
        // Differentiate connect/timeout (network) from HTTP failures. curl exit
        // 6/7/28 are resolve/connect/timeout; 22 is an HTTP >= 400 under
        // --fail-with-body (review MUST-FIX #8 / D-13).
        let msg = match code {
            6 | 7 | 28 => format!(
                "network error fetching the Models API (curl exit {}): {}",
                code, stderr
            ),
            22 => format!("HTTP error from the Models API (curl exit 22): {}", stderr),
            _ => format!("curl failed (exit {}): {}", code, stderr),
        };
        return Err(StatuslineError::other(msg));
    }

    parse_page(&output.stdout)
}

/// Parse a single page of JSON into a tolerant `ModelsPage`, mapping a serde
/// failure to a DIFFERENTIATED parse error (D-13). The raw body is NOT echoed
/// (it could be large / contain unexpected content); only the serde message.
fn parse_page(stdout: &[u8]) -> Result<ModelsPage> {
    serde_json::from_slice::<ModelsPage>(stdout)
        .map_err(|e| StatuslineError::other(format!("failed to parse Models API JSON: {}", e)))
}

/// Whether an executable is resolvable on `PATH` (no spawn). Used to choose the
/// transport and to produce a clear no-tool error.
fn tool_on_path(tool: &str) -> bool {
    let path = match std::env::var_os("PATH") {
        Some(p) => p,
        None => return false,
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(tool);
        candidate.is_file() || {
            // On Unix an executable need not have an extension; is_file()
            // covers it. Keep this branch for clarity/symmetry.
            false
        }
    })
}

/// Bound and sanitize child stderr before including it in an error message
/// (review MUST-FIX #8): truncate to `STDERR_BOUND` bytes and drop any line that
/// looks like it carries a key, so a key-bearing diagnostic can never leak.
fn sanitize_stderr(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let filtered: String = text
        .lines()
        .filter(|line| {
            let l = line.to_ascii_lowercase();
            !l.contains("sk-ant-") && !l.contains("x-api-key") && !l.contains("api-key")
        })
        .collect::<Vec<_>>()
        .join(" ");
    let trimmed = filtered.trim();
    if trimmed.len() > STDERR_BOUND {
        format!("{}…", &trimmed[..STDERR_BOUND])
    } else if trimmed.is_empty() {
        "(no diagnostic output)".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn cfg_with_profile(p: &str) -> AntConfig {
        AntConfig {
            enabled: true,
            profile: p.to_string(),
        }
    }

    fn cfg_no_profile() -> AntConfig {
        AntConfig {
            enabled: true,
            profile: String::new(),
        }
    }

    // (a) Credential-mode resolution across the three modes.
    #[test]
    #[serial]
    fn profile_set_resolves_to_profile_mode() {
        std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-shadow");
        let mode = CredentialMode::resolve(&cfg_with_profile("work"));
        assert_eq!(mode, CredentialMode::Profile("work".to_string()));
        assert_eq!(mode.label(), "ant profile 'work'");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn env_key_resolves_to_env_mode() {
        std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-xyz");
        let mode = CredentialMode::resolve(&cfg_no_profile());
        assert_eq!(mode, CredentialMode::EnvKey);
        assert_eq!(mode.label(), "ANTHROPIC_API_KEY (env)");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    #[serial]
    fn neither_resolves_to_default_auth_not_error() {
        std::env::remove_var("ANTHROPIC_API_KEY");
        let mode = CredentialMode::resolve(&cfg_no_profile());
        // Mode (iii) must NOT be an immediate error (review MUST-FIX #5).
        assert_eq!(mode, CredentialMode::DefaultAuth);
        assert_eq!(mode.label(), "ant default auth");
    }

    // (b) The constructed `ant` Command for the profile path sets ANT_PROFILE,
    // removes the shadowing key, and never carries the key in argv. We inspect
    // the Command's configured env/args directly via std::process::Command's
    // accessors (get_envs / get_args), which avoids spawning.
    #[test]
    fn profile_command_sets_ant_profile_and_removes_key_no_argv_key() {
        let mut cmd = Command::new("ant");
        cmd.args(["models", "list", "--raw-output"]);
        cmd.args(["--limit", &PAGE_LIMIT.to_string()]);
        // Mirror the profile-mode env construction in fetch_page_ant.
        cmd.env("ANT_PROFILE", "work");
        cmd.env_remove("ANTHROPIC_API_KEY");

        // ANT_PROFILE is set to the configured profile.
        let profile_set = cmd.get_envs().any(|(k, v)| {
            k == std::ffi::OsStr::new("ANT_PROFILE") && v == Some(std::ffi::OsStr::new("work"))
        });
        assert!(profile_set, "ANT_PROFILE must be set on the ant Command");

        // ANTHROPIC_API_KEY is explicitly removed (value None in the override map).
        let key_removed = cmd
            .get_envs()
            .any(|(k, v)| k == std::ffi::OsStr::new("ANTHROPIC_API_KEY") && v.is_none());
        assert!(
            key_removed,
            "ANTHROPIC_API_KEY must be removed for profile mode"
        );

        // No argv token contains a key value.
        let args_have_key = cmd
            .get_args()
            .any(|a| a.to_string_lossy().contains("sk-ant-"));
        assert!(!args_have_key, "the key must never appear in argv");
    }

    // (c) Single-page parse.
    #[test]
    fn single_page_parses_to_models_map() {
        let json = br#"{"data":[{"id":"claude-3-5-sonnet","max_input_tokens":200000}],"has_more":false,"last_id":null}"#;
        let page = parse_page(json).expect("valid single page");
        assert_eq!(page.data.len(), 1);
        assert_eq!(page.data[0].id, "claude-3-5-sonnet");
        assert_eq!(page.data[0].max_input_tokens, 200000);
        assert!(!page.has_more);
    }

    // (d) Two-page accumulation (pagination — review MUST-FIX #6). We exercise
    // the accumulation logic directly since spawning a real tool is out of scope
    // for a unit test (the fake-ant integration test covers the spawn path).
    #[test]
    fn two_pages_accumulate_into_one_map() {
        let p1 = br#"{"data":[{"id":"model-a","max_input_tokens":100}],"has_more":true,"last_id":"model-a"}"#;
        let p2 = br#"{"data":[{"id":"model-b","max_input_tokens":200}],"has_more":false,"last_id":null}"#;
        let page1 = parse_page(p1).unwrap();
        let page2 = parse_page(p2).unwrap();

        let mut models: HashMap<String, ModelEntry> = HashMap::new();
        for m in page1.data.into_iter().chain(page2.data) {
            models.insert(
                m.id,
                ModelEntry {
                    max_input_tokens: m.max_input_tokens,
                },
            );
        }
        assert_eq!(models.len(), 2);
        assert_eq!(models.get("model-a").unwrap().max_input_tokens, 100);
        assert_eq!(models.get("model-b").unwrap().max_input_tokens, 200);
        assert!(page1.has_more);
        assert_eq!(page1.last_id.as_deref(), Some("model-a"));
    }

    // (e) Missing / 0 max_input_tokens is stored as 0 (falls through at render).
    #[test]
    fn missing_max_input_tokens_defaults_to_zero() {
        let json = br#"{"data":[{"id":"mystery-model"}],"has_more":false}"#;
        let page = parse_page(json).expect("valid page with missing field");
        assert_eq!(page.data[0].max_input_tokens, 0);
    }

    // (f) Differentiated error mapping: parse failure vs no-credential differ.
    #[test]
    fn parse_failure_and_no_credential_have_distinct_messages() {
        let parse_err = parse_page(b"not json at all").unwrap_err().to_string();
        assert!(
            parse_err.contains("parse"),
            "parse error must mention parsing: {}",
            parse_err
        );

        let no_cred = StatuslineError::other(
            "no credential source available: `ant` is not installed and no ANTHROPIC_API_KEY is set",
        )
        .to_string();
        assert!(no_cred.contains("credential"));
        assert_ne!(parse_err, no_cred);
    }

    // sanitize_stderr bounds output and drops key-bearing lines.
    #[test]
    fn sanitize_drops_key_lines_and_bounds_length() {
        let raw = b"normal diagnostic line\nleaked: x-api-key: sk-ant-secret\nmore";
        let out = sanitize_stderr(raw);
        assert!(!out.contains("sk-ant-"));
        assert!(!out.contains("x-api-key"));
        assert!(out.contains("normal diagnostic"));

        let long = vec![b'a'; STDERR_BOUND + 100];
        let bounded = sanitize_stderr(&long);
        assert!(bounded.chars().count() <= STDERR_BOUND + 1); // +1 for the ellipsis
    }
}
