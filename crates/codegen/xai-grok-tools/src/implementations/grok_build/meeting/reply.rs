//! `meeting_reply` — post `[Turbo] …` to meeting chat (Graph) or save locally.

use super::text_output;
use crate::types::output::ToolOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

pub const MEETING_REPLY_TOOL_NAME: &str = "meeting_reply";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct MeetingReplyInput {
    #[schemars(description = "Answer to post. A [Turbo] prefix is added if missing.")]
    pub answer: String,
}

#[derive(Debug, Default)]
pub struct MeetingReplyTool;

impl crate::types::tool_metadata::ToolMetadata for MeetingReplyTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Meeting
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Send a Turbo answer to the meeting. Posts to meeting chat as \"Turbo (Notetaker)\" when the notetaker bot is in the meeting; otherwise falls back to Teams chat as the signed-in user when GROK_GRAPH_TOKEN is set. Always saves last_reply.md. Prefix [Turbo]."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for MeetingReplyTool {
    type Args = MeetingReplyInput;
    type Output = ToolOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(MEETING_REPLY_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            MEETING_REPLY_TOOL_NAME,
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

    #[tracing::instrument(name = "tool.meeting_reply", skip_all)]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: MeetingReplyInput,
    ) -> Result<ToolOutput, xai_tool_runtime::ToolError> {
        let handle = super::require_handle(&ctx).await?;
        Ok(text_output(handle.reply(&input.answer).await?))
    }
}
