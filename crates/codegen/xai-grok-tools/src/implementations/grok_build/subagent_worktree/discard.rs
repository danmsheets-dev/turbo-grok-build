//! `discard_subagent` — drop a live subagent worktree; keep snapshot ref by default.
//!
//! RC13 Wave A: discard always leaves meta in a **terminal** disposition:
//! `land_status=discarded`, `worktree_state=cleaned`, `status` never left as
//! `running`, and `snapshot_dropped` only when the ref was actually deleted.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{resolve_subagent_work, update_meta_discarded};
use crate::implementations::grok_build::task::TaskTool;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

pub const DISCARD_SUBAGENT_TOOL_NAME: &str = "discard_subagent";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DiscardSubagentInput {
    #[schemars(
        description = "Required. Subagent id from `task` completion. Loads subagents/<id>/meta.json."
    )]
    pub subagent_id: String,

    /// When true, also attempt to delete the durable snapshot ref from the parent
    /// repo. Default false keeps `refs/grok/subagents/<id>` for recovery.
    #[serde(default)]
    #[schemars(
        description = "When true, also delete the snapshot ref from the parent repo. Default false keeps the ref for recovery."
    )]
    pub drop_snapshot: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DiscardSubagentOutput {
    pub subagent_id: String,
    pub success: bool,
    /// Whether a live worktree directory was removed.
    pub worktree_removed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_ref: Option<String>,
    /// True when the snapshot ref was deleted from the parent repo.
    pub snapshot_dropped: bool,
    pub message: String,
}

impl xai_tool_runtime::ToolOutput for DiscardSubagentOutput {
    fn model_output(&self) -> Vec<xai_tool_runtime::ContentBlock> {
        vec![xai_tool_runtime::ContentBlock::Text {
            text: self.message.clone(),
        }]
    }
}

#[derive(Debug, Default)]
pub struct DiscardSubagentTool;

impl crate::types::tool_metadata::ToolMetadata for DiscardSubagentTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        r#"Discard a subagent's live worktree after review (does not land changes).

Resolves `subagents/<subagent_id>/meta.json` and:
1. Removes the live worktree directory when still present (best-effort)
2. Always marks terminal meta: `land_status=discarded`, `worktree_state=cleaned`,
   clears `worktree_path`, and never leaves `status=running`
3. Keeps `snapshot_ref` / `changes.patch` by default so recovery remains possible
4. Optionally drops the snapshot ref when `drop_snapshot=true` (`snapshot_dropped`
   is true only when the ref was actually deleted)

Use after deciding not to `land_subagent`. Isolation is preserved until land;
discard only cleans local disk / optional refs."#
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

impl xai_tool_runtime::Tool for DiscardSubagentTool {
    type Args = DiscardSubagentInput;
    type Output = DiscardSubagentOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(DISCARD_SUBAGENT_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            DISCARD_SUBAGENT_TOOL_NAME,
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
        name = "tool.discard_subagent",
        skip_all,
        fields(subagent_id = %input.subagent_id)
    )]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: DiscardSubagentInput,
    ) -> Result<DiscardSubagentOutput, xai_tool_runtime::ToolError> {
        let work = resolve_subagent_work(&ctx, &input.subagent_id).await?;
        let drop_snapshot = input.drop_snapshot.unwrap_or(false);

        let mut worktree_removed = false;
        let mut remove_err: Option<String> = None;
        if let Some(ref wt) = work.live_worktree {
            // Clear live marker first so concurrent keep-N prune cannot race a
            // half-deleted RUNNING tree (RC13 Wave A).
            let marker = wt.join(".grok-subagent-live");
            let _ = tokio::fs::remove_file(&marker).await;
            // Windows MAX_PATH: use long-path-aware removal (same as
            // xai-fast-worktree). Plain remove_dir_all fails on deep
            // `.godot/imported` / nested node_modules trees.
            let wt_path = wt.clone();
            match tokio::task::spawn_blocking(move || xai_grok_paths::remove_dir_all_long(&wt_path))
                .await
            {
                Ok(Ok(())) => {
                    worktree_removed = true;
                    tracing::info!(
                        subagent_id = %work.subagent_id,
                        path = %wt.display(),
                        "discard_subagent removed live worktree"
                    );
                }
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Already gone — still terminal-clean meta below.
                    worktree_removed = true;
                }
                Ok(Err(e)) => {
                    remove_err = Some(format!(
                        "failed to remove live worktree {}: {e}",
                        wt.display()
                    ));
                }
                Err(e) => {
                    remove_err = Some(format!(
                        "failed to remove live worktree {}: {e}",
                        wt.display()
                    ));
                }
            }
        } else {
            // No live path claimed / on disk — discard still terminal-cleans meta.
            worktree_removed = false;
        }

        // snapshot_dropped is honest: true only when we actually deleted a ref.
        let mut snapshot_dropped = false;
        let snapshot_ref = work.snapshot_ref.clone();
        if drop_snapshot {
            if let Some(ref snap) = snapshot_ref {
                match super::git_capture(&work.parent_git_root, &["update-ref", "-d", snap]).await {
                    Ok(_) => {
                        snapshot_dropped = true;
                    }
                    Err(e) => {
                        // Keep snapshot_dropped=false — do not claim drop on failure.
                        tracing::warn!(
                            subagent_id = %work.subagent_id,
                            snapshot_ref = %snap,
                            error = %e,
                            "discard_subagent could not drop snapshot ref"
                        );
                    }
                }
            }
            // drop_snapshot=true but no snapshot_ref → snapshot_dropped stays false.
        }

        // RC13 Wave A: always terminal meta — land_status=discarded,
        // worktree_state=cleaned, status not left running, clear worktree_path.
        // Must run even when remove_dir_all failed so the session is not left
        // `running` / `land_status=pending` after a discard attempt.
        update_meta_discarded(&work.meta_path, snapshot_dropped).await;
        if let Some(msg) = remove_err {
            return Err(xai_tool_runtime::ToolError::custom(
                "worktree_remove_failed",
                msg,
            ));
        }

        let mut message = format!(
            "Discarded subagent `{}`: worktree_removed={worktree_removed}, snapshot_dropped={snapshot_dropped}, \
             land_status=discarded, worktree_state=cleaned.",
            work.subagent_id
        );
        if let Some(ref snap) = snapshot_ref {
            if snapshot_dropped {
                message.push_str(&format!(" Snapshot ref `{snap}` deleted."));
            } else if drop_snapshot {
                message.push_str(&format!(
                    " Snapshot ref `{snap}` retained (drop failed or ref already absent)."
                ));
            } else {
                message.push_str(&format!(
                    " Snapshot ref `{snap}` retained (pass drop_snapshot=true to delete)."
                ));
            }
        }
        if work.patch_path.is_some() {
            message.push_str(" changes.patch retained under the session subagent dir.");
        }

        Ok(DiscardSubagentOutput {
            subagent_id: work.subagent_id,
            success: true,
            worktree_removed,
            snapshot_ref: if snapshot_dropped { None } else { snapshot_ref },
            snapshot_dropped,
            message,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::implementations::grok_build::subagent_worktree::update_meta_discarded;

    #[tokio::test]
    async fn update_meta_discarded_is_always_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let meta_path = dir.path().join("meta.json");
        let initial = serde_json::json!({
            "subagent_id": "s1",
            "status": "running",
            "worktree_path": "/tmp/subagent-s1",
            "worktree_state": "live",
            "land_status": "pending",
            "snapshot_ref": "refs/grok/subagents/s1",
        });
        tokio::fs::write(&meta_path, serde_json::to_string_pretty(&initial).unwrap())
            .await
            .unwrap();

        update_meta_discarded(&meta_path, false).await;

        let raw = tokio::fs::read_to_string(&meta_path).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["land_status"], "discarded");
        assert_eq!(v["worktree_state"], "cleaned");
        assert_eq!(v["status"], "cancelled"); // was running
        assert!(v["worktree_path"].is_null());
        // snapshot not dropped → ref retained
        assert_eq!(v["snapshot_ref"], "refs/grok/subagents/s1");
    }

    #[tokio::test]
    async fn update_meta_discarded_clears_snapshot_when_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let meta_path = dir.path().join("meta.json");
        let initial = serde_json::json!({
            "subagent_id": "s2",
            "status": "completed",
            "worktree_state": "preserved",
            "land_status": "pending",
            "snapshot_ref": "refs/grok/subagents/s2",
        });
        tokio::fs::write(&meta_path, serde_json::to_string_pretty(&initial).unwrap())
            .await
            .unwrap();

        update_meta_discarded(&meta_path, true).await;

        let raw = tokio::fs::read_to_string(&meta_path).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["land_status"], "discarded");
        assert_eq!(v["worktree_state"], "cleaned");
        assert_eq!(v["status"], "completed"); // not running — leave terminal status
        assert!(v["snapshot_ref"].is_null());
    }

    #[tokio::test]
    async fn discard_updates_meta_even_when_remove_fails() {
        // Contract: even if remove_dir_all fails, discard still writes
        // terminal meta before returning worktree_remove_failed.
        let dir = tempfile::tempdir().unwrap();
        let meta_path = dir.path().join("meta.json");
        let initial = serde_json::json!({
            "subagent_id": "s3",
            "status": "running",
            "worktree_path": "/tmp/subagent-s3",
            "worktree_state": "live",
            "land_status": "pending",
        });
        tokio::fs::write(&meta_path, serde_json::to_string_pretty(&initial).unwrap())
            .await
            .unwrap();
        update_meta_discarded(&meta_path, false).await;
        let err = xai_tool_runtime::ToolError::custom(
            "worktree_remove_failed",
            "failed to remove live worktree /tmp/subagent-s3: simulated",
        );
        assert_eq!(err.kind, xai_tool_runtime::ToolErrorKind::Custom);
        assert_eq!(
            err.details
                .as_ref()
                .and_then(|v| v.get("code"))
                .and_then(|v| v.as_str()),
            Some("worktree_remove_failed")
        );
        let raw = tokio::fs::read_to_string(&meta_path).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["land_status"], "discarded");
        assert_eq!(v["status"], "cancelled");
        assert!(v["worktree_path"].is_null());
    }
}
