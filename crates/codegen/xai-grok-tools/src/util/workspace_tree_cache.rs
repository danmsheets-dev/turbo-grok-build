//! Process-local workspace tree index cache.
//!
//! Phase 1 (PR-B): tools load the atlas on first use via [`get_or_load`].
//! Callers may also fire-and-forget [`kickoff_load`] at trusted workspace open
//! so the first tool call is a cache hit.
//!
//! **Do not block session start on a full walk.** Kickoff is best-effort and
//! runs on a detached thread. Miss recovery uses [`try_get`] / [`try_load_cached`]
//! only (never builds).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use xai_workspace_tree::{
    TreeIndex, WorkspaceTreeConfig, build_and_save, load_index_for_root, load_or_build,
};

use crate::types::resources::{SharedResources, WorkspaceTreeIndexingAllowed};

/// RC13 P1 F9: refuse atlas walk/build when shell stamped trust=false.
pub async fn ensure_indexing_allowed(
    resources: &SharedResources,
) -> Result<(), xai_tool_runtime::ToolError> {
    let res = resources.lock().await;
    if let Some(gate) = res.get::<WorkspaceTreeIndexingAllowed>()
        && !gate.0
    {
        return Err(xai_tool_runtime::ToolError::custom(
            "workspace_tree_untrusted",
            "workspace tree indexing is disabled for this folder (not trusted). \
             Trust the workspace or set GROK_FOLDER_TRUST_INERT=1 only for tests.",
        ));
    }
    Ok(())
}

/// In-memory handle shared by tools and miss recovery.
pub type SharedTreeIndex = Arc<TreeIndex>;

fn cache() -> &'static Mutex<HashMap<String, SharedTreeIndex>> {
    static CACHE: OnceLock<Mutex<HashMap<String, SharedTreeIndex>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Per-workspace load/build mutex so kickoff + first tool do not dual-walk (RC13 P1 F8).
fn load_locks() -> &'static Mutex<HashMap<String, Arc<Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_for_key(key: &str) -> Arc<Mutex<()>> {
    let mut map = load_locks().lock().unwrap_or_else(|e| e.into_inner());
    map.entry(key.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Canonical cache key for a workspace root (best-effort).
pub fn cache_key_for_root(root: &Path) -> String {
    dunce::canonicalize(root)
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn insert(key: String, index: TreeIndex) -> SharedTreeIndex {
    let arc = Arc::new(index);
    if let Ok(mut guard) = cache().lock() {
        guard.insert(key, Arc::clone(&arc));
    }
    arc
}

/// Return a process-cached index for `root` if already loaded (no I/O).
pub fn try_get(root: &Path) -> Option<SharedTreeIndex> {
    let key = cache_key_for_root(root);
    cache().lock().ok()?.get(&key).cloned()
}

/// Try process cache, then durable store (no build). Used by miss recovery.
pub fn try_load_cached(root: &Path, config: &WorkspaceTreeConfig) -> Option<SharedTreeIndex> {
    if let Some(idx) = try_get(root) {
        return Some(idx);
    }
    match load_index_for_root(root, config) {
        Ok(idx) => Some(insert(cache_key_for_root(root), idx)),
        Err(e) => {
            tracing::debug!(
                root = %root.display(),
                error = %e,
                "workspace tree store not available for miss recovery"
            );
            None
        }
    }
}

/// Load from store or build+save, then cache. Blocking; call from spawn_blocking.
///
/// On process-cache hit, reassess git freshness; if the index is stale, rebuild
/// (same policy as [`xai_workspace_tree::load_or_build`]).
///
/// Serializes concurrent load/build for the same root (kickoff + first tool).
pub fn get_or_load(root: &Path, config: &WorkspaceTreeConfig) -> Result<SharedTreeIndex, String> {
    if !config.enabled {
        return Err("workspace tree is disabled (GROK_WORKSPACE_TREE=0)".into());
    }
    let key = cache_key_for_root(root);
    let key_lock = lock_for_key(&key);
    let _serial = key_lock.lock().unwrap_or_else(|e| e.into_inner());

    if let Ok(guard) = cache().lock()
        && let Some(idx) = guard.get(&key)
    {
        let mut meta = idx.meta.clone();
        let state = xai_workspace_tree::reassess_freshness(root, &mut meta);
        if !matches!(state, xai_workspace_tree::FreshnessState::Stale) {
            return Ok(Arc::clone(idx));
        }
    }
    let index = load_or_build(root, config).map_err(|e| e.to_string())?;
    Ok(insert(key, index))
}

/// Force rebuild+save and replace the cache entry.
pub fn refresh(root: &Path, config: &WorkspaceTreeConfig) -> Result<SharedTreeIndex, String> {
    let key = cache_key_for_root(root);
    let key_lock = lock_for_key(&key);
    let _serial = key_lock.lock().unwrap_or_else(|e| e.into_inner());
    let index = build_and_save(root, config).map_err(|e| e.to_string())?;
    Ok(insert(key, index))
}

/// Fire-and-forget `load_or_build` into the process cache.
///
/// Safe to call from session/workspace open. Never blocks the caller.
/// Wired from shell trusted-folder session open (`agent_ops` when
/// `project_scope_allowed`). Uses the same per-key lock as [`get_or_load`].
pub fn kickoff_load(root: PathBuf) {
    kickoff_load_with_config(root, WorkspaceTreeConfig::from_env());
}

/// Like [`kickoff_load`] with an explicit config (e.g. custom store dir in tests).
pub fn kickoff_load_with_config(root: PathBuf, config: WorkspaceTreeConfig) {
    if !config.enabled {
        return;
    }
    let key = cache_key_for_root(&root);
    if let Ok(guard) = cache().lock()
        && guard.contains_key(&key)
    {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("workspace-tree-kickoff".into())
        .spawn(move || {
            // Share singleflight with get_or_load / refresh.
            let key_lock = lock_for_key(&key);
            let _serial = key_lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Ok(guard) = cache().lock()
                && guard.contains_key(&key)
            {
                return;
            }
            match load_or_build(&root, &config) {
                Ok(idx) => {
                    let _ = insert(key, idx);
                    tracing::debug!(
                        root = %root.display(),
                        "workspace tree kickoff load complete"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        root = %root.display(),
                        error = %e,
                        "workspace tree kickoff load failed"
                    );
                }
            }
        });
}

/// Test helper: clear the process cache.
#[cfg(test)]
pub fn clear_cache_for_tests() {
    if let Ok(mut guard) = cache().lock() {
        guard.clear();
    }
    if let Ok(mut locks) = load_locks().lock() {
        locks.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn get_or_load_caches_and_refresh_rebuilds() {
        clear_cache_for_tests();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("ws");
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::write(root.join("scripts/a.rs"), b"fn a(){}").unwrap();
        let store = tmp.path().join("store");
        let mut cfg = WorkspaceTreeConfig::default();
        cfg.store_dir = Some(store);

        let a = get_or_load(&root, &cfg).unwrap();
        let b = try_get(&root).expect("cached");
        assert!(Arc::ptr_eq(&a, &b));

        std::fs::write(root.join("scripts/b.rs"), b"fn b(){}").unwrap();
        let c = refresh(&root, &cfg).unwrap();
        assert!(!Arc::ptr_eq(&a, &c));
        assert!(c.name_index.contains_key("b.rs") || c.name_index.contains_key("b"));
    }
}
