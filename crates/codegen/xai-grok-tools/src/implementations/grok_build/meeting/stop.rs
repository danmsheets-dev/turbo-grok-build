//! `meeting_stop` — end capture and finalize the transcript.

use super::text_output;
use crate::types::output::ToolOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

pub const MEETING_STOP_TOOL_NAME: &str = "meeting_stop";

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct MeetingStopInput {}

#[derive(Debug, Default)]
pub struct MeetingStopTool;

impl crate::types::tool_metadata::ToolMetadata for MeetingStopTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Stop the Fathom-style meeting notetaker for this session. Flushes the transcript. Then load meeting_transcript and call meeting_notes with a work-only recap so it is saved as Meetings/YYYY-MM-DD - <name>.md in the launch work folder."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for MeetingStopTool {
    type Args = MeetingStopInput;
    type Output = ToolOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(MEETING_STOP_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            MEETING_STOP_TOOL_NAME,
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

    #[tracing::instrument(name = "tool.meeting_stop", skip_all)]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        _input: MeetingStopInput,
    ) -> Result<ToolOutput, xai_tool_runtime::ToolError> {
        let handle = super::require_handle(&ctx).await?;
        Ok(text_output(handle.stop()?))
    }
}
