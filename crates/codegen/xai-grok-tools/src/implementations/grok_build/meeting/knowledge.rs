//! `meeting_knowledge` — optional extra notes path for `Turbo:` Q&A.
//!
//! Not required. Turbo already researches the folder it was launched from.

use super::text_output;
use crate::types::output::ToolOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

pub const MEETING_KNOWLEDGE_TOOL_NAME: &str = "meeting_knowledge";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct MeetingKnowledgeInput {
    #[schemars(
        description = "Optional extra notes folder (already exists). Turbo already researches the launch workspace; this path is extra context only. Does not create files."
    )]
    pub path: String,
}

#[derive(Debug, Default)]
pub struct MeetingKnowledgeTool;

impl crate::types::tool_metadata::ToolMetadata for MeetingKnowledgeTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Meeting
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Optional extra notes path for coworker `Turbo:` questions. Not required — Turbo already researches the launch workspace with full tools (including MCP). Does not create a folder or projects.md."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for MeetingKnowledgeTool {
    type Args = MeetingKnowledgeInput;
    type Output = ToolOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(MEETING_KNOWLEDGE_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            MEETING_KNOWLEDGE_TOOL_NAME,
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

    #[tracing::instrument(name = "tool.meeting_knowledge", skip_all)]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: MeetingKnowledgeInput,
    ) -> Result<ToolOutput, xai_tool_runtime::ToolError> {
        let handle = super::require_handle(&ctx).await?;
        Ok(text_output(handle.attach_knowledge(input.path.trim())?))
    }
}
