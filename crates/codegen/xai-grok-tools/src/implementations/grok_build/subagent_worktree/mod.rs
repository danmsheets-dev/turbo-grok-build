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
    pub baseline_ref: Option<String>,
    #[serde(default)]
    pub patch_path: Option<String>,
    #[serde(default)]
    pub worktree_state: Option<String>,
    #[serde(default)]
    pub child_cwd: Option<String>,
    #[serde(default)]
    pub diffstat: Option<String>,
    #[serde(default)]
    pub changed_paths: Option<Vec<String>>,
    /// Relative path prefixes the child may write / parent may land.
    /// When non-empty, land refuses paths outside these prefixes.
    #[serde(default)]
    pub allowed_paths: Option<Vec<String>>,
}

/// Default max files for land without `force` (dirty-tree mega-patch guard).
pub const DEFAULT_LAND_MAX_FILES: u32 = 50;

/// Parse `files_changed` from a compact diffstat like `235 files, +22578/-51`.
pub fn parse_files_changed_from_diffstat(stat: &str) -> Option<u32> {
    let first = stat.split(',').next()?.trim();
    let n = first.split_whitespace().next()?;
    n.parse().ok()
}

/// Land safety: refuse huge unintended patches unless `force`.
pub fn land_size_guard(
    files_changed: Option<u32>,
    force: bool,
    max_files: u32,
) -> Result<(), xai_tool_runtime::ToolError> {
    if force {
        return Ok(());
    }
    let Some(n) = files_changed else {
        return Ok(());
    };
    if n > max_files {
        return Err(xai_tool_runtime::ToolError::custom(
            "land_too_large",
            format!(
                "Refusing land: agent delta touches {n} files (limit {max_files}). \
                 On dirty parents without a spawn baseline this often includes \
                 unrelated untracked files. Review with `turbo subagent diff <id>` \
                 / `diff_subagent`, then re-run with force=true only if intentional."
            ),
        ));
    }
    Ok(())
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
            // Semantic union-merge for kit manifests (densify parallel land).
            let data = if is_manifest_json_path(rel) {
                match tokio::fs::read_to_string(&dest).await {
                    Ok(parent_text) => {
                        match union_merge_manifest_json(&parent_text, data) {
                            Some(merged) => merged,
                            None => data.to_owned(),
                        }
                    }
                    Err(_) => data.to_owned(),
                }
            } else {
                data.to_owned()
            };
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

/// `assets/manifest/<kit>.json` (and nested under that tree).
pub fn is_manifest_json_path(rel: &str) -> bool {
    let n = rel.replace('\\', "/");
    let n = n.trim_start_matches("./");
    n.starts_with("assets/manifest/") && n.ends_with(".json")
}

/// Name-keyed union merge for densify kit manifests.
///
/// Supports:
/// - JSON array of objects with a `name` field (preferred)
/// - JSON object map keyed by stem/name
///
/// Child wins on key collision for the same name. Returns `None` when shapes
/// don't match or parsing fails (caller falls back to overwrite).
pub fn union_merge_manifest_json(parent: &str, child: &str) -> Option<String> {
    let parent_v: serde_json::Value = serde_json::from_str(parent).ok()?;
    let child_v: serde_json::Value = serde_json::from_str(child).ok()?;
    let merged = match (&parent_v, &child_v) {
        (serde_json::Value::Array(pa), serde_json::Value::Array(ca)) => {
            let mut by_name: indexmap::IndexMap<String, serde_json::Value> =
                indexmap::IndexMap::new();
            let mut anonymous: Vec<serde_json::Value> = Vec::new();
            for item in pa.iter().chain(ca.iter()) {
                match item.get("name").and_then(|n| n.as_str()) {
                    Some(name) if !name.is_empty() => {
                        by_name.insert(name.to_owned(), item.clone());
                    }
                    _ => anonymous.push(item.clone()),
                }
            }
            let mut out: Vec<serde_json::Value> = by_name.into_values().collect();
            out.extend(anonymous);
            serde_json::Value::Array(out)
        }
        (serde_json::Value::Object(po), serde_json::Value::Object(co)) => {
            let mut out = po.clone();
            for (k, v) in co {
                out.insert(k.clone(), v.clone());
            }
            serde_json::Value::Object(out)
        }
        _ => return None,
    };
    serde_json::to_string_pretty(&merged).ok()
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

/// RC13 Wave A: always terminal-clean meta after discard.
///
/// Writes:
/// - `land_status = "discarded"`
/// - `worktree_state = "cleaned"`
/// - `worktree_path = null` (never advertise a live tree after discard)
/// - if `status == "running"` → `status = "cancelled"` (never leave running)
/// - if `snapshot_dropped` → `snapshot_ref = null` (honest with tool output)
///
/// Best-effort: silent on missing/unparseable meta (callers still succeed when
/// the worktree was already gone).
pub async fn update_meta_discarded(meta_path: &Path, snapshot_dropped: bool) {
    let Ok(raw) = tokio::fs::read_to_string(meta_path).await else {
        return;
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    obj.insert(
        "land_status".to_owned(),
        serde_json::Value::String("discarded".to_owned()),
    );
    obj.insert(
        "worktree_state".to_owned(),
        serde_json::Value::String("cleaned".to_owned()),
    );
    obj.insert("worktree_path".to_owned(), serde_json::Value::Null);
    // Never leave status=running after discard.
    let status = obj.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if status.is_empty() || status == "running" {
        obj.insert(
            "status".to_owned(),
            serde_json::Value::String("cancelled".to_owned()),
        );
    }
    if snapshot_dropped {
        obj.insert("snapshot_ref".to_owned(), serde_json::Value::Null);
    }
    let Ok(pretty) = serde_json::to_string_pretty(&value) else {
        return;
    };
    let _ = tokio::fs::write(meta_path, pretty).await;
}

// ───────────────────────────────────────────────────────────────────────────
// Path allowlists (`allowed_paths` on spawn / meta.json)
// ───────────────────────────────────────────────────────────────────────────

/// Normalize a relative path for allowlist matching.
///
/// - Converts `\` → `/`
/// - Strips leading `./` segments
/// - Collapses `.` and resolves `..` (rejects escape above the root)
/// - Rejects absolute paths (Unix `/…`, Windows drive `C:…`, UNC `//…`)
///
/// Returns `None` when the path cannot be safely treated as a relative
/// in-repo path.
pub fn normalize_allowlist_path(path: &str) -> Option<String> {
    let mut s = path.trim().replace('\\', "/");
    if s.is_empty() {
        return None;
    }
    // Absolute / drive / UNC
    if s.starts_with('/') || s.starts_with("//") {
        return None;
    }
    if s.len() >= 2 && s.as_bytes()[1] == b':' {
        // Windows drive letter
        return None;
    }
    while s.starts_with("./") {
        s = s[2..].to_owned();
    }
    if s == "." {
        return None;
    }
    // Drop trailing slash for segment processing (prefix matching re-adds as needed)
    let trailing_slash = s.ends_with('/') && s.len() > 1;
    if trailing_slash {
        s.pop();
    }
    let mut stack: Vec<&str> = Vec::new();
    for part in s.split('/') {
        match part {
            "" | "." => continue,
            ".." => {
                if stack.is_empty() {
                    return None; // escapes root
                }
                stack.pop();
            }
            other => stack.push(other),
        }
    }
    if stack.is_empty() {
        return None;
    }
    Some(stack.join("/"))
}

/// Effective allowlist from meta: non-empty cleaned prefixes, or `None` (unrestricted).
pub fn effective_allowed_paths(meta: &SubagentMetaView) -> Option<Vec<String>> {
    let raw = meta.allowed_paths.as_ref()?;
    let mut out = Vec::new();
    for p in raw {
        if let Some(n) = normalize_allowlist_path(p) {
            out.push(n);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// True when `path` is under any allowlist prefix (exact file or directory prefix).
///
/// Prefix `"crates/foo"` matches `crates/foo`, `crates/foo/bar.rs`, but not
/// `crates/foobar`. Empty / missing allowlist is unrestricted (always true).
pub fn path_is_allowed(path: &str, allowed: &[String]) -> bool {
    if allowed.is_empty() {
        return true;
    }
    let Some(norm) = normalize_allowlist_path(path) else {
        return false;
    };
    for pref in allowed {
        if norm == *pref || norm.starts_with(&format!("{pref}/")) {
            return true;
        }
    }
    false
}

/// Partition paths into (allowed, denied) under the given allowlist.
/// When `allowed` is empty, every path is allowed.
pub fn partition_by_allowlist(
    paths: &[String],
    allowed: &[String],
) -> (Vec<String>, Vec<String>) {
    if allowed.is_empty() {
        return (paths.to_vec(), Vec::new());
    }
    let mut ok = Vec::new();
    let mut denied = Vec::new();
    for p in paths {
        if path_is_allowed(p, allowed) {
            ok.push(p.clone());
        } else {
            denied.push(p.clone());
        }
    }
    (ok, denied)
}

/// If meta has a non-empty allowlist and `paths` contains anything outside it,
/// return a clear land-refusal error. Otherwise `Ok(())`.
pub fn refuse_land_outside_allowlist(
    meta: &SubagentMetaView,
    paths: &[String],
) -> Result<(), xai_tool_runtime::ToolError> {
    let Some(allowed) = effective_allowed_paths(meta) else {
        return Ok(());
    };
    let (_ok, denied) = partition_by_allowlist(paths, &allowed);
    if denied.is_empty() {
        return Ok(());
    }
    let preview: Vec<&str> = denied.iter().take(8).map(String::as_str).collect();
    let more = if denied.len() > 8 {
        format!(" (+{} more)", denied.len() - 8)
    } else {
        String::new()
    };
    Err(xai_tool_runtime::ToolError::custom(
        "path_allowlist_violation",
        format!(
            "land refused: {} path(s) outside allowed_paths {:?}: {}{more}. \
             Re-spawn with a wider allowlist, land only in-allowlist changes, \
             or omit allowed_paths for unrestricted land.",
            denied.len(),
            allowed,
            preview.join(", "),
        ),
    ))
}

#[cfg(test)]
mod manifest_merge_tests {
    use super::*;

    #[test]
    fn is_manifest_path_detects_kit_json() {
        assert!(is_manifest_json_path("assets/manifest/hull.json"));
        assert!(is_manifest_json_path("assets/manifest/company_prop.json"));
        assert!(!is_manifest_json_path("assets/models/hull.glb"));
        assert!(!is_manifest_json_path("src/main.rs"));
    }

    #[test]
    fn union_merge_arrays_by_name_child_wins() {
        let parent = r#"[
          {"name": "a", "budget_class": "small"},
          {"name": "b", "budget_class": "mid"}
        ]"#;
        let child = r#"[
          {"name": "b", "budget_class": "large", "built_by": "wave4"},
          {"name": "c", "budget_class": "tiny"}
        ]"#;
        let merged = union_merge_manifest_json(parent, child).expect("merge");
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        let b = arr.iter().find(|x| x["name"] == "b").unwrap();
        assert_eq!(b["budget_class"], "large");
        assert_eq!(b["built_by"], "wave4");
        assert!(arr.iter().any(|x| x["name"] == "a"));
        assert!(arr.iter().any(|x| x["name"] == "c"));
    }

    #[test]
    fn union_merge_objects_shallow() {
        let parent = r#"{"a": 1, "b": 2}"#;
        let child = r#"{"b": 9, "c": 3}"#;
        let merged = union_merge_manifest_json(parent, child).unwrap();
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], 9);
        assert_eq!(v["c"], 3);
    }
}

#[cfg(test)]
mod allowlist_tests {
    use super::*;

    #[test]
    fn normalize_strips_dot_slash_and_backslashes() {
        assert_eq!(
            normalize_allowlist_path(r".\crates\foo\bar.rs").as_deref(),
            Some("crates/foo/bar.rs")
        );
        assert_eq!(
            normalize_allowlist_path("./docs/a.md").as_deref(),
            Some("docs/a.md")
        );
    }

    #[test]
    fn normalize_rejects_escape_and_absolute() {
        assert!(normalize_allowlist_path("../secret").is_none());
        assert!(normalize_allowlist_path("a/../../b").is_none());
        assert!(normalize_allowlist_path("/etc/passwd").is_none());
        assert!(normalize_allowlist_path("C:\\Windows").is_none());
    }

    #[test]
    fn normalize_resolves_internal_dotdot() {
        assert_eq!(
            normalize_allowlist_path("crates/foo/../bar/x.rs").as_deref(),
            Some("crates/bar/x.rs")
        );
    }

    #[test]
    fn path_is_allowed_prefix_not_partial_name() {
        let allowed = vec!["crates/foo".to_string()];
        assert!(path_is_allowed("crates/foo/src/lib.rs", &allowed));
        assert!(path_is_allowed("crates/foo", &allowed));
        assert!(!path_is_allowed("crates/foobar/x.rs", &allowed));
        assert!(!path_is_allowed("docs/a.md", &allowed));
    }

    #[test]
    fn partition_and_refuse() {
        let meta = SubagentMetaView {
            subagent_id: "s".into(),
            parent_session_id: None,
            status: None,
            worktree_path: None,
            snapshot_ref: None,
            baseline_ref: None,
            patch_path: None,
            worktree_state: None,
            child_cwd: None,
            diffstat: None,
            changed_paths: None,
            allowed_paths: Some(vec!["crates/a/".into(), "docs".into()]),
        };
        let paths = vec![
            "crates/a/mod.rs".into(),
            "crates/b/other.rs".into(),
            "docs/readme.md".into(),
        ];
        let allowed = effective_allowed_paths(&meta).unwrap();
        let (ok, denied) = partition_by_allowlist(&paths, &allowed);
        assert_eq!(ok, vec!["crates/a/mod.rs", "docs/readme.md"]);
        assert_eq!(denied, vec!["crates/b/other.rs"]);
        assert!(refuse_land_outside_allowlist(&meta, &paths).is_err());
        assert!(refuse_land_outside_allowlist(&meta, &ok).is_ok());

        let unrestricted = SubagentMetaView {
            allowed_paths: None,
            ..meta.clone()
        };
        assert!(refuse_land_outside_allowlist(&unrestricted, &paths).is_ok());
    }
}

