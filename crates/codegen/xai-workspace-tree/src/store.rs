//! Durable store under `~/.grok/workspace-trees/<id>/`.

use crate::config::WorkspaceTreeConfig;
use crate::error::{Error, Result};
use crate::identity::{default_store_root, workspace_id_for_path};
use crate::model::{Meta, TreeIndex, TreePayload, SCHEMA_VERSION};
use crate::walk::build_index;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
// chrono used by prune_store for meta.updated_at parsing

const META_FILE: &str = "meta.json";
const TREE_FILE: &str = "tree.v1.json";
const REGISTRY_FILE: &str = "index.json";

/// Resolve store root from config or default.
pub fn store_root(config: &WorkspaceTreeConfig) -> PathBuf {
    config
        .store_dir
        .clone()
        .unwrap_or_else(default_store_root)
}

/// Directory for a workspace id: `<store_root>/<workspace_id>/`.
pub fn workspace_store_dir(store_root: &Path, workspace_id: &str) -> PathBuf {
    store_root.join(workspace_id)
}

/// Atomically write `contents` to `path` via temp + rename.
///
/// On Windows, `rename` does **not** replace an existing destination; we remove
/// the target first after the temp file is fully written and fsynced (RC13 P0 F2).
pub fn write_atomically(path: &Path, contents: &[u8]) -> Result<()> {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir).map_err(|source| Error::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_owned());
    let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!("{name}.{}.{nonce}.tmp", std::process::id()));
    let result = (|| {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(contents)?;
        f.sync_all()?;
        // Windows: rename fails with "file exists" if dest already exists.
        // Unix rename replaces atomically — remove_file is still safe there.
        if path.exists() {
            fs::remove_file(path)?;
        }
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Save index to store (`meta.json` + `tree.v1.json`). Returns workspace store dir.
pub fn save_index(index: &TreeIndex, config: &WorkspaceTreeConfig) -> Result<PathBuf> {
    let root = store_root(config);
    let dir = workspace_store_dir(&root, &index.meta.workspace_id);
    fs::create_dir_all(&dir).map_err(|source| Error::Io {
        path: dir.clone(),
        source,
    })?;

    let meta_json = serde_json::to_vec_pretty(&index.meta)?;
    write_atomically(&dir.join(META_FILE), &meta_json)?;

    let payload = index.to_payload();
    let tree_json = serde_json::to_vec_pretty(&payload)?;
    write_atomically(&dir.join(TREE_FILE), &tree_json)?;

    // Optional registry update (best-effort).
    let _ = update_registry(&root, &index.meta);

    Ok(dir)
}

/// Load an index by workspace id.
pub fn load_index(workspace_id: &str, config: &WorkspaceTreeConfig) -> Result<TreeIndex> {
    let root = store_root(config);
    let dir = workspace_store_dir(&root, workspace_id);
    load_index_from_dir(&dir)
}

/// Load index for a filesystem root (computes workspace id first).
pub fn load_index_for_root(root_path: &Path, config: &WorkspaceTreeConfig) -> Result<TreeIndex> {
    let id = workspace_id_for_path(root_path)?;
    load_index(&id, config)
}

/// Load from an explicit store directory.
pub fn load_index_from_dir(dir: &Path) -> Result<TreeIndex> {
    let meta_path = dir.join(META_FILE);
    let tree_path = dir.join(TREE_FILE);
    if !meta_path.exists() || !tree_path.exists() {
        return Err(Error::StoreMissing {
            workspace_id: dir
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            store_root: dir
                .parent()
                .unwrap_or(dir)
                .to_path_buf(),
        });
    }

    let meta_bytes = fs::read(&meta_path).map_err(|source| Error::Io {
        path: meta_path.clone(),
        source,
    })?;
    let meta: Meta = serde_json::from_slice(&meta_bytes).map_err(|e| Error::StoreCorrupt {
        path: meta_path.clone(),
        message: e.to_string(),
    })?;
    if meta.schema_version != SCHEMA_VERSION {
        return Err(Error::SchemaVersion {
            found: meta.schema_version,
            expected: SCHEMA_VERSION,
        });
    }

    let tree_bytes = fs::read(&tree_path).map_err(|source| Error::Io {
        path: tree_path.clone(),
        source,
    })?;
    let payload: TreePayload =
        serde_json::from_slice(&tree_bytes).map_err(|e| Error::StoreCorrupt {
            path: tree_path,
            message: e.to_string(),
        })?;
    if payload.schema_version != SCHEMA_VERSION {
        return Err(Error::SchemaVersion {
            found: payload.schema_version,
            expected: SCHEMA_VERSION,
        });
    }

    Ok(TreeIndex::from_parts(meta, payload))
}

/// Build index and persist it.
pub fn build_and_save(root: &Path, config: &WorkspaceTreeConfig) -> Result<TreeIndex> {
    let index = build_index(root, config)?;
    save_index(&index, config)?;
    Ok(index)
}

/// Load from store if present, otherwise build + save.
///
/// RC13 P0 F4: when a durable index exists, reassess freshness against the live
/// workspace (git HEAD). If the index is **stale** (HEAD moved), rebuild so
/// tools do not serve a permanently “fresh” lie.
pub fn load_or_build(root: &Path, config: &WorkspaceTreeConfig) -> Result<TreeIndex> {
    match load_index_for_root(root, config) {
        Ok(mut idx) => {
            let state = crate::walk::reassess_freshness(root, &mut idx.meta);
            if matches!(state, crate::model::FreshnessState::Stale) {
                return build_and_save(root, config);
            }
            Ok(idx)
        }
        Err(Error::StoreMissing { .. }) => build_and_save(root, config),
        Err(e) => Err(e),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Registry {
    schema_version: u32,
    workspaces: Vec<RegistryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegistryEntry {
    workspace_id: String,
    root: String,
    updated_at: String,
}

fn update_registry(store_root: &Path, meta: &Meta) -> Result<()> {
    let path = store_root.join(REGISTRY_FILE);
    let mut reg = if path.exists() {
        let bytes = fs::read(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        serde_json::from_slice::<Registry>(&bytes).unwrap_or_default()
    } else {
        Registry {
            schema_version: SCHEMA_VERSION,
            workspaces: Vec::new(),
        }
    };
    reg.schema_version = SCHEMA_VERSION;
    if let Some(existing) = reg
        .workspaces
        .iter_mut()
        .find(|e| e.workspace_id == meta.workspace_id)
    {
        existing.root = meta.canonical_root.clone();
        existing.updated_at = meta.updated_at.clone();
    } else {
        reg.workspaces.push(RegistryEntry {
            workspace_id: meta.workspace_id.clone(),
            root: meta.canonical_root.clone(),
            updated_at: meta.updated_at.clone(),
        });
    }
    let json = serde_json::to_vec_pretty(&reg)?;
    write_atomically(&path, &json)
}

/// Result of a store prune pass (RC13 P2 F18).
#[derive(Debug, Clone, Default)]
pub struct PruneReport {
    pub removed_dirs: u32,
    pub freed_bytes: u64,
    pub remaining_dirs: u32,
    pub remaining_bytes: u64,
}

/// Approximate recursive size of a directory.
pub fn dir_size_bytes(path: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(rd) = fs::read_dir(path) else {
        return 0;
    };
    for ent in rd.flatten() {
        let p = ent.path();
        if p.is_dir() {
            total = total.saturating_add(dir_size_bytes(&p));
        } else if let Ok(md) = ent.metadata() {
            total = total.saturating_add(md.len());
        }
    }
    total
}

/// Summarize disk usage under the workspace-tree store root.
pub fn store_disk_usage(config: &WorkspaceTreeConfig) -> (u32, u64) {
    let root = store_root(config);
    if !root.is_dir() {
        return (0, 0);
    }
    let mut dirs = 0u32;
    let mut bytes = 0u64;
    if let Ok(rd) = fs::read_dir(&root) {
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() {
                dirs = dirs.saturating_add(1);
                bytes = bytes.saturating_add(dir_size_bytes(&p));
            } else if let Ok(md) = ent.metadata() {
                bytes = bytes.saturating_add(md.len());
            }
        }
    }
    (dirs, bytes)
}

/// Prune durable workspace indexes older than `max_age` (by meta.updated_at or dir mtime).
///
/// Also enforces optional `keep_newest` (0 = unlimited) after age filter.
/// Never deletes `index.json` at the store root.
pub fn prune_store(
    config: &WorkspaceTreeConfig,
    max_age: std::time::Duration,
    keep_newest: usize,
) -> Result<PruneReport> {
    let root = store_root(config);
    if !root.is_dir() {
        return Ok(PruneReport::default());
    }
    let now = std::time::SystemTime::now();
    let mut candidates: Vec<(PathBuf, std::time::SystemTime, u64)> = Vec::new();
    let rd = fs::read_dir(&root).map_err(|source| Error::Io {
        path: root.clone(),
        source,
    })?;
    for ent in rd.flatten() {
        let p = ent.path();
        if !p.is_dir() {
            continue;
        }
        let size = dir_size_bytes(&p);
        let mtime = ent
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        // Prefer meta.updated_at if present.
        let meta_path = p.join(META_FILE);
        let age_base = if meta_path.is_file() {
            if let Ok(bytes) = fs::read(&meta_path) {
                if let Ok(meta) = serde_json::from_slice::<Meta>(&bytes) {
                    chrono::DateTime::parse_from_rfc3339(&meta.updated_at)
                        .ok()
                        .map(|dt| std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(dt.timestamp().max(0) as u64))
                        .unwrap_or(mtime)
                } else {
                    mtime
                }
            } else {
                mtime
            }
        } else {
            mtime
        };
        candidates.push((p, age_base, size));
    }

    // Sort oldest first for age prune; we'll sort by newest later for keep-N.
    candidates.sort_by_key(|(_, t, _)| *t);

    let mut report = PruneReport::default();
    let mut kept: Vec<(PathBuf, std::time::SystemTime, u64)> = Vec::new();
    for (p, t, size) in candidates {
        let age = now.duration_since(t).unwrap_or_default();
        if age > max_age {
            if fs::remove_dir_all(&p).is_ok() {
                report.removed_dirs = report.removed_dirs.saturating_add(1);
                report.freed_bytes = report.freed_bytes.saturating_add(size);
            }
        } else {
            kept.push((p, t, size));
        }
    }

    if keep_newest > 0 && kept.len() > keep_newest {
        // Drop oldest beyond keep_newest.
        kept.sort_by_key(|(_, t, _)| std::cmp::Reverse(*t));
        let drop_list: Vec<_> = kept.into_iter().skip(keep_newest).collect();
        for (p, _, size) in drop_list {
            if fs::remove_dir_all(&p).is_ok() {
                report.removed_dirs = report.removed_dirs.saturating_add(1);
                report.freed_bytes = report.freed_bytes.saturating_add(size);
            }
        }
    }

    let (rem_dirs, rem_bytes) = store_disk_usage(config);
    report.remaining_dirs = rem_dirs;
    report.remaining_bytes = rem_bytes;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn write_atomically_overwrites_existing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("meta.json");
        write_atomically(&path, b"first").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first");
        // Second write must succeed on Windows (replace existing).
        write_atomically(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
    }
}

