//! `ant` subcommand handlers (out-of-band Anthropic enrichment).
//!
//! Thin binary-only handlers for `statusline ant sync-models`. All reusable
//! logic (the fetch transport, credential modes, pagination, parsing) lives in
//! `statusline::ant::fetch`; the cache IO lives in `statusline::ant::cache`.
//! This handler only orchestrates: load config -> fetch -> write cache ->
//! print a credential-safe summary, emitting the D-03 divergence note on the
//! SYNC PATH ONLY (never the render path).

use std::collections::HashMap;

use crate::ant::cache::{models_cache_path, write_models_cache, ModelsCache};
use crate::config::Config;
use crate::error::Result;

/// Dispatch entry for `statusline ant <action>`.
pub(crate) fn handle_ant_command(action: crate::AntAction) -> Result<()> {
    match action {
        crate::AntAction::SyncModels { quiet } => sync_models(quiet),
    }
}

/// `ant sync-models`: fetch the Models API out-of-band and publish the cache.
///
/// On success writes the versioned cache (only after all pages parse — the
/// fetch layer guarantees this), emits the D-03 one-time stderr divergence note
/// for any cached window that differs from a winning user override, and (unless
/// `quiet`) prints a short, KEY-FREE summary. On failure the error propagates to
/// `main`, which exits non-zero with a differentiated stderr message (D-13).
fn sync_models(quiet: bool) -> Result<()> {
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
