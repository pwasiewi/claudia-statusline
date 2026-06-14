//! ant/curl usage fetch (implemented in 08-02)
//!
//! Placeholder module: 08-01 declares `pub mod usage;` so the module graph is
//! stable for Wave 2; 08-02 overwrites this file with the org-admin usage/cost
//! fetch (cents→USD aggregation, today/MTD windows, per-model token breakdown,
//! all-or-nothing two-endpoint write). The per-account cache contracts it writes
//! to live in `super::cache` (`UsageCache`, `write_usage_cache`,
//! `sanitize_account_name`).
