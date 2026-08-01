//! `diff_subagent` — show a subagent's work vs parent HEAD.
//!
//! Prefers (in order): live worktree, snapshot_ref, then changes.patch.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    git_capture, git_capture_status, parse_name_status, resolve_subagent_work, truncate_diff_text,
};
use crate::implementations::grok_build::task::TaskTool;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

pub const DIFF_SUBAGENT_TOOL_NAME: &str = "diff_subagent";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DiffSubagentInput {
    #[schemars(
        description = "Required. Subagent id returned by `task` / completion (`subagent_id`). Resolves session subagents/<id>/meta.json for snapshot_ref, worktree_path, and patch_path."
    )]
    pub subagent_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DiffSubagentOutput {
    pub subagent_id: String,
    /// Which artifact was used: `live_worktree`, `snapshot_ref`, or `patch`.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_path: Option<String>,
    /// Relative paths changed vs parent HEAD (or listed in the patch).
    pub files: Vec<String>,
    /// Unified diff text (may be truncated).
    pub diff: String,
    pub truncated: bool,
    /// Human summary for the model.
    pub message: String,
}

impl xai_tool_runtime::ToolOutput for DiffSubagentOutput {
    fn model_output(&self) -> Vec<xai_tool_runtime::ContentBlock> {
        let mut text = String::new();
        text.push_str(&self.message);
        text.push('\n');
        if !self.files.is_empty() {
            text.push_str("\n## Files\n");
            for f in &self.files {
                text.push_str("- ");
                text.push_str(f);
                text.push('\n');
            }
        }
        if !self.diff.trim().is_empty() {
            text.push_str("\n## Diff\n```diff\n");
            text.push_str(&self.diff);
            if !self.diff.ends_with('\n') {
                text.push('\n');
            }
            text.push_str("```\n");
        }
        vec![xai_tool_runtime::ContentBlock::Text { text }]
    }
}

#[derive(Debug, Default)]
pub struct DiffSubagentTool;

impl crate::types::tool_metadata::ToolMetadata for DiffSubagentTool {
    fn kind(&self) -> ToolKind {
        // Same family as other subagent management tools; not used in by_kind
        // templates (id is explicit). ToolKind::Other keeps capability maps stable.
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        r#"Show the diff of a completed (or still-live) subagent's work against the parent repository HEAD.

Resolves `subagents/<subagent_id>/meta.json` and diffs in priority order:
1. **live worktree** (`worktree_path` still on disk) — committed + dirty changes vs parent HEAD
2. **snapshot_ref** (e.g. `refs/grok/subagents/<id>`) — `git diff HEAD <snapshot_ref>`
3. **patch_path** / `changes.patch` — returns the exported unified patch

Returns unified diff text plus a file list. Use this before `land_subagent` to review multi-agent work without git archaeology. Read-only — does not modify the parent tree."#
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        // Available whenever the parent can spawn subagents.
        use crate::types::tool_metadata::ToolMetadata as TM;
        Expr::Value(ToolRequirement::Tool {
            namespace: TM::tool_namespace(&TaskTool).to_string(),
            id: xai_tool_runtime::Tool::id(&TaskTool).to_string(),
            if_params: None,
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

impl xai_tool_runtime::Tool for DiffSubagentTool {
    type Args = DiffSubagentInput;
    type Output = DiffSubagentOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(DIFF_SUBAGENT_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            DIFF_SUBAGENT_TOOL_NAME,
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: true,
            tool_scope: Some(xai_tool_protocol::ToolScope::Read),
            ..Default::default()
        }
    }

    #[tracing::instrument(
        name = "tool.diff_subagent",
        skip_all,
        fields(subagent_id = %input.subagent_id)
    )]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: DiffSubagentInput,
    ) -> Result<DiffSubagentOutput, xai_tool_runtime::ToolError> {
        let work = resolve_subagent_work(&ctx, &input.subagent_id).await?;
        let parent = &work.parent_git_root;

        // 1) Live worktree
        if let Some(ref wt) = work.live_worktree {
            let parent_head = git_capture(parent, &["rev-parse", "HEAD"])
                .await
                .map_err(|e| {
                    xai_tool_runtime::ToolError::custom("git_error", format!("parent HEAD: {e}"))
                })?;
            let parent_head = parent_head.trim().to_owned();

            let name_status = git_capture(
                wt,
                &["diff", "--name-status", &parent_head],
            )
            .await
            .unwrap_or_default();
            let mut files = parse_name_status(&name_status);

            // Untracked files in the worktree (not in name-status against parent_head)
            if let Ok(untracked) =
                git_capture(wt, &["ls-files", "--others", "--exclude-standard"]).await
            {
                for line in untracked.lines() {
                    let p = line.trim();
                    if !p.is_empty() && !files.iter().any(|f| f == p) {
                        files.push(p.to_owned());
                    }
                }
            }
            files.sort();
            files.dedup();

            let mut diff = git_capture(wt, &["diff", &parent_head])
                .await
                .unwrap_or_default();
            // Append untracked as /dev/null diffs when possible
            if let Ok(untracked) =
                git_capture(wt, &["ls-files", "--others", "--exclude-standard"]).await
            {
                for line in untracked.lines() {
                    let p = line.trim();
                    if p.is_empty() {
                        continue;
                    }
                    let path = wt.join(p);
                    if let Ok(body) = tokio::fs::read_to_string(&path).await {
                        use std::fmt::Write;
                        let _ = write!(
                            &mut diff,
                            "diff --git a/{p} b/{p}\n\
                             new file mode 100644\n\
                             --- /dev/null\n\
                             +++ b/{p}\n"
                        );
                        for l in body.lines() {
                            let _ = writeln!(&mut diff, "+{l}");
                        }
                        if !body.ends_with('\n') && !body.is_empty() {
                            diff.push('\n');
                        }
                    }
                }
            }

            let (diff, truncated) = truncate_diff_text(&diff);
            let message = format!(
                "Diff for subagent `{}` from live worktree {} ({} file(s) vs parent HEAD).",
                work.subagent_id,
                wt.display(),
                files.len()
            );
            return Ok(DiffSubagentOutput {
                subagent_id: work.subagent_id,
                source: "live_worktree".into(),
                snapshot_ref: work.snapshot_ref,
                worktree_path: Some(wt.display().to_string()),
                patch_path: work
                    .patch_path
                    .as_ref()
                    .map(|p| p.display().to_string()),
                files,
                diff,
                truncated,
                message,
            });
        }

        // 2) Snapshot ref
        if let Some(ref snap) = work.snapshot_ref {
            // Ensure ref resolves in parent repo
            git_capture(parent, &["rev-parse", "--verify", snap])
                .await
                .map_err(|e| {
                    xai_tool_runtime::ToolError::custom(
                        "snapshot_missing",
                        format!(
                            "snapshot_ref `{snap}` does not resolve in {}: {e}",
                            parent.display()
                        ),
                    )
                })?;

            let name_status = git_capture(
                parent,
                &["diff", "--name-status", "HEAD", snap],
            )
            .await
            .map_err(|e| {
                xai_tool_runtime::ToolError::custom("git_error", format!("diff name-status: {e}"))
            })?;
            let files = parse_name_status(&name_status);
            let raw_diff = git_capture(parent, &["diff", "HEAD", snap])
                .await
                .map_err(|e| {
                    xai_tool_runtime::ToolError::custom("git_error", format!("diff: {e}"))
                })?;
            let (diff, truncated) = truncate_diff_text(&raw_diff);
            let message = format!(
                "Diff for subagent `{}` from snapshot_ref `{snap}` ({} file(s) vs parent HEAD).",
                work.subagent_id,
                files.len()
            );
            return Ok(DiffSubagentOutput {
                subagent_id: work.subagent_id,
                source: "snapshot_ref".into(),
                snapshot_ref: Some(snap.clone()),
                worktree_path: work.meta.worktree_path.clone(),
                patch_path: work
                    .patch_path
                    .as_ref()
                    .map(|p| p.display().to_string()),
                files,
                diff,
                truncated,
                message,
            });
        }

        // 3) Patch file
        if let Some(ref patch) = work.patch_path {
            let raw = tokio::fs::read_to_string(patch).await.map_err(|e| {
                xai_tool_runtime::ToolError::custom(
                    "patch_read_failed",
                    format!("failed to read {}: {e}", patch.display()),
                )
            })?;
            let files = files_from_patch(&raw);
            // Optional: check whether patch still applies cleanly (informational)
            let (_ok, _stdout, check_err) = git_capture_status(
                parent,
                &[
                    "apply",
                    "--check",
                    "--unsafe-paths",
                    &patch.to_string_lossy(),
                ],
            )
            .await
            .unwrap_or((false, String::new(), String::new()));
            let (diff, truncated) = truncate_diff_text(&raw);
            let mut message = format!(
                "Diff for subagent `{}` from patch {} ({} file(s)).",
                work.subagent_id,
                patch.display(),
                files.len()
            );
            if !check_err.trim().is_empty() {
                message.push_str(&format!(
                    "\nNote: `git apply --check` reports: {}",
                    check_err.trim()
                ));
            }
            return Ok(DiffSubagentOutput {
                subagent_id: work.subagent_id,
                source: "patch".into(),
                snapshot_ref: None,
                worktree_path: work.meta.worktree_path.clone(),
                patch_path: Some(patch.display().to_string()),
                files,
                diff,
                truncated,
                message,
            });
        }

        Err(xai_tool_runtime::ToolError::custom(
            "no_subagent_work",
            "no live worktree, snapshot_ref, or patch available",
        ))
    }
}

pub(crate) fn files_from_patch(patch: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in patch.lines() {
        // diff --git a/foo b/foo
        if let Some(rest) = line.strip_prefix("diff --git ") {
            // split into a/... b/...
            if let Some(b_idx) = rest.find(" b/") {
                let b = &rest[b_idx + 3..];
                if !b.is_empty() {
                    files.push(b.to_owned());
                    continue;
                }
            }
        }
        if let Some(p) = line.strip_prefix("+++ b/") {
            let p = p.trim();
            if p != "/dev/null" && !p.is_empty() {
                files.push(p.to_owned());
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_patch_paths() {
        let patch = "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -1 +1 @@
-old
+new
diff --git a/src/b.rs b/src/b.rs
new file mode 100644
--- /dev/null
+++ b/src/b.rs
@@ -0,0 +1 @@
+hi
";
        let files = files_from_patch(patch);
        assert_eq!(files, vec!["src/a.rs".to_owned(), "src/b.rs".to_owned()]);
    }
}
