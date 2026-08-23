//! Workspace-level standing job index: `{workspace}/.grok/schedules.json`.
//!
//! Live `SchedulerState` is process memory (plus optional session tool-state).
//! This file is what `turbo schedule list|show|cancel` reads with Turbo closed,
//! and what the actor reloads on start so `/schedule` jobs survive a pager
//! restart. `/loop` jobs are not written here.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::types::ScheduledTask;

pub const INDEX_REL: &str = ".grok/schedules.json";
const INDEX_VERSION: u32 = 1;
const MAX_CANCELLED: usize = 200;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ScheduleIndex {
    #[serde(default = "index_version")]
    pub version: u32,
    #[serde(default)]
    pub tasks: Vec<ScheduledTask>,
    /// Ids cancelled from the CLI (or actor delete) so a later persist cannot
    /// resurrect them from in-memory state while Turbo is still running.
    #[serde(default)]
    pub cancelled: Vec<String>,
}

fn index_version() -> u32 {
    INDEX_VERSION
}

pub fn schedule_index_path(workspace: &Path) -> PathBuf {
    workspace.join(INDEX_REL)
}

pub fn load_schedule_index(workspace: &Path) -> io::Result<ScheduleIndex> {
    let path = schedule_index_path(workspace);
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(ScheduleIndex {
                version: INDEX_VERSION,
                ..ScheduleIndex::default()
            });
        }
        Err(e) => return Err(e),
    };
    let mut index: ScheduleIndex = serde_json::from_str(&raw).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid {INDEX_REL}: {e}"),
        )
    })?;
    if index.version == 0 {
        index.version = INDEX_VERSION;
    }
    Ok(index)
}

pub fn save_schedule_index(workspace: &Path, index: &ScheduleIndex) -> io::Result<()> {
    let path = schedule_index_path(workspace);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        if parent
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "refusing to write through a symlinked .grok folder",
            ));
        }
    }
    let body = serde_json::to_vec_pretty(index)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, body)?;
    match fs::rename(&tmp, &path) {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = fs::remove_file(&path);
            fs::rename(&tmp, &path)
        }
    }
}

/// Replace on-disk live tasks with `live`, keeping cancelled ids that are not
/// currently running (so a CLI cancel while Turbo is up is not overwritten).
pub fn upsert_live_tasks(workspace: &Path, live: &[ScheduledTask]) -> io::Result<()> {
    let mut index = load_schedule_index(workspace)?;
    let cancelled: std::collections::HashSet<&str> =
        index.cancelled.iter().map(String::as_str).collect();
    index.tasks = live
        .iter()
        .filter(|t| !cancelled.contains(t.id.as_str()))
        .cloned()
        .collect();
    index.version = INDEX_VERSION;
    save_schedule_index(workspace, &index)
}

pub fn cancel_task(workspace: &Path, id: &str) -> io::Result<bool> {
    let id = id.trim();
    if id.is_empty() {
        return Ok(false);
    }
    let mut index = load_schedule_index(workspace)?;
    let existed = index.tasks.iter().any(|t| t.id == id);
    index.tasks.retain(|t| t.id != id);
    if !index.cancelled.iter().any(|c| c == id) {
        index.cancelled.push(id.to_string());
        if index.cancelled.len() > MAX_CANCELLED {
            let drop_n = index.cancelled.len() - MAX_CANCELLED;
            index.cancelled.drain(0..drop_n);
        }
    }
    save_schedule_index(workspace, &index)?;
    Ok(existed)
}

pub fn uncancel_task(workspace: &Path, id: &str) -> io::Result<()> {
    let mut index = load_schedule_index(workspace)?;
    let before = index.cancelled.len();
    index.cancelled.retain(|c| c != id);
    if index.cancelled.len() != before {
        save_schedule_index(workspace, &index)?;
    }
    Ok(())
}

pub fn visible_tasks(index: &ScheduleIndex) -> Vec<&ScheduledTask> {
    let cancelled: std::collections::HashSet<&str> =
        index.cancelled.iter().map(String::as_str).collect();
    index
        .tasks
        .iter()
        .filter(|t| !cancelled.contains(t.id.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sample(id: &str) -> ScheduledTask {
        let mut t = ScheduledTask::new(3600, "search rust".into(), true, true);
        t.id = id.into();
        t.apply_standing();
        t.title = Some("rust".into());
        t.created_at = Utc::now();
        t
    }

    #[test]
    fn round_trip_and_cli_cancel_is_sticky() {
        let dir = tempfile::TempDir::new().unwrap();
        let live = vec![sample("aaa"), sample("bbb")];
        upsert_live_tasks(dir.path(), &live).unwrap();
        let loaded = load_schedule_index(dir.path()).unwrap();
        assert_eq!(visible_tasks(&loaded).len(), 2);

        assert!(cancel_task(dir.path(), "aaa").unwrap());
        // Actor persist of the still-running copy must not resurrect aaa.
        upsert_live_tasks(dir.path(), &live).unwrap();
        let after = load_schedule_index(dir.path()).unwrap();
        let ids: Vec<_> = visible_tasks(&after)
            .iter()
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(ids, vec!["bbb"]);
        assert!(after.cancelled.iter().any(|c| c == "aaa"));

        uncancel_task(dir.path(), "aaa").unwrap();
        upsert_live_tasks(dir.path(), &live).unwrap();
        let restored = load_schedule_index(dir.path()).unwrap();
        assert_eq!(visible_tasks(&restored).len(), 2);
    }

    #[test]
    fn missing_file_is_empty_index() {
        let dir = tempfile::TempDir::new().unwrap();
        let idx = load_schedule_index(dir.path()).unwrap();
        assert!(idx.tasks.is_empty());
        assert!(idx.cancelled.is_empty());
    }
}
