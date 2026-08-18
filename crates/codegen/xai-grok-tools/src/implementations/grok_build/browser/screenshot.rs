//! `browser_screenshot` — capture the Agent WebView to a PNG.

use crate::types::output::ToolOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

pub const BROWSER_SCREENSHOT_TOOL_NAME: &str = "browser_screenshot";

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct BrowserScreenshotInput {}

#[derive(Debug, Default)]
pub struct BrowserScreenshotTool;

impl crate::types::tool_metadata::ToolMetadata for BrowserScreenshotTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Capture a PNG screenshot of the Turbo Agent WebView. When telling the user where it was saved, refer to the short session-relative path (e.g. `images/browser-1.png`)."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

impl xai_tool_runtime::Tool for BrowserScreenshotTool {
    type Args = BrowserScreenshotInput;
    type Output = ToolOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(BROWSER_SCREENSHOT_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            BROWSER_SCREENSHOT_TOOL_NAME,
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

    #[tracing::instrument(name = "tool.browser_screenshot", skip_all)]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        _input: BrowserScreenshotInput,
    ) -> Result<ToolOutput, xai_tool_runtime::ToolError> {
        let handle = super::require_handle(&ctx).await?;
        let result = handle.screenshot().await?;
        Ok(super::text_output(format!(
            "Screenshot saved to {} ({}x{}).",
            result.path, result.width, result.height
        )))
    }
}
