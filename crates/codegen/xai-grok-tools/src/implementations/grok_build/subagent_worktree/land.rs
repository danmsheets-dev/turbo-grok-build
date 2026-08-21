//! `land_subagent` — apply a subagent's work into the parent repository.
//!
//! Priority:
//! 1. Live worktree → `workspace.apply_worktree` (if
//!    [`super::LiveWorktreeApplyBackend`] is injected) else in-process merge
//!    that mirrors ApplyMode Merge/Overwrite
//! 2. snapshot_ref → per-file checkout from the ref (merge = fail closed)
//! 3. patch_path → `git apply` (merge uses `--3way` check; fail closed)

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    LandMode, LiveWorktreeApplyBackend, apply_file_content_with_union, effective_allowed_paths,
    filter_harness_land_paths, git_capture, git_capture_status, git_show_blob, parse_name_status,
    refuse_land_outside_allowlist, resolve_subagent_work, to_apply_mode, update_meta_land_status,
};
use crate::implementations::grok_build::task::TaskTool;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};
use crate::types::tool_metadata::shared_resources;

pub const LAND_SUBAGENT_TOOL_NAME: &str = "land_subagent";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LandSubagentInput {
    #[schemars(
        description = "Required. Subagent id from `task` completion. Loads subagents/<id>/meta.json (snapshot_ref, worktree_path, patch_path)."
    )]
    pub subagent_id: String,

    #[serde(default)]
    #[schemars(
        description = "Land mode: `merge` (default) or `overwrite`. Merge fails closed on conflict — no silent parent overwrite. Overwrite replaces parent files with the child's content for all changed paths."
    )]
    pub mode: Option<String>,

    /// Bypass the large-patch safety guard (default max 50 files).
    #[serde(default)]
    #[schemars(
        description = "When true, allow landing agent deltas larger than the safety limit (default 50 files). Use only after reviewing `diff_subagent` / `turbo subagent diff`."
    )]
    pub force: Option<bool>,

    /// Only land paths that do **not** already exist on the parent (new files).
    /// Skips modifications/deletes of existing parent paths. Useful for refill
    /// snapshots that redo already-landed densify plus a few new stems.
    #[serde(default)]
    #[schemars(
        description = "When true, land only paths missing on the parent tree (new files). Existing parent paths are skipped rather than merged/overwritten."
    )]
    pub only_missing: Option<bool>,

    /// Union-merge JSON arrays of objects by this key (default when set: `name`).
    /// `assets/manifest/*.json` always merges by `name`. Other `.json` files
    /// merge only when this is set. Fail closed if the file is not mergeable.
    #[serde(default)]
    #[schemars(
        description = "JSON union-merge key (e.g. `name`). Kit manifests under assets/manifest/ always merge by name. When set, every other landed `.json` file is union-merged by this key; fail closed if the shape is not an array of objects or an object map."
    )]
    pub json_union_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LandSubagentOutput {
    pub subagent_id: String,
    pub success: bool,
    pub mode: String,
    /// Artifact used: `live_worktree`, `snapshot_ref`, or `patch`.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_path: Option<String>,
    /// Files successfully applied into the parent.
    pub files_landed: Vec<String>,
    /// Conflicting paths when merge fails closed (empty on success).
    pub conflicts: Vec<String>,
    pub message: String,
}

impl xai_tool_runtime::ToolOutput for LandSubagentOutput {
    fn model_output(&self) -> Vec<xai_tool_runtime::ContentBlock> {
        let mut text = String::new();
        text.push_str(&self.message);
        text.push('\n');
        if !self.files_landed.is_empty() {
            text.push_str("\n## Files landed\n");
            for f in &self.files_landed {
                text.push_str("- ");
                text.push_str(f);
                text.push('\n');
            }
        }
        if !self.conflicts.is_empty() {
            text.push_str("\n## Conflicts (nothing applied — fail closed)\n");
            for f in &self.conflicts {
                text.push_str("- ");
                text.push_str(f);
                text.push('\n');
            }
            text.push_str(
                "\nResolve conflicts manually or re-run with mode=`overwrite` only if you \
                 intend to discard parent-side edits on those paths.\n",
            );
        }
        vec![xai_tool_runtime::ContentBlock::Text { text }]
    }
}

#[derive(Debug, Default)]
pub struct LandSubagentTool;

impl crate::types::tool_metadata::ToolMetadata for LandSubagentTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        r#"Apply a subagent's isolated work into the parent repository (explicit land — isolation is preserved until this call).

Resolves `subagents/<subagent_id>/meta.json` and lands from (priority order):
1. **live worktree** — `workspace.apply_worktree` when the host injects it; otherwise in-process apply with the same semantics
2. **snapshot_ref** — per-file content from `refs/grok/subagents/<id>` (or whatever ref was snapshotted)
3. **patch_path** / `changes.patch` — `git apply`

**mode** (default `merge`):
- `merge` — fail closed on conflict: if the parent file diverged from the spawn base while the child also changed it, land aborts with a clear conflict list and does **not** silently overwrite
- `overwrite` — replace parent files with child content for all changed paths

Use `diff_subagent` first to review. Do not land untrusted or unreviewed work.

When the subagent was spawned with `allowed_paths`, land refuses (error) if any changed path falls outside those relative prefixes — fail closed like merge conflicts. Harness markers (`.grok-subagent-live`, `.grok/`) are ignored by the allowlist and are never copied into the parent.

`assets/manifest/*.json` is union-merged by `name` so parallel densify children do not wipe sibling rows. Pass `json_union_by` to apply the same merge to other landed `.json` files (fail closed if the shape is not an array of objects or an object map).

**only_missing** (optional): when true, land only paths that do not already exist on the parent (new files). Skips updates to already-present parent paths — useful for stale refill snapshots."#
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        use crate::types::tool_metadata::ToolMetadata as TM;
        Expr::Value(ToolRequirement::Tool {
            namespace: TM::tool_namespace(&TaskTool).to_string(),
            id: xai_tool_runtime::Tool::id(&TaskTool).to_string(),
            if_params: None,
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }
}

impl xai_tool_runtime::Tool for LandSubagentTool {
    type Args = LandSubagentInput;
    type Output = LandSubagentOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(LAND_SUBAGENT_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            LAND_SUBAGENT_TOOL_NAME,
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: false,
            tool_scope: Some(xai_tool_protocol::ToolScope::Write),
            ..Default::default()
        }
    }

    #[tracing::instrument(
        name = "tool.land_subagent",
        skip_all,
        fields(subagent_id = %input.subagent_id, mode = ?input.mode)
    )]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: LandSubagentInput,
    ) -> Result<LandSubagentOutput, xai_tool_runtime::ToolError> {
        let mode = LandMode::from_input(input.mode.as_deref())
            .map_err(|e| xai_tool_runtime::ToolError::custom("invalid_mode", e))?;

        let work = resolve_subagent_work(&ctx, &input.subagent_id).await?;
        let force = input.force.unwrap_or(false);
        let only_missing = input.only_missing.unwrap_or(false);
        let json_union_by = input
            .json_union_by
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let files_hint = work
            .meta
            .diffstat
            .as_deref()
            .and_then(super::parse_files_changed_from_diffstat)
            .or_else(|| {
                work.meta
                    .changed_paths
                    .as_ref()
                    .map(|p| super::filter_harness_land_paths(p).len() as u32)
            });
        super::land_size_guard(files_hint, force, super::DEFAULT_LAND_MAX_FILES)?;
        let resources = shared_resources(&ctx)?;

        // Prefer agent-only snapshot (baseline..snap) over live dirty tree when
        // a spawn baseline exists — soft-preserve leaves live trees full of
        // parent dirt that would inflate land (harness QA P1).
        let prefer_snapshot = work
            .meta
            .baseline_ref
            .as_ref()
            .is_some_and(|b| !b.is_empty())
            && work.snapshot_ref.is_some();
        if prefer_snapshot {
            if let Some(ref snap) = work.snapshot_ref {
                return land_snapshot_ref(&work, snap, mode, only_missing, json_union_by).await;
            }
        }

        // Fail closed when this was an isolation worktree run but baseline is
        // missing — live-tree land would reintroduce dirty-parent inflation.
        // Operators may pass force=true to override (escape hatch).
        let was_worktree = work.live_worktree.is_some()
            || work
                .meta
                .worktree_path
                .as_ref()
                .is_some_and(|p| !p.is_empty())
            || work.meta.worktree_state.as_deref() == Some("preserved")
            || work.meta.worktree_state.as_deref() == Some("cleaned");
        let baseline_missing = work
            .meta
            .baseline_ref
            .as_ref()
            .map(|b| b.is_empty())
            .unwrap_or(true);
        if was_worktree && baseline_missing && !force {
            return Err(xai_tool_runtime::ToolError::custom(
                "land_baseline_missing",
                "land refused: isolation worktree has no baseline_ref (agent-only delta \
                 unavailable). Re-run the child with a successful baseline capture, or pass \
                 force=true to land the live/dirty tree (unsafe — may include parent dirt).",
            ));
        }

        // 1) Live worktree (no baseline, or snapshot missing)
        if let Some(ref wt) = work.live_worktree {
            // Pre-check allowlist against worktree change set before any apply.
            // Fail closed when an allowlist is set and path collection fails.
            match collect_live_worktree_paths(wt, &work.parent_git_root).await {
                Ok(paths) => refuse_land_outside_allowlist(&work.meta, &paths)?,
                Err(e) if effective_allowed_paths(&work.meta).is_some() => {
                    return Err(xai_tool_runtime::ToolError::custom(
                        "land_allowlist_check_failed",
                        format!(
                            "land refused: could not enumerate worktree changes for \
                             allowed_paths check: {e}"
                        ),
                    ));
                }
                Err(_) => {}
            }
            // Host apply_worktree does not support only_missing filtering — use
            // in-process path when that flag is set so we can skip existing files.
            if !only_missing {
                let wt_str = wt.to_string_lossy().into_owned();
                let backend = {
                    let res = resources.lock().await;
                    res.get::<LiveWorktreeApplyBackend>().map(|b| b.0.clone())
                };
                if let Some(apply_fn) = backend {
                    let apply_mode = to_apply_mode(mode);
                    match apply_fn(wt_str.clone(), apply_mode).await {
                        Ok(resp) => {
                            return map_apply_response(
                                &work,
                                mode,
                                "live_worktree",
                                Some(wt.display().to_string()),
                                resp,
                            )
                            .await;
                        }
                        Err(e) => {
                            return Err(xai_tool_runtime::ToolError::custom(
                                "apply_worktree_failed",
                                format!("workspace.apply_worktree failed for {wt_str}: {e}"),
                            ));
                        }
                    }
                }
            }
            // In-process mirror of apply_worktree Merge/Overwrite.
            return land_live_worktree_inprocess(&work, wt, mode, only_missing, json_union_by)
                .await;
        }

        // 2) Snapshot ref
        if let Some(ref snap) = work.snapshot_ref {
            return land_snapshot_ref(&work, snap, mode, only_missing, json_union_by).await;
        }

        // 3) Patch
        if let Some(ref patch) = work.patch_path {
            return land_patch(&work, patch, mode).await;
        }

        Err(xai_tool_runtime::ToolError::custom(
            "no_subagent_work",
            "no live worktree, snapshot_ref, or patch available to land",
        ))
    }
}

/// Collect relative paths changed in a live worktree vs parent HEAD (+ untracked).
async fn collect_live_worktree_paths(
    wt: &std::path::Path,
    parent: &std::path::Path,
) -> Result<Vec<String>, String> {
    let parent_head = git_capture(parent, &["rev-parse", "HEAD"]).await?;
    let parent_head = parent_head.trim().to_owned();
    let name_status = git_capture(wt, &["diff", "--name-status", &parent_head]).await?;
    let mut paths = parse_name_status(&name_status);
    if let Ok(untracked) = git_capture(wt, &["ls-files", "--others", "--exclude-standard"]).await {
        for line in untracked.lines() {
            let p = line.trim();
            if !p.is_empty() {
                paths.push(p.to_owned());
            }
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

async fn map_apply_response(
    work: &super::ResolvedSubagentWork,
    mode: LandMode,
    source: &str,
    worktree_path: Option<String>,
    resp: xai_grok_workspace_types::rpc::worktree::ApplyWorktreeResponse,
) -> Result<LandSubagentOutput, xai_tool_runtime::ToolError> {
    use xai_grok_workspace_types::rpc::worktree::ApplyWorktreeResponse;
    match resp {
        ApplyWorktreeResponse::Success { files, .. } => {
            let files_landed: Vec<String> = files.into_iter().map(|f| f.path).collect();
            update_meta_land_status(&work.meta_path, "landed").await;
            Ok(LandSubagentOutput {
                subagent_id: work.subagent_id.clone(),
                success: true,
                mode: mode.as_str().into(),
                source: source.into(),
                snapshot_ref: work.snapshot_ref.clone(),
                worktree_path,
                patch_path: work.patch_path.as_ref().map(|p| p.display().to_string()),
                message: format!(
                    "Landed subagent `{}` via {source} (mode={}) — {} file(s).",
                    work.subagent_id,
                    mode.as_str(),
                    files_landed.len()
                ),
                files_landed,
                conflicts: vec![],
            })
        }
        ApplyWorktreeResponse::Conflicts { files, conflicts } => {
            // Fail closed: merge apply_worktree plans first and applies only
            // when conflict-free (`files` should be empty). Keep partial note
            // only if a legacy host still returns already-written paths.
            let conflict_paths: Vec<String> = conflicts.into_iter().map(|c| c.path).collect();
            let partial: Vec<String> = files.into_iter().map(|f| f.path).collect();
            update_meta_land_status(&work.meta_path, "conflict").await;
            let mut message = format!(
                "Land of subagent `{}` via {source} hit {} conflict(s) (mode={}). \
                 Fail closed: nothing applied; resolve conflicts or re-run with mode=`overwrite`.",
                work.subagent_id,
                conflict_paths.len(),
                mode.as_str()
            );
            if !partial.is_empty() {
                message.push_str(&format!(
                    " Warning: host reported {} already-applied path(s) — unexpected after \
                     plan-first merge; verify parent tree.",
                    partial.len()
                ));
            }
            Ok(LandSubagentOutput {
                subagent_id: work.subagent_id.clone(),
                success: false,
                mode: mode.as_str().into(),
                source: source.into(),
                snapshot_ref: work.snapshot_ref.clone(),
                worktree_path,
                patch_path: work.patch_path.as_ref().map(|p| p.display().to_string()),
                files_landed: partial,
                conflicts: conflict_paths,
                message,
            })
        }
    }
}

/// In-process land for a live worktree — mirrors workspace `apply_worktree`
/// but **fail-closed**: detects all conflicts first, then applies only if
/// merge is clean (or mode is overwrite).
async fn land_live_worktree_inprocess(
    work: &super::ResolvedSubagentWork,
    wt: &std::path::Path,
    mode: LandMode,
    only_missing: bool,
    json_union_by: Option<&str>,
) -> Result<LandSubagentOutput, xai_tool_runtime::ToolError> {
    let parent = &work.parent_git_root;
    let parent_head = git_capture(parent, &["rev-parse", "HEAD"])
        .await
        .map_err(|e| {
            xai_tool_runtime::ToolError::custom("git_error", format!("parent HEAD: {e}"))
        })?;
    let parent_head = parent_head.trim().to_owned();

    let name_status = git_capture(wt, &["diff", "--name-status", &parent_head])
        .await
        .map_err(|e| {
            xai_tool_runtime::ToolError::custom("git_error", format!("worktree diff: {e}"))
        })?;
    let mut paths = parse_name_status(&name_status);
    if let Ok(untracked) = git_capture(wt, &["ls-files", "--others", "--exclude-standard"]).await {
        for line in untracked.lines() {
            let p = line.trim();
            if !p.is_empty() {
                paths.push(p.to_owned());
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths = filter_harness_land_paths(&paths);

    if only_missing {
        paths.retain(|p| !parent.join(p).exists());
    }

    if paths.is_empty() {
        update_meta_land_status(&work.meta_path, "landed").await;
        return Ok(LandSubagentOutput {
            subagent_id: work.subagent_id.clone(),
            success: true,
            mode: mode.as_str().into(),
            source: "live_worktree".into(),
            snapshot_ref: work.snapshot_ref.clone(),
            worktree_path: Some(wt.display().to_string()),
            patch_path: work.patch_path.as_ref().map(|p| p.display().to_string()),
            files_landed: vec![],
            conflicts: vec![],
            message: format!(
                "Subagent `{}` live worktree has no {} vs parent HEAD — nothing to land.",
                work.subagent_id,
                if only_missing {
                    "missing (new) paths"
                } else {
                    "changes"
                }
            ),
        });
    }

    refuse_land_outside_allowlist(&work.meta, &paths)?;

    let mut plan: Vec<(String, Option<Vec<u8>>)> = Vec::new(); // path, theirs
    let mut conflicts = Vec::new();

    for path in &paths {
        let wt_file = wt.join(path);
        let theirs = if wt_file.is_file() {
            Some(tokio::fs::read(&wt_file).await.map_err(|e| {
                xai_tool_runtime::ToolError::custom(
                    "read_failed",
                    format!("read {}: {e}", wt_file.display()),
                )
            })?)
        } else {
            None // deleted in child
        };

        if only_missing {
            // Parent path does not exist (filtered above) — always take theirs.
            plan.push((path.clone(), theirs));
            continue;
        }

        if mode == LandMode::Overwrite {
            plan.push((path.clone(), theirs));
            continue;
        }

        // Merge: base = parent HEAD blob; ours = current parent file (bytes)
        let base = git_show_blob(parent, &parent_head, path).await;
        let main_file = parent.join(path);
        let ours = if main_file.is_file() {
            tokio::fs::read(&main_file).await.ok()
        } else {
            None
        };

        if base == ours {
            // Parent unchanged since base → safe to take theirs
            plan.push((path.clone(), theirs));
        } else if base != theirs {
            // Both sides changed → conflict
            conflicts.push(path.clone());
        }
        // else: child matches base, parent diverged — skip (parent wins)
    }

    if !conflicts.is_empty() {
        update_meta_land_status(&work.meta_path, "conflict").await;
        return Ok(LandSubagentOutput {
            subagent_id: work.subagent_id.clone(),
            success: false,
            mode: mode.as_str().into(),
            source: "live_worktree".into(),
            snapshot_ref: work.snapshot_ref.clone(),
            worktree_path: Some(wt.display().to_string()),
            patch_path: work.patch_path.as_ref().map(|p| p.display().to_string()),
            files_landed: vec![],
            conflicts,
            message: format!(
                "Land of subagent `{}` aborted (mode=merge, fail closed): parent files diverged \
                 from the worktree base on conflict paths. Re-run with mode=`overwrite` only if \
                 you intend to discard those parent edits.",
                work.subagent_id
            ),
        });
    }

    let mut landed = Vec::new();
    for (path, theirs) in plan {
        apply_file_content_with_union(parent, &path, theirs.as_deref(), json_union_by)
            .await
            .map_err(|e| xai_tool_runtime::ToolError::custom("land_write_failed", e))?;
        landed.push(path);
    }

    update_meta_land_status(&work.meta_path, "landed").await;
    Ok(LandSubagentOutput {
        subagent_id: work.subagent_id.clone(),
        success: true,
        mode: mode.as_str().into(),
        source: "live_worktree".into(),
        snapshot_ref: work.snapshot_ref.clone(),
        worktree_path: Some(wt.display().to_string()),
        patch_path: work.patch_path.as_ref().map(|p| p.display().to_string()),
        message: format!(
            "Landed subagent `{}` from live worktree (mode={}) — {} file(s).",
            work.subagent_id,
            mode.as_str(),
            landed.len()
        ),
        files_landed: landed,
        conflicts: vec![],
    })
}

async fn land_snapshot_ref(
    work: &super::ResolvedSubagentWork,
    snap: &str,
    mode: LandMode,
    only_missing: bool,
    json_union_by: Option<&str>,
) -> Result<LandSubagentOutput, xai_tool_runtime::ToolError> {
    let parent = &work.parent_git_root;
    git_capture(parent, &["rev-parse", "--verify", snap])
        .await
        .map_err(|e| {
            xai_tool_runtime::ToolError::custom(
                "snapshot_missing",
                format!("snapshot_ref `{snap}` does not resolve: {e}"),
            )
        })?;

    let parent_head = git_capture(parent, &["rev-parse", "HEAD"])
        .await
        .map_err(|e| {
            xai_tool_runtime::ToolError::custom("git_error", format!("parent HEAD: {e}"))
        })?;
    let parent_head = parent_head.trim().to_owned();

    // Agent-only base: spawn baseline when present (excludes dirty-parent bulk
    // copied into the sandbox). Fall back to merge-base(HEAD, snap) / HEAD.
    let baseline = work.meta.baseline_ref.as_deref().filter(|b| !b.is_empty());
    let base_rev = if let Some(base) = baseline {
        if git_capture(parent, &["rev-parse", "--verify", base])
            .await
            .is_ok()
        {
            base.to_owned()
        } else {
            // Declared baseline must resolve — do not silently fall back to
            // merge-base/HEAD (would reintroduce dirty-parent inflation).
            return Err(xai_tool_runtime::ToolError::custom(
                "land_baseline_missing",
                format!(
                    "land refused: baseline_ref `{base}` does not resolve. \
                     Re-spawn the child for a fresh baseline, or pass force=true \
                     only if you accept non-agent-only land."
                ),
            ));
        }
    } else {
        git_capture(parent, &["merge-base", "HEAD", snap])
            .await
            .map(|s| s.trim().to_owned())
            .unwrap_or_else(|_| parent_head.clone())
    };

    // Agent-only path list: baseline..snapshot (not HEAD..snapshot).
    let name_status = git_capture(parent, &["diff", "--name-status", &base_rev, snap])
        .await
        .map_err(|e| {
            xai_tool_runtime::ToolError::custom("git_error", format!("diff name-status: {e}"))
        })?;
    let mut paths = filter_harness_land_paths(&parse_name_status(&name_status));
    if only_missing {
        paths.retain(|p| !parent.join(p).exists());
    }

    if paths.is_empty() {
        update_meta_land_status(&work.meta_path, "landed").await;
        return Ok(LandSubagentOutput {
            subagent_id: work.subagent_id.clone(),
            success: true,
            mode: mode.as_str().into(),
            source: "snapshot_ref".into(),
            snapshot_ref: Some(snap.to_owned()),
            worktree_path: work.meta.worktree_path.clone(),
            patch_path: work.patch_path.as_ref().map(|p| p.display().to_string()),
            files_landed: vec![],
            conflicts: vec![],
            message: format!(
                "Subagent `{}` snapshot `{snap}` has no {} vs `{base_rev}` — nothing to land.",
                work.subagent_id,
                if only_missing {
                    "missing (new) paths"
                } else {
                    "agent-only diff"
                }
            ),
        });
    }

    refuse_land_outside_allowlist(&work.meta, &paths)?;

    let mut plan: Vec<(String, Option<Vec<u8>>)> = Vec::new();
    let mut conflicts = Vec::new();

    for path in &paths {
        let theirs = git_show_blob(parent, snap, path).await;
        if only_missing || mode == LandMode::Overwrite {
            plan.push((path.clone(), theirs));
            continue;
        }
        let base = git_show_blob(parent, &base_rev, path).await;
        let main_file = parent.join(path);
        let ours = if main_file.is_file() {
            tokio::fs::read(&main_file).await.ok()
        } else {
            None
        };
        if base == ours {
            plan.push((path.clone(), theirs));
        } else if base != theirs {
            conflicts.push(path.clone());
        }
    }

    if !conflicts.is_empty() {
        update_meta_land_status(&work.meta_path, "conflict").await;
        return Ok(LandSubagentOutput {
            subagent_id: work.subagent_id.clone(),
            success: false,
            mode: mode.as_str().into(),
            source: "snapshot_ref".into(),
            snapshot_ref: Some(snap.to_owned()),
            worktree_path: work.meta.worktree_path.clone(),
            patch_path: work.patch_path.as_ref().map(|p| p.display().to_string()),
            files_landed: vec![],
            conflicts,
            message: format!(
                "Land of subagent `{}` from snapshot `{snap}` aborted (mode=merge, fail closed). \
                 Parent diverged on conflict paths. Use mode=`overwrite` only if intentional.",
                work.subagent_id
            ),
        });
    }

    let mut landed = Vec::new();
    for (path, theirs) in plan {
        apply_file_content_with_union(parent, &path, theirs.as_deref(), json_union_by)
            .await
            .map_err(|e| xai_tool_runtime::ToolError::custom("land_write_failed", e))?;
        landed.push(path);
    }

    update_meta_land_status(&work.meta_path, "landed").await;
    Ok(LandSubagentOutput {
        subagent_id: work.subagent_id.clone(),
        success: true,
        mode: mode.as_str().into(),
        source: "snapshot_ref".into(),
        snapshot_ref: Some(snap.to_owned()),
        worktree_path: work.meta.worktree_path.clone(),
        patch_path: work.patch_path.as_ref().map(|p| p.display().to_string()),
        message: format!(
            "Landed subagent `{}` from snapshot_ref `{snap}` (mode={}) — {} file(s).",
            work.subagent_id,
            mode.as_str(),
            landed.len()
        ),
        files_landed: landed,
        conflicts: vec![],
    })
}

/// Fail closed on absolute / drive / `..` paths in an exported patch (C3).
fn refuse_patch_paths_in_repo(paths: &[String]) -> Result<(), xai_tool_runtime::ToolError> {
    let mut bad = Vec::new();
    for p in paths {
        let s = p.trim().replace('\\', "/");
        if s.is_empty() {
            continue;
        }
        if s.starts_with('/')
            || s.starts_with("//")
            || (s.len() >= 2 && s.as_bytes()[1] == b':')
            || s.split('/').any(|part| part == "..")
            || super::normalize_allowlist_path(p).is_none()
        {
            bad.push(p.clone());
        }
    }
    if bad.is_empty() {
        return Ok(());
    }
    let preview: Vec<&str> = bad.iter().take(6).map(String::as_str).collect();
    Err(xai_tool_runtime::ToolError::custom(
        "land_patch_path_escape",
        format!(
            "land refused: patch contains {} path(s) outside the repository \
             (absolute or `..`): {}. Export a repo-relative patch only.",
            bad.len(),
            preview.join(", "),
        ),
    ))
}

async fn land_patch(
    work: &super::ResolvedSubagentWork,
    patch: &std::path::Path,
    mode: LandMode,
) -> Result<LandSubagentOutput, xai_tool_runtime::ToolError> {
    let parent = &work.parent_git_root;
    let patch_str = patch.to_string_lossy().into_owned();

    // Refuse before apply when patch touches paths outside allowlist, or any
    // absolute / traversal path (host escape via crafted patch).
    let patch_body = tokio::fs::read_to_string(patch).await.unwrap_or_default();
    let patch_files = super::diff::files_from_patch(&patch_body);
    refuse_patch_paths_in_repo(&patch_files)?;
    refuse_land_outside_allowlist(&work.meta, &patch_files)?;

    // Always check first so we can fail closed without partial apply.
    // Do NOT pass --unsafe-paths: git must refuse absolute / outside-repo paths.
    let check_args: Vec<&str> = if mode == LandMode::Merge {
        vec!["apply", "--check", "--3way", &patch_str]
    } else {
        vec!["apply", "--check", &patch_str]
    };
    let (ok, _stdout, stderr) = git_capture_status(parent, &check_args)
        .await
        .map_err(|e| xai_tool_runtime::ToolError::custom("git_error", e))?;

    if !ok {
        update_meta_land_status(&work.meta_path, "conflict").await;
        return Ok(LandSubagentOutput {
            subagent_id: work.subagent_id.clone(),
            success: false,
            mode: mode.as_str().into(),
            source: "patch".into(),
            snapshot_ref: work.snapshot_ref.clone(),
            worktree_path: work.meta.worktree_path.clone(),
            patch_path: Some(patch.display().to_string()),
            files_landed: vec![],
            conflicts: extract_conflict_hints(&stderr),
            message: format!(
                "Land of subagent `{}` from patch failed (mode={}, fail closed). \
                 git apply --check: {}",
                work.subagent_id,
                mode.as_str(),
                stderr.trim()
            ),
        });
    }

    let apply_args: Vec<&str> = if mode == LandMode::Merge {
        vec!["apply", "--3way", &patch_str]
    } else {
        vec!["apply", &patch_str]
    };
    let (ok, _stdout, stderr) = git_capture_status(parent, &apply_args)
        .await
        .map_err(|e| xai_tool_runtime::ToolError::custom("git_error", e))?;

    if !ok {
        update_meta_land_status(&work.meta_path, "conflict").await;
        return Ok(LandSubagentOutput {
            subagent_id: work.subagent_id.clone(),
            success: false,
            mode: mode.as_str().into(),
            source: "patch".into(),
            snapshot_ref: work.snapshot_ref.clone(),
            worktree_path: work.meta.worktree_path.clone(),
            patch_path: Some(patch.display().to_string()),
            files_landed: vec![],
            conflicts: extract_conflict_hints(&stderr),
            message: format!(
                "Land of subagent `{}` from patch failed during apply (mode={}): {}",
                work.subagent_id,
                mode.as_str(),
                stderr.trim()
            ),
        });
    }

    let files = filter_harness_land_paths(&patch_files);
    scrub_harness_from_parent(parent, &patch_files).await;

    update_meta_land_status(&work.meta_path, "landed").await;
    Ok(LandSubagentOutput {
        subagent_id: work.subagent_id.clone(),
        success: true,
        mode: mode.as_str().into(),
        source: "patch".into(),
        snapshot_ref: work.snapshot_ref.clone(),
        worktree_path: work.meta.worktree_path.clone(),
        patch_path: Some(patch.display().to_string()),
        message: format!(
            "Landed subagent `{}` from patch {} (mode={}) — {} file(s).",
            work.subagent_id,
            patch.display(),
            mode.as_str(),
            files.len()
        ),
        files_landed: files,
        conflicts: vec![],
    })
}

async fn scrub_harness_from_parent(parent: &std::path::Path, paths: &[String]) {
    for p in paths {
        if !super::is_harness_land_path(p) {
            continue;
        }
        let dest = parent.join(p);
        if dest.is_file() {
            let _ = tokio::fs::remove_file(&dest).await;
        }
    }
}

fn extract_conflict_hints(stderr: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in stderr.lines() {
        let line = line.trim();
        // Common git apply messages: "error: patch failed: path:N" / "U path"
        if let Some(rest) = line.strip_prefix("error: patch failed: ") {
            if let Some(path) = rest.split(':').next() {
                out.push(path.to_owned());
            }
        }
        if let Some(path) = line.strip_prefix("U ") {
            out.push(path.to_owned());
        }
    }
    out.sort();
    out.dedup();
    out
}
