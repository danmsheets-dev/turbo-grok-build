//! `discard_subagent` — drop a live subagent worktree; keep snapshot ref by default.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{resolve_subagent_work, update_meta_land_status};
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
2. Marks `land_status=discarded` and clears the live path in meta
3. Keeps `snapshot_ref` / `changes.patch` by default so recovery remains possible
4. Optionally drops the snapshot ref when `drop_snapshot=true`

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
        if let Some(ref wt) = work.live_worktree {
            match tokio::fs::remove_dir_all(wt).await {
                Ok(()) => {
                    worktree_removed = true;
                    tracing::info!(
                        subagent_id = %work.subagent_id,
                        path = %wt.display(),
                        "discard_subagent removed live worktree"
                    );
                }
                Err(e) => {
                    return Err(xai_tool_runtime::ToolError::custom(
                        "worktree_remove_failed",
                        format!("failed to remove live worktree {}: {e}", wt.display()),
                    ));
                }
            }
        }

        let mut snapshot_dropped = false;
        let snapshot_ref = work.snapshot_ref.clone();
        if drop_snapshot {
            if let Some(ref snap) = snapshot_ref {
                match super::git_capture(&work.parent_git_root, &["update-ref", "-d", snap]).await {
                    Ok(_) => {
                        snapshot_dropped = true;
                    }
                    Err(e) => {
                        tracing::warn!(
                            subagent_id = %work.subagent_id,
                            snapshot_ref = %snap,
                            error = %e,
                            "discard_subagent could not drop snapshot ref"
                        );
                    }
                }
            }
        }

        // Best-effort meta update: land_status + clear live path + optional state.
        if let Ok(raw) = tokio::fs::read_to_string(&work.meta_path).await
            && let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&raw)
            && let Some(obj) = value.as_object_mut()
        {
            obj.insert(
                "land_status".to_owned(),
                serde_json::Value::String("discarded".to_owned()),
            );
            if worktree_removed {
                obj.insert("worktree_path".to_owned(), serde_json::Value::Null);
                obj.insert(
                    "worktree_state".to_owned(),
                    serde_json::Value::String("cleaned".to_owned()),
                );
            }
            if snapshot_dropped {
                obj.insert("snapshot_ref".to_owned(), serde_json::Value::Null);
            }
            if let Ok(pretty) = serde_json::to_string_pretty(&value) {
                let _ = tokio::fs::write(&work.meta_path, pretty).await;
            }
        } else {
            update_meta_land_status(&work.meta_path, "discarded").await;
        }

        let mut message = format!(
            "Discarded subagent `{}`: worktree_removed={worktree_removed}, snapshot_dropped={snapshot_dropped}.",
            work.subagent_id
        );
        if let Some(ref snap) = snapshot_ref {
            if snapshot_dropped {
                message.push_str(&format!(" Snapshot ref `{snap}` deleted."));
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
            snapshot_ref: if snapshot_dropped {
                None
            } else {
                snapshot_ref
            },
            snapshot_dropped,
            message,
        })
    }
}
