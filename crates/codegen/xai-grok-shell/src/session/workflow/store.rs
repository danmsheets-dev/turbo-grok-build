use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

use crate::session::persistence::PersistenceMsg;

use super::tracker::WorkflowRunState;
use xai_workflow::TaskQueueState;

pub(crate) const WORKFLOW_RUN_MANIFEST_VERSION: u8 = 5;
pub(crate) const MAX_RESTORED_WORKFLOW_RUNS: usize = 128;
/// Bound directory scanning separately from the valid-run cap so malformed
/// entries cannot consume the number of runs that are actually restored.
pub(crate) const MAX_SCANNED_WORKFLOW_ENTRIES: usize = 2_048;
pub(crate) const MAX_WORKFLOW_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;
pub(crate) const MAX_WORKFLOW_ARGS_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRunManifest {
    pub version: u8,
    pub state: WorkflowRunState,
    pub script_revision: u32,
    #[serde(default)]
    pub task_queue: TaskQueueState,
}

#[derive(Debug, Clone)]
pub struct RestoredWorkflowRun {
    pub manifest: WorkflowRunManifest,
    pub script: String,
    pub args: serde_json::Value,
}

#[derive(Debug, Clone)]
struct RunSource {
    script: String,
    args: serde_json::Value,
    revision: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkflowRunStore {
    session_dir: Option<PathBuf>,
    persistence_tx: mpsc::UnboundedSender<PersistenceMsg>,
    sources: Arc<parking_lot::Mutex<HashMap<String, RunSource>>>,
    task_queues: Arc<parking_lot::Mutex<HashMap<String, TaskQueueState>>>,
}

impl WorkflowRunStore {
    pub(crate) fn new(
        session_dir: Option<PathBuf>,
        persistence_tx: mpsc::UnboundedSender<PersistenceMsg>,
    ) -> Self {
        Self {
            session_dir,
            persistence_tx,
            sources: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            task_queues: Arc::new(parking_lot::Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn from_restored(
        session_dir: Option<PathBuf>,
        persistence_tx: mpsc::UnboundedSender<PersistenceMsg>,
        restored: Vec<RestoredWorkflowRun>,
    ) -> (Self, Vec<WorkflowRunState>) {
        let store = Self::new(session_dir, persistence_tx);
        let mut states = Vec::with_capacity(restored.len());
        let mut restored: Vec<(RestoredWorkflowRun, usize)> = restored
            .into_iter()
            .enumerate()
            .map(|(i, run)| (run, i))
            .collect();
        restored.sort_by(|(a, ai), (b, bi)| {
            let at = a
                .manifest
                .state
                .history
                .first()
                .map(|event| event.at.as_str())
                .unwrap_or("");
            let bt = b
                .manifest
                .state
                .history
                .first()
                .map(|event| event.at.as_str())
                .unwrap_or("");
            at.cmp(bt).then(ai.cmp(bi))
        });
        {
            let mut sources = store.sources.lock();
            for (run, _) in restored {
                let run_id = run.manifest.state.run_id.clone();
                let script = run.script;
                let mut queue_error = None;
                if run.manifest.version >= WORKFLOW_RUN_MANIFEST_VERSION {
                    let mut task_queue = run.manifest.task_queue.clone();
                    match xai_workflow::extract_meta(&script) {
                        Ok(meta) => {
                            if task_queue.tasks.is_empty() && !meta.tasks.is_empty() {
                                match xai_workflow::TaskQueueState::from_definitions(&meta.tasks) {
                                    Ok(rebuilt) => task_queue = rebuilt,
                                    Err(error) => queue_error = Some(error.to_string()),
                                }
                            } else if !task_queue.matches_definitions(&meta.tasks) {
                                queue_error = Some(
                                    "persisted task queue does not match the stored workflow script"
                                        .to_string(),
                                );
                            }
                        }
                        Err(error) => {
                            queue_error = Some(format!("invalid stored workflow script: {error}"));
                        }
                    }
                    if queue_error.is_none()
                        && let Err(error) = task_queue.validate()
                    {
                        queue_error = Some(format!("invalid persisted task queue: {error}"));
                    }
                    if queue_error.is_none() {
                        store.task_queues.lock().insert(run_id.clone(), task_queue);
                    }
                }
                sources.insert(
                    run_id,
                    RunSource {
                        script,
                        args: run.args,
                        revision: run.manifest.script_revision,
                    },
                );
                let mut state = run.manifest.state;
                if run.manifest.version < WORKFLOW_RUN_MANIFEST_VERSION
                    || state.agent_budget.is_none()
                {
                    state.status = super::tracker::WorkflowRunStatus::Interrupted;
                    state.pause_message = Some(
                        "this workflow predates agent-count accounting and cannot be resumed; start a new run"
                            .to_string(),
                    );
                    state.agent_budget = None;
                    state.agents_used = 0;
                    state.token_leases.clear();
                    state.agent_usage_incomplete = true;
                } else if let Some(error) = queue_error {
                    state.status = super::tracker::WorkflowRunStatus::Interrupted;
                    state.pause_message = Some(format!(
                        "workflow task queue could not be restored: {error}"
                    ));
                    state.result_summary = None;
                }
                states.push(state);
            }
        }
        (store, states)
    }

    pub(crate) fn register(
        &self,
        run_id: &str,
        script: &str,
        args: &serde_json::Value,
    ) -> io::Result<()> {
        validate_run_id(run_id)?;
        if self.sources.lock().contains_key(run_id) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("workflow source already registered: {run_id}"),
            ));
        }

        if let Some(run_dir) = self.run_dir(run_id) {
            let scripts_dir = run_dir.join("scripts");
            std::fs::create_dir_all(&scripts_dir)?;
            let args_json = serde_json::to_vec_pretty(args).map_err(io::Error::other)?;
            atomic_write_new(&run_dir.join("args.json"), &args_json)?;
            atomic_write_new(&script_revision_path(&run_dir, 0), script.as_bytes())?;
            atomic_write_replace(&run_dir.join("script.rhai"), script.as_bytes())?;
        }

        self.sources.lock().insert(
            run_id.to_owned(),
            RunSource {
                script: script.to_owned(),
                args: args.clone(),
                revision: 0,
            },
        );
        Ok(())
    }

    pub(crate) fn set_task_queue(&self, run_id: &str, task_queue: TaskQueueState) {
        self.task_queues
            .lock()
            .insert(run_id.to_owned(), task_queue);
    }

    pub(crate) fn task_queue_for(&self, run_id: &str) -> TaskQueueState {
        self.task_queues
            .lock()
            .get(run_id)
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn task_queue_is_initialized(&self, run_id: &str) -> bool {
        self.task_queues.lock().contains_key(run_id)
    }

    pub(crate) fn task_queues_snapshot(&self) -> HashMap<String, TaskQueueState> {
        self.task_queues.lock().clone()
    }

    fn manifest_for(&self, state: &WorkflowRunState) -> Option<WorkflowRunManifest> {
        let revision = self
            .sources
            .lock()
            .get(&state.run_id)
            .map(|source| source.revision)?;
        Some(WorkflowRunManifest {
            version: WORKFLOW_RUN_MANIFEST_VERSION,
            state: state.clone(),
            script_revision: revision,
            task_queue: self
                .task_queues
                .lock()
                .get(&state.run_id)
                .cloned()
                .unwrap_or_default(),
        })
    }

    pub(crate) fn persist_now(&self, state: &WorkflowRunState) -> io::Result<()> {
        let Some(manifest) = self.manifest_for(state) else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "workflow state has no registered resume source",
            ));
        };
        let Some(run_dir) = self.run_dir(&state.run_id) else {
            return Ok(());
        };
        let json = serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?;
        if json.len() as u64 > MAX_WORKFLOW_MANIFEST_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("workflow manifest exceeds {MAX_WORKFLOW_MANIFEST_BYTES} bytes"),
            ));
        }
        crate::session::storage::jsonl::write_workflow_manifest_sync(
            &run_dir.join("state.json"),
            &manifest.state.run_id,
            manifest.state.revision,
            manifest.version,
            &json,
        )
    }

    pub(crate) fn persist(&self, state: &WorkflowRunState) -> io::Result<()> {
        let manifest = self.manifest_for(state).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "workflow state has no registered resume source",
            )
        })?;
        self.persistence_tx
            .send(PersistenceMsg::WorkflowRunState(manifest))
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "workflow persistence channel closed",
                )
            })
    }

    pub(crate) async fn persist_ack(&self, state: &WorkflowRunState) -> io::Result<()> {
        let manifest = self.manifest_for(state).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "workflow state has no registered resume source",
            )
        })?;
        let (respond_to, response) = oneshot::channel();
        self.persistence_tx
            .send(PersistenceMsg::WorkflowRunStateAndAck {
                manifest,
                respond_to,
            })
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "workflow persistence channel closed",
                )
            })?;
        response.await.map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "workflow persistence actor dropped acknowledgement",
            )
        })?
    }

    pub(crate) fn remove(&self, run_id: &str) {
        self.sources.lock().remove(run_id);
        self.task_queues.lock().remove(run_id);
        if let Some(run_dir) = self.run_dir(run_id) {
            if let Err(error) = atomic_write_replace(&run_dir.join("cleared"), b"") {
                tracing::warn!(run_id, %error, "failed to tombstone cleared workflow run");
            }
            if let Err(error) = std::fs::remove_file(run_dir.join("state.json"))
                && error.kind() != io::ErrorKind::NotFound
            {
                tracing::warn!(run_id, %error, "failed to remove workflow manifest during clear");
            }
        }
        if self
            .persistence_tx
            .send(PersistenceMsg::DeleteWorkflowRunState(run_id.to_owned()))
            .is_err()
        {
            tracing::warn!(run_id, "workflow persistence channel closed during clear");
        }
    }

    pub(crate) fn script_for(&self, run_id: &str) -> Option<String> {
        self.sources
            .lock()
            .get(run_id)
            .map(|source| source.script.clone())
    }

    pub(crate) fn args_for(&self, run_id: &str) -> Option<serde_json::Value> {
        self.sources
            .lock()
            .get(run_id)
            .map(|source| source.args.clone())
    }

    pub(crate) fn script_copy_path(&self, run_id: &str) -> Option<PathBuf> {
        validate_run_id(run_id).ok()?;
        self.sources.lock().contains_key(run_id).then_some(())?;
        Some(self.run_dir(run_id)?.join("script.rhai"))
    }

    fn run_dir(&self, run_id: &str) -> Option<PathBuf> {
        self.session_dir
            .as_ref()
            .map(|dir| dir.join("workflows").join(run_id))
    }
}

pub(crate) fn validate_run_id(run_id: &str) -> io::Result<()> {
    if run_id.is_empty()
        || !run_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid workflow run id",
        ));
    }
    Ok(())
}

pub(crate) fn script_revision_path(run_dir: &Path, revision: u32) -> PathBuf {
    run_dir.join("scripts").join(format!("{revision:04}.rhai"))
}

fn metadata_is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return metadata.file_attributes() & 0x0400_0000 != 0;
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub(crate) fn reject_reparse_components(path: &Path) -> io::Result<()> {
    for component in path.ancestors() {
        match std::fs::symlink_metadata(component) {
            Ok(metadata) if metadata_is_reparse_point(&metadata) => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "workflow path contains a reparse point: {}",
                        component.display()
                    ),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(crate) fn read_bounded_nofollow(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    reject_reparse_components(path)?;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata_is_reparse_point(&metadata) || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "workflow artifact is not a regular file: {}",
                path.display()
            ),
        ));
    }
    if metadata.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "workflow artifact exceeds {limit} bytes: {}",
                path.display()
            ),
        ));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_OPEN_REPARSE_POINT prevents a symlink/reparse-point
        // follow between metadata validation and open.
        options.custom_flags(0x0020_0000);
    }
    let file = options.open(path)?;
    let opened = file.metadata()?;
    if metadata_is_reparse_point(&opened) || !opened.is_file() || opened.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("workflow artifact changed during open: {}", path.display()),
        ));
    }
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "workflow artifact exceeds {limit} bytes: {}",
                path.display()
            ),
        ));
    }
    Ok(bytes)
}

fn atomic_write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("immutable workflow file already exists: {}", path.display()),
        ));
    }
    atomic_write(path, bytes, false)
}

fn atomic_write_replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    atomic_write(path, bytes, true)
}

fn atomic_write(path: &Path, bytes: &[u8], replace: bool) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "workflow path has no parent")
    })?;
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "workflow path is not UTF-8"))?;
    let tmp = parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        uuid::Uuid::now_v7().simple()
    ));
    let result = crate::session::storage::with_workflow_state_write_lock(|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if !replace && path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("immutable workflow file already exists: {}", path.display()),
            ));
        }
        if replace {
            crate::session::storage::replace_file_atomic(&tmp, path)
        } else {
            std::fs::rename(&tmp, path)
        }
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::workflow::tracker::WorkflowTracker;

    #[test]
    fn script_and_args_are_immutable() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let store = WorkflowRunStore::new(Some(dir.path().to_path_buf()), tx);
        let args = serde_json::json!({"objective": "ship"});

        store.register("wf_1", "complete(1);", &args).unwrap();
        std::fs::write(
            dir.path().join("workflows/wf_1/script.rhai"),
            "complete(2);",
        )
        .unwrap();

        let run_dir = dir.path().join("workflows/wf_1");
        assert_eq!(
            std::fs::read_to_string(run_dir.join("scripts/0000.rhai")).unwrap(),
            "complete(1);"
        );
        assert!(!run_dir.join("scripts/0001.rhai").exists());
        assert_eq!(store.script_for("wf_1").as_deref(), Some("complete(1);"));
        assert_eq!(store.args_for("wf_1"), Some(args));
    }

    #[tokio::test]
    async fn acknowledged_persist_returns_storage_failure() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let store = WorkflowRunStore::new(None, tx);
        store
            .register("wf_1", "complete(1);", &serde_json::json!({}))
            .unwrap();
        let state = WorkflowTracker::default().start_run(
            "wf_1".into(),
            "demo".into(),
            "objective".into(),
            Vec::new(),
            None,
            None,
        );
        let writer = tokio::spawn(async move {
            let Some(PersistenceMsg::WorkflowRunStateAndAck { respond_to, .. }) = rx.recv().await
            else {
                panic!("expected acknowledged workflow manifest");
            };
            let _ = respond_to.send(Err(io::Error::other("disk full")));
        });

        assert_eq!(
            store.persist_ack(&state).await.unwrap_err().to_string(),
            "disk full"
        );
        writer.await.unwrap();
    }

    #[test]
    fn output_budget_manifest_is_interrupted_after_total_budget_upgrade() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let state = WorkflowTracker::default().start_run(
            "wf_legacy".into(),
            "demo".into(),
            "objective".into(),
            Vec::new(),
            Some(1_000),
            None,
        );
        let restored = RestoredWorkflowRun {
            manifest: WorkflowRunManifest {
                version: WORKFLOW_RUN_MANIFEST_VERSION - 1,
                state,
                script_revision: 0,
                task_queue: xai_workflow::TaskQueueState::default(),
            },
            script: "complete(1);".into(),
            args: serde_json::json!({}),
        };

        let (_store, states) = WorkflowRunStore::from_restored(None, tx, vec![restored]);
        let state = &states[0];
        assert_eq!(
            state.status,
            crate::session::workflow::tracker::WorkflowRunStatus::Interrupted
        );
        assert_eq!(state.agent_budget, None);
        assert!(state.agent_usage_incomplete);
        assert!(
            state
                .pause_message
                .as_deref()
                .is_some_and(|message| message.contains("predates agent-count accounting"))
        );
    }
}
