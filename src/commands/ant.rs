//! `ant` subcommand handlers (out-of-band Anthropic enrichment).
//!
//! Thin binary-only handlers for `statusline ant sync-models`. All reusable
//! logic (the fetch transport, credential modes, pagination, parsing) lives in
//! `statusline::ant::fetch`; the cache IO lives in `statusline::ant::cache`.
//! This handler only orchestrates: load config -> fetch -> write cache ->
//! print a credential-safe summary, emitting the D-03 divergence note on the
//! SYNC PATH ONLY (never the render path).

use std::collections::HashMap;

use crate::ant::cache::{
    models_cache_path, usage_cache_path, write_models_cache, write_usage_cache, ModelsCache,
};
use crate::config::Config;
use crate::error::{Result, StatuslineError};

/// Dispatch entry for `statusline ant <action>`.
pub(crate) fn handle_ant_command(action: crate::AntAction) -> Result<()> {
    match action {
        crate::AntAction::SyncModels { quiet, max_age } => sync_models(quiet, max_age),
        crate::AntAction::SyncUsage {
            quiet,
            account,
            max_age,
        } => sync_usage(quiet, account, max_age),
        crate::AntAction::Doctor { json, probe } => doctor(json, probe),
    }
}

/// `ant doctor`: a passive, secret-safe diagnostic of the ant enrichment state.
///
/// Reports (D-13/D-14/D-15/D-16): whether the `ant` CLI is on `PATH`; the `[ant]`
/// config (enabled / profile / configured accounts); per-cache presence, freshness
/// (humanized age), staleness, and on-disk paths; the credential SOURCE LABELS for
/// the models and usage paths (NEVER the key, and — critically — NEVER executing the
/// credential command in passive mode); which enrichment is currently active given
/// `[ant].enabled` + `STATUSLINE_ANT_ACCOUNT`; and a security self-audit reusing the
/// shared `scan_artifacts_for_keys` leak scanner. `--json` mirrors `health --json`'s
/// schema; the human default mirrors its section layout. `--probe` (opt-in) is the
/// ONLY new credential/network site (Task 2 fills it).
///
/// This handler is deliberately THIN: all scan logic lives in `crate::ant::audit`,
/// all parse/humanize logic in `crate::ant::duration`, and all label logic in
/// `crate::ant::fetch::CredentialMode` — mirroring the module-doc split above.
fn doctor(json_output: bool, probe: bool) -> Result<()> {
    // Stub: filled by Task 2. Keep the signature so the crate compiles and the
    // dispatch arm + clap variant are exercisable now.
    let _ = (json_output, probe);
    Ok(())
}

/// `ant sync-models`: fetch the Models API out-of-band and publish the cache.
///
/// On success writes the versioned cache (only after all pages parse — the
/// fetch layer guarantees this), emits the D-03 one-time stderr divergence note
/// for any cached window that differs from a winning user override, and (unless
/// `quiet`) prints a short, KEY-FREE summary. On failure the error propagates to
/// `main`, which exits non-zero with a differentiated stderr message (D-13).
fn sync_models(quiet: bool, max_age: Option<String>) -> Result<()> {
    // ANT-30 / D-02: self-throttle BEFORE any network/subprocess/credential work.
    // When `--max-age` is set and the cached `fetched_at` is younger than the
    // threshold, skip the fetch entirely and exit 0. Omitting `--max-age` always
    // fetches (manual runs are never throttled). A future-dated cache (negative
    // age / clock skew) is treated as fresh (Pitfall 1). The total `read_models_cache`
    // collapses every error to `None`, so a missing/corrupt cache simply falls
    // through and fetches.
    if let Some(spec) = max_age {
        let max = crate::ant::duration::parse_max_age(&spec)?;
        if let Some(cache) = crate::ant::cache::read_models_cache() {
            let age = cache.age().to_std().unwrap_or(std::time::Duration::ZERO);
            if age < max {
                if !quiet {
                    println!("Models cache is fresh (within {spec}); skipping fetch.");
                }
                return Ok(());
            }
        }
    }

    let config = Config::load()?;

    // The ONLY network/subprocess/credential touchpoint in the crate.
    let outcome = crate::ant::fetch::fetch_models(&config.ant)?;

    // Publish the cache (fetch already assembled all pages).
    write_models_cache(&outcome.cache)?;

    // D-03: one-time stderr divergence note, SYNC PATH ONLY. This never touches
    // the render path / get_context_window_for_model. The note carries no secret.
    for (id, cached, override_val) in divergences(&outcome.cache, &config.context.model_windows) {
        eprintln!(
            "note: cached context window for {id} ({cached}) differs from your \
             [context.model_windows] override ({override_val}); the override wins at render time"
        );
    }

    if !quiet {
        let count = outcome.cache.models.len();
        let path = models_cache_path()?;
        println!(
            "Synced {} model{} from the Models API.",
            count,
            if count == 1 { "" } else { "s" }
        );
        println!("Cache: {}", path.display());
        // Credential SOURCE label only — never the key (D-09 / D-17).
        println!("Credential source: {}", outcome.credential_source);
    }

    Ok(())
}

/// `ant sync-usage`: fetch the org usage & cost Admin endpoints out-of-band and
/// publish the ACTIVE account's per-account slice.
///
/// The active account is resolved from `--account <name>` else the
/// `STATUSLINE_ANT_ACCOUNT` env var (clear error if neither is set — D-18). Its
/// `admin_key_command` argv is looked up in `[ant.accounts.<name>]`; if the
/// account/table is absent OR the command is empty, a clear feature-absent error
/// is returned and the process exits non-zero (D-02 — never a silent downgrade).
/// On success the slice is written ONLY after BOTH endpoints succeed + parse
/// (`fetch_usage` guarantees this — D-16), and (unless `quiet`) a KEY-FREE
/// summary is printed (account, cache path, a credential-source LABEL, today/MTD
/// USD totals + the `UTC` tz label). On any failure the differentiated error
/// (401/403 taxonomy — D-17) propagates to `main` for a non-zero exit.
fn sync_usage(
    quiet: bool,
    account_override: Option<String>,
    max_age: Option<String>,
) -> Result<()> {
    let config = Config::load()?;

    // Resolve the active account: --account override else STATUSLINE_ANT_ACCOUNT.
    let account = account_override
        .or_else(|| std::env::var("STATUSLINE_ANT_ACCOUNT").ok())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            StatuslineError::other(
                "no active ant account: pass --account <name> or set STATUSLINE_ANT_ACCOUNT",
            )
        })?;

    // ANT-30 / D-02: self-throttle AFTER the active account is resolved but BEFORE
    // resolving/exec'ing the credential command or any Admin fetch. When `--max-age`
    // is set and the active account's cached `fetched_at` is younger than the
    // threshold, skip the fetch and exit 0. Omitting `--max-age` always fetches. A
    // future-dated cache (negative age) is treated as fresh (Pitfall 1).
    if let Some(spec) = &max_age {
        let max = crate::ant::duration::parse_max_age(spec)?;
        if let Some(cache) = crate::ant::cache::read_usage_cache(&account) {
            let age = cache.age().to_std().unwrap_or(std::time::Duration::ZERO);
            if age < max {
                if !quiet {
                    println!(
                        "Usage cache for ant account '{account}' is fresh (within {spec}); skipping fetch."
                    );
                }
                return Ok(());
            }
        }
    }

    // Look up the account's admin_key_command. Absent account / empty command =>
    // feature-absent (D-02): a clear error + non-zero exit, never a silent skip.
    let admin_key_command = match config.ant.accounts.get(&account) {
        Some(acct) if !acct.admin_key_command.is_empty() => acct.admin_key_command.clone(),
        Some(_) => {
            return Err(StatuslineError::other(format!(
                "ant account '{account}' has no admin_key_command configured \
                 (add [ant.accounts.{account}].admin_key_command to enable usage sync)"
            )));
        }
        None => {
            return Err(StatuslineError::other(format!(
                "ant account '{account}' is not configured \
                 (add an [ant.accounts.{account}] table with admin_key_command)"
            )));
        }
    };

    // The ONLY network/subprocess/credential touchpoint for the usage path. The
    // key is resolved + handed to curl via stdin inside fetch_usage; it never
    // appears here, in the summary, or in any error.
    let slice = crate::ant::usage::fetch_usage(&account, &admin_key_command)?;

    // Publish the per-account slice (fetch already gated on both endpoints — D-16).
    write_usage_cache(&slice)?;

    if !quiet {
        let path = usage_cache_path(&account)?;
        println!("Synced usage & cost for ant account '{}'.", slice.account);
        println!("Cache: {}", path.display());
        // Credential SOURCE label only — never the key, never the raw argv (D-09/D-17).
        println!("Credential source: admin_key_command (credential command)");
        println!(
            "Today: ${:.2} {tz}   MTD: ${:.2} {tz}",
            slice.today_usd,
            slice.mtd_usd,
            tz = slice.tz
        );
    }

    Ok(())
}

/// Pure, testable D-03 comparison: for each model id present in BOTH the cache
/// and the user's `[context.model_windows]` overrides, return a divergence tuple
/// `(id, cached_max_input_tokens, override)` when the cached value is `> 0` AND
/// differs from the override. A cached `0` is NOT a divergence (it means
/// "unknown" and falls through — D-06), and an id with no override is skipped.
fn divergences(
    cached: &ModelsCache,
    overrides: &HashMap<String, usize>,
) -> Vec<(String, u64, usize)> {
    let mut out = Vec::new();
    for (id, entry) in &cached.models {
        if let Some(&override_val) = overrides.get(id) {
            let cached_val = entry.max_input_tokens;
            if cached_val > 0 && cached_val != override_val as u64 {
                out.push((id.clone(), cached_val, override_val));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ant::cache::{ModelEntry, MODELS_CACHE_SCHEMA_VERSION};
    use chrono::Utc;

    fn cache_with(models: &[(&str, u64)]) -> ModelsCache {
        let mut map = HashMap::new();
        for (id, tokens) in models {
            map.insert(
                id.to_string(),
                ModelEntry {
                    max_input_tokens: *tokens,
                },
            );
        }
        ModelsCache {
            schema_version: MODELS_CACHE_SCHEMA_VERSION,
            fetched_at: Utc::now(),
            models: map,
        }
    }

    fn overrides(pairs: &[(&str, usize)]) -> HashMap<String, usize> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    // (i) A differing override IS a divergence.
    #[test]
    fn differing_override_is_a_divergence() {
        let cache = cache_with(&[("claude-x", 200_000)]);
        let ov = overrides(&[("claude-x", 100_000)]);
        let div = divergences(&cache, &ov);
        assert_eq!(div.len(), 1);
        assert_eq!(div[0], ("claude-x".to_string(), 200_000, 100_000));
    }

    // (ii) A matching override is NOT a divergence.
    #[test]
    fn matching_override_is_not_a_divergence() {
        let cache = cache_with(&[("claude-x", 200_000)]);
        let ov = overrides(&[("claude-x", 200_000)]);
        assert!(divergences(&cache, &ov).is_empty());
    }

    // (iii) No override => not a divergence.
    #[test]
    fn no_override_is_not_a_divergence() {
        let cache = cache_with(&[("claude-x", 200_000)]);
        let ov = overrides(&[]);
        assert!(divergences(&cache, &ov).is_empty());
    }

    // (iv) A cached 0 is NOT a divergence (no false positive).
    #[test]
    fn cached_zero_is_not_a_divergence() {
        let cache = cache_with(&[("claude-x", 0)]);
        let ov = overrides(&[("claude-x", 100_000)]);
        assert!(divergences(&cache, &ov).is_empty());
    }
}
