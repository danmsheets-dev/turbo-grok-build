//! `meeting_notes` — persist a markdown recap next to the transcript.

use super::text_output;
use crate::types::output::ToolOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

pub const MEETING_NOTES_TOOL_NAME: &str = "meeting_notes";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct MeetingNotesInput {
    #[schemars(
        description = "Work-only markdown recap. Saved as notes.md in the session folder and as Meetings/YYYY-MM-DD - <name>.md in the launch work folder."
    )]
    pub markdown: String,
}

#[derive(Debug, Default)]
pub struct MeetingNotesTool;

impl crate::types::tool_metadata::ToolMetadata for MeetingNotesTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Save a work-only meeting summary. Call meeting_transcript first. Writes notes.md next to the transcript and Meetings/YYYY-MM-DD - <Meeting Name>.md in the launch work folder. Drop small talk; keep only business/work content. Do not invent content."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for MeetingNotesTool {
    type Args = MeetingNotesInput;
    type Output = ToolOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(MEETING_NOTES_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            MEETING_NOTES_TOOL_NAME,
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

    #[tracing::instrument(name = "tool.meeting_notes", skip_all)]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: MeetingNotesInput,
    ) -> Result<ToolOutput, xai_tool_runtime::ToolError> {
        let handle = super::require_handle(&ctx).await?;
        let cwd = match crate::types::tool_metadata::shared_resources(&ctx) {
            Ok(resources) => crate::types::tool_metadata::resolve_cwd(&ctx, &resources)
                .await
                .ok(),
            Err(_) => None,
        };
        Ok(text_output(handle.write_notes(&input.markdown, cwd.as_deref())?))
    }
}
