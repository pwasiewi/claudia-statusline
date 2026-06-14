//! Ant (Anthropic CLI enrichment) module configuration.
//!
//! Defines the `[ant]` TOML section for the opt-in Claude API enrichment
//! feature. Uses `#[serde(default)]` so existing configs without an `[ant]`
//! section silently receive sensible defaults.
//!
//! Per CONTEXT.md decision D-08, `enabled` defaults to **false** (the opposite
//! of `[gsd]`, which defaults to true). The section is intentionally minimal —
//! D-10 forbids a `cache_dir` key (location is controlled by `XDG_CACHE_HOME`)
//! and there are no usage/staleness sub-toggles in this foundation plan.

use serde::{Deserialize, Serialize};

/// Configuration for the `ant` (opt-in Claude API enrichment) module.
///
/// Added to `statusline.toml` as an `[ant]` section. All fields have defaults
/// via `#[serde(default)]`, so configs without this section work unchanged and
/// — critically — render byte-identically to v3.1.0 when `[ant]` is absent or
/// `enabled = false`.
///
/// # Example
///
/// ```toml
/// [ant]
/// enabled = true
/// profile = "work"   # optional `ant` CLI profile name; default ""
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AntConfig {
    /// Enable the `ant` enrichment module (default: **false**, opt-in per D-08).
    pub enabled: bool,
    /// Optional `ant` CLI profile name to use for out-of-band fetches.
    /// When empty, the default profile / key-precedence applies.
    pub profile: String,
}

// The manual impl is intentional (and mirrors `GsdConfig`): it makes the
// security-relevant D-08 default — `enabled = false` (opt-in) — explicit at the
// definition site rather than implied by the field type's derived default.
#[allow(clippy::derivable_impls)]
impl Default for AntConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            profile: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled_with_empty_profile() {
        let cfg = AntConfig::default();
        assert!(!cfg.enabled, "ant must default to disabled (D-08)");
        assert!(cfg.profile.is_empty(), "profile must default to empty");
    }

    #[test]
    fn toml_without_ant_table_yields_default() {
        // Deserializing an empty TOML document with #[serde(default)] yields the
        // default AntConfig (mirrors how a statusline.toml with no [ant] section
        // is handled when the field carries #[serde(default)]).
        let cfg: AntConfig = toml::from_str("").expect("empty TOML should parse to default");
        assert!(!cfg.enabled);
        assert!(cfg.profile.is_empty());
    }

    #[test]
    fn toml_enables_and_sets_profile() {
        let cfg: AntConfig =
            toml::from_str("enabled = true\nprofile = \"work\"\n").expect("valid TOML");
        assert!(cfg.enabled);
        assert_eq!(cfg.profile, "work");
    }
}
