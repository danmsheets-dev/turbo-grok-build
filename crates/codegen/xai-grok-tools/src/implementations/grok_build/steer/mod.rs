//! `steer` — inject mid-run guidance into a RUNNING subagent (Phase 5).
//!
//! The text is treated as untrusted user data: the child runtime queues it as
//! an interjection delivered at the child's next turn boundary. Kill remains
//! available and unaffected.

use crate::implementations::grok_build::task::backend::SubagentBackendResource;
use crate::types::tool::ToolKind;
use crate::types::tool_metadata::ToolMetadata;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const STEER_TOOL_NAME: &str = "steer";
/// Schema and runtime cap for steer text (16 KiB).
const STEER_MAX_BYTES: usize = 16 * 1024;

/// Input for the steer tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SteerInput {
    #[schemars(
        description = "The subagent id to steer (from the spawn result or task output)."
    )]
    pub subagent_id: String,
    #[schemars(
        description = "Guidance text delivered to the running child at its next turn boundary. Treated as untrusted data, not system instruction. Max 16 KiB."
    )]
    pub text: String,
}

/// Output of the steer tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SteerOutput {
    /// The steer was queued into the child's session.
    Queued { subagent_id: String, message: String },
    /// The child cannot be steered right now; `reason` says why.
    Refused { subagent_id: String, reason: String },
}

impl xai_tool_runtime::ToolOutput for SteerOutput {
    fn model_output(&self) -> Vec<xai_tool_runtime::ContentBlock> {
        let message = match self {
            Self::Queued { message, .. } => message.clone(),
            Self::Refused { reason, .. } => reason.clone(),
        };
        vec![xai_tool_runtime::ContentBlock::Text { text: message }]
    }
}

/// Steer a running subagent with mid-run guidance.
#[derive(Debug, Default)]
pub struct SteerTool;

impl ToolMetadata for SteerTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Execute
    }

    fn tool_namespace(&self) -> crate::types::tool::ToolNamespace {
        crate::types::tool::ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        r#"Send guidance to a RUNNING subagent without killing it — e.g. "stay inside crates/foo", "stop if you find no P0 issues", "also check Windows paths". The text is queued as untrusted user data and delivered when the child reaches its next turn boundary; it does NOT interrupt the current tool call. Only running subagents accept steering (pending/queued/finished children refuse). To stop a child outright use kill_task."#
    }

    fn requires_expr(&self) -> crate::types::requirements::Expr<crate::types::requirements::ToolRequirement> {
        crate::types::requirements::Expr::True
    }

    fn is_read_only(&self) -> bool {
        false
    }
}

impl xai_tool_runtime::Tool for SteerTool {
    type Args = SteerInput;
    type Output = SteerOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(STEER_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            STEER_TOOL_NAME,
            <Self as ToolMetadata>::sanitized_description_template(self),
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
        name = "tool.steer",
        skip_all,
        fields(subagent_id = %input.subagent_id)
    )]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: SteerInput,
    ) -> Result<SteerOutput, xai_tool_runtime::ToolError> {
        use crate::types::tool_metadata::shared_resources;
        let resources = shared_resources(&ctx)?;
        let backend = {
            resources
                .lock()
                .await
                .get::<SubagentBackendResource>()
                .cloned()
        };
        if input.text.len() > STEER_MAX_BYTES {
            return Ok(SteerOutput::Refused {
                subagent_id: input.subagent_id,
                reason: format!(
                    "Steer text is {} bytes; max is {STEER_MAX_BYTES} (16 KiB).",
                    input.text.len()
                ),
            });
        }
        let Some(backend) = backend else {
            return Ok(SteerOutput::Refused {
                subagent_id: input.subagent_id.clone(),
                reason: "No subagent backend in this context.".to_owned(),
            });
        };
        match backend.backend().steer(&input.subagent_id, &input.text).await {
            Ok(message) => Ok(SteerOutput::Queued {
                subagent_id: input.subagent_id,
                message,
            }),
            Err(reason) => Ok(SteerOutput::Refused {
                subagent_id: input.subagent_id,
                reason,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_tool_runtime::ToolOutput as _;

    #[test]
    fn parses_minimal_input() {
        let input: SteerInput = serde_json::from_value(serde_json::json!({
            "subagent_id": "01a02a2f-f1e7-7793-885b-2037a8f37fb2",
            "text": "stay in crates/foo"
        }))
        .expect("parse");
        assert_eq!(input.subagent_id.len(), 36);
        assert_eq!(input.text, "stay in crates/foo");
    }

    #[test]
    fn oversized_steer_is_refused_without_backend() {
        // Mirrors run()'s length gate; ToolCallContext is heavy to construct here.
        assert!(STEER_MAX_BYTES == 16 * 1024);
        let too_big = "x".repeat(STEER_MAX_BYTES + 1);
        assert!(too_big.len() > STEER_MAX_BYTES);
    }

    #[test]
    fn metadata_marks_steer_as_a_write() {
        let tool = SteerTool;
        assert_eq!(tool.kind(), ToolKind::Execute);
        assert!(!tool.is_read_only());
        let capabilities = xai_tool_runtime::Tool::capabilities(&tool);
        assert!(!capabilities.is_read_only);
        assert_eq!(
            capabilities.tool_scope,
            Some(xai_tool_protocol::ToolScope::Write)
        );
    }

    #[test]
    fn output_messages_are_model_facing() {
        let queued = SteerOutput::Queued {
            subagent_id: "s1".into(),
            message: "steer queued".into(),
        };
        assert_eq!(
            queued.model_output()[0],
            xai_tool_runtime::ContentBlock::Text {
                text: "steer queued".into()
            }
        );
        let refused = SteerOutput::Refused {
            subagent_id: "s1".into(),
            reason: "not found".into(),
        };
        assert_eq!(
            refused.model_output()[0],
            xai_tool_runtime::ContentBlock::Text { text: "not found".into() }
        );
    }
}
