//! Wave 0 unit-test suite for the Phase 08 org-admin usage/cost path.
//!
//! Per VALIDATION.md §"Per-Task Verification Map", this file declares every
//! Wave 0 unit-test fn by name. As of 08-02 all stubs are implemented as REAL
//! passing tests exercising the pure helpers exposed by
//! `statusline::ant::usage` (`sum_cents_to_usd`, `utc_windows`,
//! `validate_page_token`, `accumulate_tokens`) plus the foundation's
//! `sanitize_account_name` (08-01). The transport / spawn paths are covered by
//! the integration suite (`tests/ant_usage_cli_tests.rs`).

use std::collections::HashMap;

use statusline::ant::cache::TokenBreakdown;
use statusline::ant::usage::{
    accumulate_tokens, sum_cents_to_usd, utc_windows, validate_page_token, ReportEnvelope,
};

// ---------------------------------------------------------------------------
// REAL (08-01): account-name sanitization — VALIDATION.md row `sanitize_account_name`
// ---------------------------------------------------------------------------
#[test]
fn sanitize_account_name() {
    use statusline::ant::cache::sanitize_account_name;

    // Traversal / separators / empty / dot-entries are rejected.
    for bad in ["../", "/", "", ".", "..", "a/b", "../../etc/foo"] {
        assert!(
            sanitize_account_name(bad).is_err(),
            "{bad:?} must be rejected"
        );
    }
    // Conservative-alphabet names are accepted and returned verbatim.
    for ok in ["work", "team-1", "acct.2"] {
        assert_eq!(
            sanitize_account_name(ok).expect("safe name"),
            ok,
            "{ok:?} must be accepted unchanged"
        );
    }
}

// ---------------------------------------------------------------------------
// REAL (08-02): cents-decimal-string sum -> USD — VALIDATION.md row `sum_cents_to_usd`
// ---------------------------------------------------------------------------
#[test]
fn sum_cents_to_usd_test() {
    // Two buckets, mixed cost_types (no filter): "1234" + "66" cents => $13.00.
    // An unparsable amount is skipped tolerantly (D-14/D-15).
    let json = r#"{
        "data": [
            {"results": [{"amount":"1234","cost_type":"tokens"},
                         {"amount":"not-a-number","cost_type":"web_search"}]},
            {"results": [{"amount":"66","cost_type":"session_usage"}]}
        ],
        "has_more": false
    }"#;
    let env: ReportEnvelope<statusline::ant::usage::CostItem> =
        serde_json::from_str(json).expect("valid cost envelope");
    let usd = sum_cents_to_usd(&env);
    assert!((usd - 13.00).abs() < 1e-9, "expected 13.00, got {usd}");
}

// ---------------------------------------------------------------------------
// REAL (08-02): UTC today/MTD bucket windows — VALIDATION.md row `utc_windows_today_mtd`
// ---------------------------------------------------------------------------
#[test]
fn utc_windows_today_mtd() {
    use chrono::{TimeZone, Utc};
    let now = Utc.with_ymd_and_hms(2026, 6, 14, 13, 45, 0).unwrap();
    let w = utc_windows(now);

    // today = [UTC midnight today, UTC midnight tomorrow), ending exclusive.
    assert!(
        w.today.starting_at.starts_with("2026-06-14T00:00:00"),
        "today start: {}",
        w.today.starting_at
    );
    assert!(
        w.today.ending_at.starts_with("2026-06-15T00:00:00"),
        "today end (exclusive): {}",
        w.today.ending_at
    );

    // MTD = [first-of-month, UTC midnight tomorrow).
    assert!(
        w.mtd.starting_at.starts_with("2026-06-01T00:00:00"),
        "mtd start: {}",
        w.mtd.starting_at
    );
    assert_eq!(
        w.mtd.ending_at, w.today.ending_at,
        "both windows share the exclusive midnight-tomorrow end"
    );

    // Both render as RFC3339.
    assert!(w.today.starting_at.contains('T'));
    assert!(w.mtd.starting_at.contains('T'));
}

// ---------------------------------------------------------------------------
// REAL (08-02): pagination page-token validation — VALIDATION.md row `validate_page_token`
// ---------------------------------------------------------------------------
#[test]
fn validate_page_token_test() {
    // The RFC3339 next_page token (with ':') is ACCEPTED (distinct from the
    // Phase-07 cursor validator which rejects ':').
    assert!(validate_page_token("2019-12-27T18:11:19.117Z").is_ok());
    assert!(validate_page_token("page_01H8xYz-abc.123").is_ok());

    // Injection / flag-leading / metachars / empty / over-len rejected.
    assert!(validate_page_token("good\"\nurl = \"http://evil").is_err());
    assert!(validate_page_token("-K/etc/passwd").is_err());
    assert!(validate_page_token("a&b#c").is_err());
    assert!(validate_page_token("a b").is_err());
    assert!(validate_page_token("").is_err());
    assert!(validate_page_token(&"a".repeat(257)).is_err());
}

// ---------------------------------------------------------------------------
// REAL (08-02): tokens-by-model accumulation — VALIDATION.md tokens accumulation row
// ---------------------------------------------------------------------------
#[test]
fn tokens_by_model_accumulates_per_type() {
    let json = r#"{
        "data": [
            {"results": [
                {"model":"opus","uncached_input_tokens":100,"cache_read_input_tokens":10,
                 "output_tokens":50,"cache_creation":{"ephemeral_1h_input_tokens":1,"ephemeral_5m_input_tokens":2}},
                {"model":"sonnet","uncached_input_tokens":200,"output_tokens":80}
            ]},
            {"results": [
                {"model":"opus","uncached_input_tokens":300,"output_tokens":40}
            ]}
        ],
        "has_more": false
    }"#;
    let env: ReportEnvelope<statusline::ant::usage::UsageItem> =
        serde_json::from_str(json).expect("valid usage env");
    let mut acc: HashMap<String, TokenBreakdown> = HashMap::new();
    accumulate_tokens(&env, &mut acc);

    let opus = acc.get("opus").expect("opus present");
    assert_eq!(opus.uncached_input, 400);
    assert_eq!(opus.cache_read_input, 10);
    assert_eq!(opus.output, 90);
    assert_eq!(opus.cache_creation_1h, 1);
    assert_eq!(opus.cache_creation_5m, 2);
    // The rendered per-model TOTAL is the sum of the five fields.
    assert_eq!(opus.total(), 400 + 10 + 90 + 1 + 2);

    let sonnet = acc.get("sonnet").expect("sonnet present");
    assert_eq!(sonnet.total(), 200 + 80);
}
