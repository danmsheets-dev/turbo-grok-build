//! `browser_scroll` — scroll a uid into view or the window by a delta.

use crate::types::output::ToolOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

pub const BROWSER_SCROLL_TOOL_NAME: &str = "browser_scroll";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct BrowserScrollInput {
    #[serde(default)]
    #[schemars(description = "Snapshot uid to scroll into view (e.g. \"4-17\").")]
    pub uid: Option<String>,
    #[serde(default)]
    #[schemars(description = "Horizontal scroll delta in CSS pixels.")]
    pub dx: Option<i32>,
    #[serde(default)]
    #[schemars(description = "Vertical scroll delta in CSS pixels (positive is down).")]
    pub dy: Option<i32>,
}

#[derive(Debug, Default)]
pub struct BrowserScrollTool;

impl crate::types::tool_metadata::ToolMetadata for BrowserScrollTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Scroll the Turbo Agent WebView. Pass a snapshot uid to bring that element into view, or dx/dy to scroll the window."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for BrowserScrollTool {
    type Args = BrowserScrollInput;
    type Output = ToolOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(BROWSER_SCROLL_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            BROWSER_SCROLL_TOOL_NAME,
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

    #[tracing::instrument(name = "tool.browser_scroll", skip_all)]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: BrowserScrollInput,
    ) -> Result<ToolOutput, xai_tool_runtime::ToolError> {
        let handle = super::require_handle(&ctx).await?;
        handle.scroll(input.uid, input.dx, input.dy).await?;
        Ok(super::text_output("Scrolled"))
    }
}
