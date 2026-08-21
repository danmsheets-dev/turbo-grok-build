//! # xai-workspace-tree
//!
//! Workspace directory atlas for Turbo / Hyper Grok Build.
//!
//! Phase 1 foundation: walk a workspace with gitignore + hard excludes, collapse
//! noisy directories, persist `meta.json` + `tree.v1.json` under
//! `~/.grok/workspace-trees/<workspace_id>/`, and expose query + inject helpers.
//!
//! ## Typical tools-layer usage
//!
//! ```ignore
//! use xai_workspace_tree::{
//!     build_and_save, inject_card, load_or_build, resolve_path, summary,
//!     WorkspaceTreeConfig,
//! };
//!
//! let config = WorkspaceTreeConfig::from_env();
//! let index = load_or_build(workspace_root, &config)?;
//! let card = inject_card(&index, &config);
//! let hits = resolve_path(&index, "ship_roster", Some("scripts/ship/"), 8);
//! let s = summary(&index, 24);
//! ```
//!
//! Do **not** dump the full tree into the model context — use [`inject_card`]
//! and the query helpers instead.

mod config;
mod error;
mod identity;
mod inject;
mod model;
mod query;
mod store;
mod walk;

pub use config::{
    CollapseConfig, InjectConfig, InjectMode, WalkConfig, WorkspaceTreeConfig,
    default_hard_exclude_exts, default_hard_exclude_names, effective_hard_exclude_names,
};
pub use error::{Error, Result};
pub use identity::{
    WORKSPACE_ID_PREFIX, canonicalize_root, default_store_root, path_to_identity_key,
    resolve_grok_home, workspace_id_for_canonical, workspace_id_for_path,
};
pub use inject::{inject_building_notice, inject_card, inject_disabled_notice};
pub use model::{
    BuildInfo, Freshness, FreshnessState, GitInfo, Meta, NodeKind, SCHEMA_VERSION, Stats,
    TreeIndex, TreeNode, TreePayload, detect_workspace_profile, extension_of, role_tags_for,
    to_posix_rel,
};
pub use query::{
    ListEntry, ListResult, QueryMeta, ResolveHit, ResolveResult, SearchHit, SearchResult,
    SummaryResult, find_node, list, resolve_path, search, summary,
};
pub use store::{
    PruneReport, build_and_save, dir_size_bytes, load_index, load_index_for_root,
    load_index_from_dir, load_or_build, prune_store, save_index, store_disk_usage, store_root,
    workspace_store_dir, write_atomically,
};
pub use walk::{build_index, path_matches_glob, reassess_freshness};
