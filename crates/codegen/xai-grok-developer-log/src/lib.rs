//! Auto Developer Log (ADL) — structured product-issue store for Hyper.
//!
//! Agents and runtime detectors file **incidents** under the configured root
//! (default `$GROK_HOME/developer-log/`). Override with env
//! `GROK_DEVELOPER_LOG_DIR`, `hyper issues set-dir`, or
//! `$GROK_HOME/developer-log.toml`. Incidents are deduplicated by fingerprint,
//! redacted for secrets/user paths, and exportable via `hyper issues export`.
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
    DIR_ENV, DeveloperLogStore, ENABLED_ENV, IndexEntry, ListFilter, StoreError,
    builtin_default_root, clear_configured_dir, config_file_path, default_root, is_enabled,
    report_best_effort, root_resolution_note, set_configured_dir, set_root_override,
};
