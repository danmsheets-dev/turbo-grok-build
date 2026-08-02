//! Auto Developer Log (ADL) — structured product-issue store for Hyper.
//!
//! Agents and runtime detectors file **incidents** under
//! `$GROK_HOME/developer-log/`. Incidents are deduplicated by fingerprint,
//! redacted for secrets/user paths, and exportable as a maintainer pack via
//! `hyper issues export`.
//!
//! Disable with `GROK_DEVELOPER_LOG=0`.

pub mod detectors;
pub mod export;
pub mod fingerprint;
pub mod redact;
pub mod schema;
pub mod store;

pub use detectors::{
    IsolationFallbackSignal, ProviderFailureSignal, StallSignal, WorktreeDisposeSignal,
    detect_isolation_fallback, detect_provider_failure, detect_subagent_stall,
    detect_worktree_dispose, detect_worktree_dispose_in, worktree_dispose_is_risky,
};
pub use export::{ExportOptions, ExportResult, export_pack};
pub use fingerprint::compute_fingerprint;
pub use schema::*;
pub use store::{
    DeveloperLogStore, ENABLED_ENV, IndexEntry, ListFilter, StoreError, default_root, is_enabled,
    report_best_effort,
};
