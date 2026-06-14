//! Wave 0 unit-test scaffold for the Phase 08 org-admin usage/cost path.
//!
//! Per VALIDATION.md §"Wave 0 Requirements" / §"Per-Task Verification Map", this
//! file declares every Wave 0 unit-test fn by name. The functions whose
//! implementation already landed in 08-01 (`sanitize_account_name`) carry REAL
//! passing assertions; the functions that land in 08-02 (`sum_cents_to_usd`,
//! `utc_windows_today_mtd`, `validate_page_token`, tokens-by-model accumulation)
//! are declared now as `#[ignore]` stubs with a `todo!("implemented in 08-02")`
//! body so the file COMPILES and the suite stays green until 08-02 fills them in.

// `sanitize_account_name` is `pub(crate)` so the external integration test reaches
// it via the crate's public-test surface. It is exercised through the public
// `statusline::ant::cache` path.

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
// STUB (08-02): cents-decimal-string sum -> USD — VALIDATION.md row `sum_cents_to_usd`
// ---------------------------------------------------------------------------
#[test]
#[ignore = "wave 0 stub — 08-02"]
fn sum_cents_to_usd() {
    // 08-02: sum ALL `amount` cents-decimal-string entries and divide by 100.
    todo!("implemented in 08-02");
}

// ---------------------------------------------------------------------------
// STUB (08-02): UTC today/MTD bucket windows — VALIDATION.md row `utc_windows_today_mtd`
// ---------------------------------------------------------------------------
#[test]
#[ignore = "wave 0 stub — 08-02"]
fn utc_windows_today_mtd() {
    // 08-02: chrono-derived [today 00:00, now) and [month-start, now) UTC windows.
    todo!("implemented in 08-02");
}

// ---------------------------------------------------------------------------
// STUB (08-02): pagination page-token validation — VALIDATION.md row `validate_page_token`
// ---------------------------------------------------------------------------
#[test]
#[ignore = "wave 0 stub — 08-02"]
fn validate_page_token() {
    // 08-02: like fetch::validate_cursor but the `:` case PASSES for the usage
    // pagination token; injection (quote/newline/flag-leading/metachars) rejected.
    todo!("implemented in 08-02");
}

// ---------------------------------------------------------------------------
// STUB (08-02): tokens-by-model accumulation — VALIDATION.md tokens accumulation row
// ---------------------------------------------------------------------------
#[test]
#[ignore = "wave 0 stub — 08-02"]
fn tokens_by_model_accumulates_per_type() {
    // 08-02: group_by=model accumulation across pages into TokenBreakdown per model.
    todo!("implemented in 08-02");
}
