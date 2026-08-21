//! `spawn_many` — fan-out multiple subagent Task spawns with optional barrier wait.
//!
//! Composes the existing [`super::task::TaskTool`] / coordinator queue
//! (`DEFAULT_MAX_CONCURRENT_SUBAGENTS` = 4, FIFO). Does **not** invent a second
//! orchestrator: each item is a normal background `task` spawn; optional
//! `wait=true` reuses `get_task_output` multi-id wait-all.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::implementations::grok_build::task::TaskTool;
use crate::implementations::grok_build::task_output::TaskOutputTool;
use crate::types::output::ToolOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};
use xai_tool_types::{
    MAX_MULTI_WAIT_IDS, SubagentCapabilityMode, SubagentIsolationMode, TaskToolInput,
    default_subagent_type,
};

pub const SPAWN_MANY_TOOL_NAME: &str = "spawn_many";

/// Cap aligns with multi-id wait / coordinator fan-out budget.
pub const MAX_SPAWN_MANY: usize = MAX_MULTI_WAIT_IDS;

/// One spawn request — TaskToolInput subset (no resume/cwd nesting).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SpawnManySpec {
    /// The full task prompt for the subagent to execute.
    #[schemars(description = "The full task prompt for the subagent to execute.")]
    pub prompt: String,

    /// Short description of the task (3-5 words).
    #[schemars(description = "Short description of the task (3-5 words).")]
    pub description: String,

    /// Subagent type (default `general-purpose`).
    #[schemars(
        description = "Subagent type: general-purpose, explore, plan, oracle, xdotcom, or a user-defined type."
    )]
    #[serde(default = "default_subagent_type")]
    pub subagent_type: String,

    /// Optional model slug for this child.
    #[schemars(description = "Optional model slug for this subagent.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Hard wall-clock limit for this child in milliseconds.
    #[schemars(
        description = "Hard wall-clock limit for this child in milliseconds (cancels the child when exceeded)."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,

    /// Isolation mode: worktree (default when omitted on task path) or none.
    #[schemars(description = "Isolation mode: \"worktree\" or \"none\".")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<SubagentIsolationMode>,

    /// Capability mode for the child.
    #[schemars(
        description = "Capability mode: \"read-only\", \"read-write\", \"execute\", or \"all\"."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_mode: Option<SubagentCapabilityMode>,

    /// Keep worktree on disk after snapshot when isolation=worktree.
    #[schemars(
        description = "When true with worktree isolation, keep the child worktree after snapshot."
    )]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "xai_tool_types::serde_lenient::deserialize_lenient_option_bool"
    )]
    pub retain_worktree: Option<bool>,
}

/// Input for `spawn_many`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SpawnManyInput {
    /// Spawn specs to enqueue (1..=MAX_SPAWN_MANY). Empty array is rejected.
    #[schemars(
        description = "Array of spawn specs (prompt, description, subagent_type, optional model/timeout_ms/isolation/capability_mode/retain_worktree). Max 20. Empty array is rejected."
    )]
    #[serde(alias = "spawns", alias = "agents", alias = "items")]
    pub tasks: Vec<SpawnManySpec>,

    /// When true, wait until all spawned tasks complete (or `timeout_ms`).
    #[schemars(
        description = "If true, barrier-wait until all spawned subagents complete (or timeout). Uses get_task_output multi-id wait-all. Default false: return ids immediately (coordinator still caps concurrency at 4 and queues the rest)."
    )]
    #[serde(
        default,
        deserialize_with = "xai_tool_types::serde_lenient::deserialize_lenient_bool"
    )]
    pub wait: bool,

    /// Barrier wait budget when `wait=true` (milliseconds). Default 600000 (10m).
    #[schemars(
        description = "When wait=true, max wait in milliseconds for all children (default 600000). Omit/0 with wait=false is ignored."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SpawnManyTaskResult {
    pub subagent_id: String,
    pub description: String,
    pub subagent_type: String,
    /// `spawned`, `failed`, or terminal status from barrier wait (`completed`, `running`, …).
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SpawnManyOutput {
    pub tasks: Vec<SpawnManyTaskResult>,
    pub waited: bool,
    pub message: String,
}

impl xai_tool_runtime::ToolOutput for SpawnManyOutput {
    fn model_output(&self) -> Vec<xai_tool_runtime::ContentBlock> {
        vec![xai_tool_runtime::ContentBlock::Text {
            text: self.message.clone(),
        }]
    }
}

#[derive(Debug, Default)]
pub struct SpawnManyTool;

impl crate::types::tool_metadata::ToolMetadata for SpawnManyTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        r#"Fan-out: spawn multiple subagents in one call (compose Task + coordinator; max 4 concurrent, FIFO queue for the rest).

Each entry is a Task-shaped spawn (prompt, description, subagent_type, optional model / timeout_ms / isolation / capability_mode / retain_worktree). Returns subagent ids + status immediately.

**wait** (default false): when true, barrier-wait until all complete using the same multi-id wait path as get_task_output (timeout_ms, default 10 minutes). Prefer wait=false for large matrices and poll with get_task_output yourself.

Empty `tasks` is rejected. Max 20 entries per call. Does not bypass isolation or land — use diff_subagent / land_subagent after review."#
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        use crate::types::tool_metadata::ToolMetadata as TM;
        // Same companions as Task: need retrieval + kill for background children.
        Expr::And(vec![
            Expr::Value(ToolRequirement::Tool {
                namespace: TM::tool_namespace(&TaskTool).to_string(),
                id: xai_tool_runtime::Tool::id(&TaskTool).to_string(),
                if_params: None,
            }),
            Expr::Value(ToolRequirement::tool_kind(ToolKind::BackgroundTaskAction)),
            Expr::Value(ToolRequirement::tool_kind(ToolKind::KillTaskAction)),
        ])
    }

    fn is_read_only(&self) -> bool {
        false
    }
}

impl xai_tool_runtime::Tool for SpawnManyTool {
    type Args = SpawnManyInput;
    type Output = SpawnManyOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(SPAWN_MANY_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            SPAWN_MANY_TOOL_NAME,
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
        name = "tool.spawn_many",
        skip_all,
        fields(task_count = %input.tasks.len(), wait = %input.wait)
    )]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: SpawnManyInput,
    ) -> Result<SpawnManyOutput, xai_tool_runtime::ToolError> {
        if input.tasks.is_empty() {
            return Err(xai_tool_runtime::ToolError::invalid_arguments(
                "spawn_many: tasks must not be empty".to_string(),
            ));
        }
        if input.tasks.len() > MAX_SPAWN_MANY {
            return Err(xai_tool_runtime::ToolError::invalid_arguments(format!(
                "spawn_many: tasks exceeds maximum of {MAX_SPAWN_MANY} entries"
            )));
        }

        let mut results = Vec::with_capacity(input.tasks.len());
        let mut spawned_ids = Vec::with_capacity(input.tasks.len());
        let task_tool = TaskTool;

        for spec in &input.tasks {
            let id = uuid::Uuid::now_v7().to_string();
            let task_input = TaskToolInput {
                prompt: spec.prompt.clone(),
                description: spec.description.clone(),
                subagent_type: if spec.subagent_type.trim().is_empty() {
                    default_subagent_type()
                } else {
                    spec.subagent_type.clone()
                },
                run_in_background: true,
                capability_mode: spec.capability_mode,
                isolation: spec.isolation,
                resume_from: None,
                cwd: None,
                model: spec.model.clone(),
                reasoning_effort: None,
                timeout_ms: spec.timeout_ms,
                retain_worktree: spec.retain_worktree,
                allowed_paths: None,
                task_id: Some(id.clone()),
            };

            match task_tool.run(ctx.clone(), task_input).await {
                Ok(ToolOutput::Text(_)) | Ok(ToolOutput::SubagentCompleted(_)) => {
                    spawned_ids.push(id.clone());
                    results.push(SpawnManyTaskResult {
                        subagent_id: id,
                        description: spec.description.clone(),
                        subagent_type: if spec.subagent_type.trim().is_empty() {
                            default_subagent_type()
                        } else {
                            spec.subagent_type.clone()
                        },
                        status: "spawned".into(),
                        error: None,
                        output: None,
                    });
                }
                Ok(other) => {
                    results.push(SpawnManyTaskResult {
                        subagent_id: id,
                        description: spec.description.clone(),
                        subagent_type: spec.subagent_type.clone(),
                        status: "failed".into(),
                        error: Some(format!("unexpected task output variant: {other:?}")),
                        output: None,
                    });
                }
                Err(e) => {
                    results.push(SpawnManyTaskResult {
                        subagent_id: id,
                        description: spec.description.clone(),
                        subagent_type: spec.subagent_type.clone(),
                        status: "failed".into(),
                        error: Some(e.to_string()),
                        output: None,
                    });
                }
            }
        }

        let mut waited = false;
        if input.wait && !spawned_ids.is_empty() {
            waited = true;
            use crate::types::tool_metadata::shared_resources;
            let resources = shared_resources(&ctx)?;
            let barrier_ms = input.timeout_ms.filter(|ms| *ms > 0).unwrap_or(600_000);
            match TaskOutputTool::run_multi_tasks(
                &spawned_ids,
                Some(barrier_ms),
                resources,
                "spawn_many",
            )
            .await
            {
                Ok(xai_tool_types::TaskOutputOutput::MultiResult(multi)) => {
                    for r in multi.results {
                        if let Some(item) = results.iter_mut().find(|t| t.subagent_id == r.task_id)
                        {
                            item.status = r.status;
                            if !r.output.is_empty() {
                                item.output = Some(r.output);
                            }
                        }
                    }
                }
                Ok(xai_tool_types::TaskOutputOutput::Result(single)) => {
                    if let Some(item) = results.iter_mut().find(|t| t.subagent_id == single.task_id)
                    {
                        item.status = single.status;
                        if !single.output.is_empty() {
                            item.output = Some(single.output);
                        }
                    }
                }
                Ok(xai_tool_types::TaskOutputOutput::TaskNotFound(msg)) => {
                    for item in results.iter_mut().filter(|t| t.status == "spawned") {
                        item.status = "unknown".into();
                        item.error = Some(msg.clone());
                    }
                }
                Err(e) => {
                    for item in results.iter_mut().filter(|t| t.status == "spawned") {
                        item.error = Some(format!("barrier wait failed: {e}"));
                    }
                }
            }
        }

        let spawned = results.iter().filter(|t| t.status != "failed").count();
        let failed = results.iter().filter(|t| t.status == "failed").count();
        let mut message = format!(
            "spawn_many: {spawned}/{} enqueued (failed={failed})",
            input.tasks.len()
        );
        if waited {
            let done = results
                .iter()
                .filter(|t| t.status == "completed" || t.status == "failed")
                .count();
            message.push_str(&format!("; waited: {done}/{} terminal", results.len()));
        } else {
            message.push_str(
                "\nUse get_task_output with task_ids=[...] and timeout_ms to wait, \
                 or re-call spawn_many with wait=true.",
            );
        }
        message.push_str("\n\n## Tasks\n");
        for t in &results {
            message.push_str(&format!(
                "- `{}` ({}) status={} desc={}\n",
                t.subagent_id, t.subagent_type, t.status, t.description
            ));
            if let Some(ref err) = t.error {
                message.push_str(&format!("  error: {err}\n"));
            }
        }

        Ok(SpawnManyOutput {
            tasks: results,
            waited,
            message,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_minimal_tasks() {
        let raw = serde_json::json!({
            "tasks": [
                {
                    "prompt": "check foo",
                    "description": "check foo",
                    "subagent_type": "explore"
                }
            ]
        });
        let input: SpawnManyInput = serde_json::from_value(raw).expect("deser");
        assert_eq!(input.tasks.len(), 1);
        assert!(!input.wait);
        assert_eq!(input.tasks[0].subagent_type, "explore");
        assert_eq!(input.tasks[0].prompt, "check foo");
    }

    #[test]
    fn deserializes_spawns_alias_and_wait() {
        let raw = serde_json::json!({
            "spawns": [
                {
                    "prompt": "a",
                    "description": "a",
                    "model": "gpt-test",
                    "timeout_ms": 1000,
                    "retain_worktree": true
                }
            ],
            "wait": true,
            "timeout_ms": 5000
        });
        let input: SpawnManyInput = serde_json::from_value(raw).expect("deser");
        assert!(input.wait);
        assert_eq!(input.timeout_ms, Some(5000));
        assert_eq!(input.tasks[0].model.as_deref(), Some("gpt-test"));
        assert_eq!(input.tasks[0].timeout_ms, Some(1000));
        assert_eq!(input.tasks[0].retain_worktree, Some(true));
        // default subagent_type
        assert_eq!(input.tasks[0].subagent_type, "general-purpose");
    }

    #[test]
    fn deserializes_lenient_wait_string() {
        let raw = serde_json::json!({
            "agents": [{"prompt": "p", "description": "d"}],
            "wait": "true"
        });
        let input: SpawnManyInput = serde_json::from_value(raw).expect("deser");
        assert!(input.wait);
    }

    #[tokio::test]
    async fn empty_tasks_rejected() {
        use std::sync::Arc;
        use xai_tool_runtime::Tool as _;

        let tool = SpawnManyTool;
        let resources = Arc::new(tokio::sync::Mutex::new(
            crate::types::resources::Resources::default(),
        ));
        let ctx = crate::types::tool_metadata::test_ctx(resources);
        let err = tool
            .run(
                ctx,
                SpawnManyInput {
                    tasks: vec![],
                    wait: false,
                    timeout_ms: None,
                },
            )
            .await
            .expect_err("empty must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("empty") || msg.contains("must not be empty"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn rejects_over_cap_in_validation_shape() {
        // Cap constant must stay aligned with multi-wait.
        assert_eq!(MAX_SPAWN_MANY, MAX_MULTI_WAIT_IDS);
        assert_eq!(MAX_SPAWN_MANY, 20);
    }
}
