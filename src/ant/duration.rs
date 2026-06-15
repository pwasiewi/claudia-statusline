//! Tiny, dependency-free duration parse + humanize helpers for the `ant`
//! enrichment feature.
//!
//! [`parse_max_age`] is a pure, validating parser for the single-unit duration
//! strings used by `--max-age` (and the `[ant]` staleness thresholds): `10m`,
//! `90s`, `24h`, `2d`. It mirrors the project's reference validating-parser
//! style ([`crate::ant::cache::sanitize_account_name`]) — `Result<_,
//! StatuslineError>` with explicit, user-facing messages and no panics. We do
//! NOT pull in a `humantime`-style crate (lean-deps policy / `deny.toml`).
//!
//! [`humanize_age`] renders a [`chrono::Duration`] as a compact relative age
//! (`<1m` / `10m` / `2h` / `3d`), clamping negative durations (clock skew) to
//! `<1m` so a slightly-ahead `fetched_at` never produces a nonsense age.

// Forward-public foundation API consumed by Plans 09-02/03/04. The binary crate
// (src/main.rs's own `mod ant`) does not reference these yet — mirror the
// existing `#![allow(dead_code)]` on fetch.rs/cache.rs/usage.rs.
#![allow(dead_code)]

/// Parse a single-unit duration string (`10m`, `90s`, `24h`, `2d`) into a
/// [`std::time::Duration`].
///
/// The grammar is a non-negative integer followed by exactly one unit suffix:
/// `s` (seconds), `m` (minutes), `h` (hours), or `d` (days). Whitespace is
/// trimmed first. Every malformed input — empty, no unit, non-numeric prefix,
/// unknown unit — returns a [`StatuslineError::Config`] with a clear message and
/// never panics or allocates unboundedly (`u64` multiply, bounded units).
///
/// [`StatuslineError::Config`]: crate::error::StatuslineError::Config
pub fn parse_max_age(s: &str) -> crate::error::Result<std::time::Duration> {
    use crate::error::StatuslineError;
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).ok_or_else(|| {
        StatuslineError::Config(format!("invalid --max-age '{s}' (need a unit: s/m/h/d)"))
    })?;
    let (num, unit) = s.split_at(split);
    let n: u64 = num
        .parse()
        .map_err(|_| StatuslineError::Config(format!("invalid --max-age number in '{s}'")))?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86_400,
        other => {
            return Err(StatuslineError::Config(format!(
                "invalid --max-age unit '{other}' (use s/m/h/d)"
            )))
        }
    };
    Ok(std::time::Duration::from_secs(secs))
}

/// Render a [`chrono::Duration`] as a compact relative age string.
///
/// Returns `<1m` for anything under a minute (including negative durations from
/// clock skew), then `{}m` / `{}h` / `{}d` using integer division. Designed for
/// the dim `{api_*_age}` template variables.
pub fn humanize_age(d: chrono::Duration) -> String {
    let secs = d.num_seconds().max(0); // negative clock skew => <1m (Pitfall 1)
    if secs < 60 {
        "<1m".to_string()
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn parse_accepts_each_unit() {
        assert_eq!(parse_max_age("90s").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_max_age("10m").unwrap(), Duration::from_secs(600));
        assert_eq!(parse_max_age("24h").unwrap(), Duration::from_secs(86_400));
        assert_eq!(parse_max_age("2d").unwrap(), Duration::from_secs(172_800));
    }

    #[test]
    fn parse_trims_whitespace() {
        assert_eq!(parse_max_age("  10m  ").unwrap(), Duration::from_secs(600));
    }

    #[test]
    fn parse_rejects_empty() {
        assert!(parse_max_age("").is_err());
    }

    #[test]
    fn parse_rejects_non_numeric() {
        assert!(parse_max_age("abc").is_err());
    }

    #[test]
    fn parse_rejects_unknown_unit() {
        assert!(parse_max_age("10x").is_err());
    }

    #[test]
    fn parse_rejects_bare_unit() {
        // "m" has no numeric prefix -> the prefix parses as empty -> Err.
        assert!(parse_max_age("m").is_err());
    }

    #[test]
    fn humanize_under_a_minute() {
        assert_eq!(humanize_age(chrono::Duration::seconds(30)), "<1m");
    }

    #[test]
    fn humanize_minutes_hours_days() {
        assert_eq!(humanize_age(chrono::Duration::seconds(600)), "10m");
        assert_eq!(humanize_age(chrono::Duration::seconds(7200)), "2h");
        assert_eq!(humanize_age(chrono::Duration::days(3)), "3d");
    }

    #[test]
    fn humanize_negative_is_clock_skew_safe() {
        // A fetched_at slightly in the future yields a negative duration.
        assert_eq!(humanize_age(chrono::Duration::seconds(-5)), "<1m");
    }
}
