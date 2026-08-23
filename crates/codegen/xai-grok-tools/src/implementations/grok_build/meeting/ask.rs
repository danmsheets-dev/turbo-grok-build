//! `meeting_ask` — load a briefing for a coworker `Turbo:` question.

use super::text_output;
use crate::types::output::ToolOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

pub const MEETING_ASK_TOOL_NAME: &str = "meeting_ask";

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct MeetingAskInput {
    #[serde(default)]
    #[schemars(
        description = "Question to answer from the launch workspace + meeting notes. Empty = next pending Turbo: question from chat/transcript."
    )]
    pub question: Option<String>,
}

#[derive(Debug, Default)]
pub struct MeetingAskTool;

impl crate::types::tool_metadata::ToolMetadata for MeetingAskTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Meeting
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Load a briefing for a coworker question about the operator's work (launch workspace + meeting notes + transcript). Then research the workspace with the best tools (files, MCP, web) and call meeting_reply. Empty question takes the next queued `Turbo:` item. Does not require a separate knowledge folder."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for MeetingAskTool {
    type Args = MeetingAskInput;
    type Output = ToolOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(MEETING_ASK_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            MEETING_ASK_TOOL_NAME,
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

    #[tracing::instrument(name = "tool.meeting_ask", skip_all)]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: MeetingAskInput,
    ) -> Result<ToolOutput, xai_tool_runtime::ToolError> {
        let handle = super::require_handle(&ctx).await?;
        let cwd = match crate::types::tool_metadata::shared_resources(&ctx) {
            Ok(resources) => crate::types::tool_metadata::resolve_cwd(&ctx, &resources)
                .await
                .ok(),
            Err(_) => None,
        };
        Ok(text_output(handle.ask(input.question.as_deref(), cwd.as_deref())?))
    }
}
