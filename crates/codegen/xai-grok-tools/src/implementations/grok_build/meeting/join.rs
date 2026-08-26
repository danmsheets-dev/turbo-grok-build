//! `meeting_join` — start meeting notes for a join URL.

use super::text_output;
use crate::types::output::ToolOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

pub const MEETING_JOIN_TOOL_NAME: &str = "meeting_join";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct MeetingJoinInput {
    #[schemars(
        description = "Zoom, Teams, Meet, or Webex join URL (https). Teams tries a guest named Turbo (Notetaker) in the lobby; if that fails, capture is local WASAPI/mic. Other platforms record this machine's audio."
    )]
    pub url: String,
    #[serde(default, alias = "name")]
    #[schemars(
        description = "Optional meeting name for the work-folder summary (e.g. Weekly website standup). Graph subject is used when omitted. Alias: `name`."
    )]
    pub title: Option<String>,
}

#[derive(Debug, Default)]
pub struct MeetingJoinTool;

impl crate::types::tool_metadata::ToolMetadata for MeetingJoinTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Meeting
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Start Turbo's meeting notetaker. Pass a Zoom/Teams/Meet/Webex https join URL and optional title. For Teams, Turbo tries to join as a guest named \"Turbo (Notetaker)\" and waits in the lobby until an organizer admits it. That is a guest in the meeting, not a third-party Fathom bot. If the guest cannot join, capture falls back to this machine's WASAPI loopback + mic and the result says so. Other platforms always use local capture. Transcribed with Grok STT. When the meeting stops, a work-only summary is saved as Meetings/YYYY-MM-DD - <name>.md in the launch work folder."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for MeetingJoinTool {
    type Args = MeetingJoinInput;
    type Output = ToolOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(MEETING_JOIN_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            MEETING_JOIN_TOOL_NAME,
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

    #[tracing::instrument(name = "tool.meeting_join", skip_all)]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: MeetingJoinInput,
    ) -> Result<ToolOutput, xai_tool_runtime::ToolError> {
        let handle = super::require_handle(&ctx).await?;
        let title = input
            .title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let text = handle.join(input.url.trim(), title).await?;
        Ok(text_output(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_alias_deserializes_to_title() {
        let v: MeetingJoinInput = serde_json::from_str(
            r#"{"url":"https://teams.microsoft.com/meet/1","name":"Standup"}"#,
        )
        .unwrap();
        assert_eq!(v.url, "https://teams.microsoft.com/meet/1");
        assert_eq!(v.title.as_deref(), Some("Standup"));
        let v2: MeetingJoinInput = serde_json::from_str(
            r#"{"url":"https://zoom.us/j/1","title":"Retro"}"#,
        )
        .unwrap();
        assert_eq!(v2.title.as_deref(), Some("Retro"));
    }
}
