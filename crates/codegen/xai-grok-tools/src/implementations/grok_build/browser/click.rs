//! `browser_click` — click a snapshot uid in the Agent WebView.

use crate::types::output::ToolOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

pub const BROWSER_CLICK_TOOL_NAME: &str = "browser_click";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct BrowserClickInput {
    /// Snapshot uid from the last `browser_snapshot`.
    #[schemars(description = "Snapshot uid from the last browser_snapshot (e.g. \"1\").")]
    pub uid: String,
}

#[derive(Debug, Default)]
pub struct BrowserClickTool;

impl crate::types::tool_metadata::ToolMetadata for BrowserClickTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Click an element in the Turbo Agent WebView by snapshot uid. Call browser_snapshot first. Unknown uids fail closed."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for BrowserClickTool {
    type Args = BrowserClickInput;
    type Output = ToolOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(BROWSER_CLICK_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            BROWSER_CLICK_TOOL_NAME,
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

    #[tracing::instrument(name = "tool.browser_click", skip_all, fields(uid = %input.uid))]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: BrowserClickInput,
    ) -> Result<ToolOutput, xai_tool_runtime::ToolError> {
        let handle = super::require_handle(&ctx).await?;
        handle.click(&input.uid).await?;
        Ok(super::text_output(format!("Clicked uid {}", input.uid)))
    }
}
