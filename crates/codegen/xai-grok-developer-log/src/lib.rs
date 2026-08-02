//! Auto Developer Log (ADL) + Feature Request Log (FRL) for Turbo.
//!
//! ## Auto Developer Log (product bugs / friction)
//! Agents and runtime detectors file **incidents** under the configured root
//! (default `$GROK_HOME/developer-log/`). Override with env
//! `GROK_DEVELOPER_LOG_DIR`, `turbo issues set-dir`, or
//! `$GROK_HOME/developer-log.toml`. Incidents are deduplicated by fingerprint,
//! redacted for secrets/user paths, and exportable via `turbo issues export`.
//! Disable with `GROK_DEVELOPER_LOG=0`.
//!
//! ## Feature Request Log (missing capabilities)
//! Agents file **feature requests** under `$GROK_HOME/feature-request-log/`
//! via the `feature_request_log` tool when harness work needs a product
//! surface that does not exist yet. Operators triage with `turbo features`.
//! Disable with `GROK_FEATURE_REQUEST_LOG=0`.

pub mod detectors;
pub mod export;
pub mod feature_request;
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
pub use feature_request::{
    FR_DIR_ENV, FR_ENABLED_ENV, FeatureRequest, FeatureRequestReport, FeatureRequestResult,
    FeatureRequestStore, FrExportOptions, FrExportResult, FrIndexEntry, FrListFilter, FrStoreError,
    RequestClass, RequestPriority, RequestStatus, agent_source, compute_fr_fingerprint,
    export_feature_requests, fr_builtin_default_root, fr_clear_configured_dir, fr_config_file_path,
    fr_default_root, fr_is_enabled, fr_root_resolution_note, fr_set_configured_dir,
    fr_set_root_override,
};
pub use fingerprint::compute_fingerprint;
pub use schema::*;
pub use store::{
    DIR_ENV, DeveloperLogStore, ENABLED_ENV, IndexEntry, ListFilter, StoreError,
    builtin_default_root, clear_configured_dir, config_file_path, default_root, is_enabled,
    report_best_effort, root_resolution_note, set_configured_dir, set_root_override,
};
