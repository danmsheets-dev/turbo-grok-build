//! Shared helpers for parent-orchestrator subagent land/diff tools.
//!
//! Resolves durable subagent metadata from
//! `{sessions_cwd}/{session_id}/subagents/{id}/meta.json` and drives git-based
//! diff/land against a live worktree, snapshot ref, or exported patch.

pub mod diff;
pub mod discard;
pub mod land;

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::implementations::grok_build::task::types::SessionIdResource;
use crate::types::tool_metadata::{resolve_cwd, shared_resources};
use crate::util::grok_home::sessions_cwd_dir;

/// Fields read from session `subagents/<id>/meta.json` for land/diff.
///
/// Deliberately a superset-tolerant view of shell `SubagentMeta` so tools keep
/// working as WP2 adds `patch_path` / `worktree_state` / etc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentMetaView {
    pub subagent_id: String,
    #[serde(default)]
    pub parent_session_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub worktree_path: Option<String>,
    #[serde(default)]
    pub snapshot_ref: Option<String>,
    #[serde(default)]
    pub patch_path: Option<String>,
    #[serde(default)]
    pub worktree_state: Option<String>,
    #[serde(default)]
    pub child_cwd: Option<String>,
}

/// Resolved artifacts for a subagent after reading meta + probing the filesystem.
#[derive(Debug, Clone)]
pub struct ResolvedSubagentWork {
    pub subagent_id: String,
    pub meta: SubagentMetaView,
    pub meta_path: PathBuf,
    pub subagent_dir: PathBuf,
    /// Live worktree directory when it still exists on disk.
    pub live_worktree: Option<PathBuf>,
    pub snapshot_ref: Option<String>,
    pub patch_path: Option<PathBuf>,
    /// Parent repo root (git top-level of the orchestrator cwd).
    pub parent_git_root: PathBuf,
    pub parent_cwd: PathBuf,
    pub session_id: String,
}

/// Land/apply conflict policy matching [`xai_grok_workspace_types::rpc::worktree::ApplyMode`].
///
/// Tool default is **merge** (fail closed on conflict). Workspace wire default
/// remains overwrite; land tools intentionally diverge for safety.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LandMode {
    /// Three-way style merge: only apply when parent file matches the spawn
    /// base (or is unchanged). Conflicts fail closed — nothing is written.
    #[default]
    Merge,
    /// Replace parent files with child content unconditionally.
    Overwrite,
}

impl LandMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Overwrite => "overwrite",
        }
    }

    pub fn from_input(raw: Option<&str>) -> Result<Self, String> {
        match raw.map(str::trim).filter(|s| !s.is_empty()) {
            None => Ok(Self::Merge),
            Some("merge") => Ok(Self::Merge),
            Some("overwrite") => Ok(Self::Overwrite),
            Some(other) => Err(format!(
                "invalid mode `{other}`: expected `merge` (default) or `overwrite`"
            )),
        }
    }
}

/// Convert tool land mode to workspace ApplyMode for live-worktree apply.
pub fn to_apply_mode(mode: LandMode) -> xai_grok_workspace_types::rpc::worktree::ApplyMode {
    match mode {
        LandMode::Merge => xai_grok_workspace_types::rpc::worktree::ApplyMode::Merge,
        LandMode::Overwrite => xai_grok_workspace_types::rpc::worktree::ApplyMode::Overwrite,
    }
}

/// Optional host-injected backend that lands a **live** worktree via
/// `workspace.apply_worktree` (or an equivalent host path).
///
/// When absent, land falls back to in-process git/fs apply that mirrors the
/// same Merge/Overwrite semantics.
pub struct LiveWorktreeApplyBackend(
    pub std::sync::Arc<
        dyn Fn(
                String,
                xai_grok_workspace_types::rpc::worktree::ApplyMode,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<
                            Output = Result<
                                xai_grok_workspace_types::rpc::worktree::ApplyWorktreeResponse,
                                String,
                            >,
                        > + Send,
                >,
            > + Send
            + Sync,
    >,
);

impl std::fmt::Debug for LiveWorktreeApplyBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveWorktreeApplyBackend").finish()
    }
}

crate::register_resource!(
    "grok_build",
    "LiveWorktreeApplyBackend",
    LiveWorktreeApplyBackend
);

/// Validate `subagent_id` is a single path segment (no traversal).
pub fn validate_subagent_id(id: &str) -> Result<(), xai_tool_runtime::ToolError> {
    let id = id.trim();
    if id.is_empty() {
        return Err(xai_tool_runtime::ToolError::custom(
            "invalid_subagent_id",
            "subagent_id must not be empty",
        ));
    }
    if id.contains('/')
        || id.contains('\\')
        || id.contains("..")
        || id == "."
        || id.chars().any(|c| c.is_control())
    {
        return Err(xai_tool_runtime::ToolError::custom(
            "invalid_subagent_id",
            format!("subagent_id `{id}` is not a valid id (no path separators)"),
        ));
    }
    Ok(())
}

/// Load and resolve subagent work artifacts for land/diff.
pub async fn resolve_subagent_work(
    ctx: &xai_tool_runtime::ToolCallContext,
    subagent_id: &str,
) -> Result<ResolvedSubagentWork, xai_tool_runtime::ToolError> {
    validate_subagent_id(subagent_id)?;
    let resources = shared_resources(ctx)?;
    let parent_cwd = resolve_cwd(ctx, &resources).await?;
    let session_id = {
        let res = resources.lock().await;
        res.get::<SessionIdResource>()
            .map(|s| s.0.clone())
            .ok_or_else(|| {
                xai_tool_runtime::ToolError::custom(
                    "missing_session_id",
                    "SessionIdResource is required to resolve subagents/<id>/meta.json",
                )
            })?
    };

    let cwd_str = parent_cwd.to_string_lossy().into_owned();
    let subagent_dir = sessions_cwd_dir(&cwd_str)
        .join(&session_id)
        .join("subagents")
        .join(subagent_id.trim());
    let meta_path = subagent_dir.join("meta.json");

    if !meta_path.is_file() {
        return Err(xai_tool_runtime::ToolError::custom(
            "subagent_not_found",
            format!(
                "no meta.json for subagent_id `{}` at {}",
                subagent_id.trim(),
                meta_path.display()
            ),
        ));
    }

    let raw = tokio::fs::read_to_string(&meta_path).await.map_err(|e| {
        xai_tool_runtime::ToolError::custom(
            "meta_read_failed",
            format!("failed to read {}: {e}", meta_path.display()),
        )
    })?;
    let meta: SubagentMetaView = serde_json::from_str(&raw).map_err(|e| {
        xai_tool_runtime::ToolError::custom(
            "meta_parse_failed",
            format!("failed to parse {}: {e}", meta_path.display()),
        )
    })?;

    let live_worktree = meta
        .worktree_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_dir());

    let snapshot_ref = meta
        .snapshot_ref
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    let patch_path = meta
        .patch_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| {
            let candidate = subagent_dir.join("changes.patch");
            candidate.is_file().then_some(candidate)
        });

    if live_worktree.is_none() && snapshot_ref.is_none() && patch_path.is_none() {
        return Err(xai_tool_runtime::ToolError::custom(
            "no_subagent_work",
            format!(
                "subagent `{}` has no live worktree, snapshot_ref, or changes.patch to act on \
                 (worktree_state={:?})",
                subagent_id.trim(),
                meta.worktree_state
            ),
        ));
    }

    let parent_git_root = find_git_root(&parent_cwd).await.map_err(|e| {
        xai_tool_runtime::ToolError::custom(
            "not_a_git_repo",
            format!(
                "parent cwd {} is not inside a git repository: {e}",
                parent_cwd.display()
            ),
        )
    })?;

    Ok(ResolvedSubagentWork {
        subagent_id: subagent_id.trim().to_owned(),
        meta,
        meta_path,
        subagent_dir,
        live_worktree,
        snapshot_ref,
        patch_path,
        parent_git_root,
        parent_cwd,
        session_id,
    })
}

pub async fn find_git_root(cwd: &Path) -> Result<PathBuf, String> {
    let out = git_capture(cwd, &["rev-parse", "--show-toplevel"]).await?;
    let root = out.trim();
    if root.is_empty() {
        return Err("empty git root".into());
    }
    Ok(PathBuf::from(root))
}

pub async fn git_capture(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("failed to spawn git {}: {e}", args.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "git {} failed ({}): {}{}",
            args.join(" "),
            output.status,
            stderr.trim(),
            if stdout.trim().is_empty() {
                String::new()
            } else {
                format!(" | {}", stdout.trim())
            }
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("git output not utf-8: {e}"))
}

/// Like [`git_capture`] but returns Ok with stdout even on non-zero exit
/// (caller inspects status via the second return value when needed).
pub async fn git_capture_status(cwd: &Path, args: &[&str]) -> Result<(bool, String, String), String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("failed to spawn git {}: {e}", args.join(" ")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Ok((output.status.success(), stdout, stderr))
}

/// Truncate model-facing diff text to the default tool output budget.
pub fn truncate_diff_text(text: &str) -> (String, bool) {
    const LIMIT: usize = crate::DEFAULT_TOOL_OUTPUT_CHARS;
    if text.len() <= LIMIT {
        return (text.to_owned(), false);
    }
    let cut = crate::util::floor_char_boundary(text, LIMIT);
    (
        format!(
            "{}\n\n…[diff truncated at {LIMIT} chars; use git show / patch_path for full content]",
            &text[..cut]
        ),
        true,
    )
}

/// Parse `git diff --name-status` output into relative paths (new path preferred).
pub fn parse_name_status(stdout: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in stdout.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split('\t');
        let status = parts.next().unwrap_or("");
        let path = if status.starts_with('R') || status.starts_with('C') {
            // rename/copy: status\told\tnew
            parts.nth(1).or_else(|| parts.next())
        } else {
            parts.next()
        };
        if let Some(p) = path.filter(|p| !p.is_empty()) {
            files.push(p.to_owned());
        }
    }
    files.sort();
    files.dedup();
    files
}

/// Read a blob at `rev:path` from a git repo. Returns None if the path is
/// missing at that revision (deleted / never existed).
pub async fn git_show_blob(repo: &Path, rev: &str, path: &str) -> Option<String> {
    let spec = format!("{rev}:{path}");
    match git_capture(repo, &["show", &spec]).await {
        Ok(s) => Some(s),
        Err(_) => None,
    }
}

/// Write or delete a file under `root` relative to `rel`.
pub async fn apply_file_content(root: &Path, rel: &str, content: Option<&str>) -> Result<(), String> {
    let dest = root.join(rel);
    match content {
        Some(data) => {
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
            }
            tokio::fs::write(&dest, data)
                .await
                .map_err(|e| format!("write {}: {e}", dest.display()))
        }
        None => {
            if dest.exists() {
                tokio::fs::remove_file(&dest)
                    .await
                    .map_err(|e| format!("remove {}: {e}", dest.display()))?;
            }
            Ok(())
        }
    }
}

/// Best-effort update of land_status in meta.json after a land attempt.
pub async fn update_meta_land_status(meta_path: &Path, land_status: &str) {
    let Ok(raw) = tokio::fs::read_to_string(meta_path).await else {
        return;
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "land_status".to_owned(),
            serde_json::Value::String(land_status.to_owned()),
        );
    }
    let Ok(pretty) = serde_json::to_string_pretty(&value) else {
        return;
    };
    let _ = tokio::fs::write(meta_path, pretty).await;
}

