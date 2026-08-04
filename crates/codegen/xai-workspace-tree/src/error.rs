//! Error types for the workspace tree crate.

use std::path::PathBuf;

/// Errors produced by workspace tree operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("path is not absolute and cannot be resolved: {path}")]
    NotAbsolute { path: PathBuf },

    #[error("failed to canonicalize path {path}: {source}")]
    Canonicalize {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("path is outside the workspace root: {path}")]
    OutsideWorkspace { path: String },

    #[error("path not found in index: {path}")]
    NotFound { path: String },

    #[error("workspace tree store not found for id {workspace_id} under {store_root}")]
    StoreMissing {
        workspace_id: String,
        store_root: PathBuf,
    },

    #[error("invalid store data at {path}: {message}")]
    StoreCorrupt { path: PathBuf, message: String },

    #[error("schema version mismatch: found {found}, expected {expected}")]
    SchemaVersion { found: u32, expected: u32 },

    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("walk error: {message}")]
    Walk { message: String },

    #[error("workspace tree is disabled in config")]
    Disabled,
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;
