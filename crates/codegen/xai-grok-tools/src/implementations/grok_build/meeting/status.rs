//! `meeting_status` — current notetaker snapshot.

use super::text_output;
use crate::types::output::ToolOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

pub const MEETING_STATUS_TOOL_NAME: &str = "meeting_status";

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct MeetingStatusInput {}

#[derive(Debug, Default)]
pub struct MeetingStatusTool;

impl crate::types::tool_metadata::ToolMetadata for MeetingStatusTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Show whether Turbo's meeting notetaker is recording, plus platform, capture source, and transcript path."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

impl xai_tool_runtime::Tool for MeetingStatusTool {
    type Args = MeetingStatusInput;
    type Output = ToolOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(MEETING_STATUS_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            MEETING_STATUS_TOOL_NAME,
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

    #[tracing::instrument(name = "tool.meeting_status", skip_all)]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        _input: MeetingStatusInput,
    ) -> Result<ToolOutput, xai_tool_runtime::ToolError> {
        let handle = super::require_handle(&ctx).await?;
        Ok(text_output(handle.status_text()?))
    }
}
