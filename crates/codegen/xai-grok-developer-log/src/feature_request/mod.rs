//! Feature Request Log (FRL) — structured product-capability requests.
//!
//! Parallel product-signal pipeline to Auto Developer Log:
//! - Agents file via the `feature_request_log` tool
//! - Store under `$GROK_HOME/feature-request-log/` (dedup by fingerprint)
//! - Operators review with `turbo features list|show|export`

pub mod export;
pub mod fingerprint;
pub mod schema;
pub mod store;

pub use export::{FrExportOptions, FrExportResult, export_feature_requests};
pub use fingerprint::compute_fr_fingerprint;
pub use schema::*;
pub use store::{
    FR_DIR_ENV, FR_ENABLED_ENV, FeatureRequestStore, FrIndexEntry, FrListFilter, FrStoreError,
    fr_builtin_default_root, fr_clear_configured_dir, fr_config_file_path, fr_default_root,
    fr_is_enabled, fr_root_resolution_note, fr_set_configured_dir, fr_set_root_override,
    load_feature_log_file_config, sanitize_feature_request,
};
