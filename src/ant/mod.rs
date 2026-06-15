//! Ant (Anthropic CLI enrichment) module.
//!
//! Opt-in, default-off enrichment of the statusline with authoritative Claude
//! API data (model metadata, and later usage/cost) that the stdin payload can't
//! provide. The render path stays offline, auth-free, and fast — refresh is
//! strictly out-of-band (a future `statusline ant sync` subcommand), and the
//! whole feature is gated behind the `[ant]` config section (default disabled).
//!
//! Submodules:
//! - [`config`] — the `[ant]` TOML section (`AntConfig`, default-off).
//! - [`cache`] — the versioned atomic model-metadata cache (write + resilient
//!   read; the read path never creates a directory, never panics, never spawns).
//! - [`fetch`] — the out-of-band `ant`/`curl` fetch (implemented in Plan 03;
//!   declared here so Plan 03 only adds the file, never edits this module).
//! - [`usage`] — the out-of-band org-admin usage/cost fetch (implemented in
//!   08-02; declared here as a placeholder so the module graph is stable).
//! - [`duration`] — pure `--max-age` / staleness duration parse + humanize
//!   helpers (no new crate; Phase 09 foundation).
//! - [`audit`] — the shared `sk-ant-` leak scanner over on-disk artifacts
//!   (caches + debug log; never scans process env, D-18; Phase 09 foundation).

pub mod audit;
pub mod cache;
pub mod config;
pub mod duration;
pub mod fetch;
pub mod usage;
