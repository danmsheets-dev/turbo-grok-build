//! `browser_navigate` — load a URL in the Agent WebView.

use crate::types::output::ToolOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

pub const BROWSER_NAVIGATE_TOOL_NAME: &str = "browser_navigate";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct BrowserNavigateInput {
    /// URL to load. `https:`, local `http:`, and `about:blank` are allowed.
    #[schemars(
        description = "URL to open in the Agent WebView. https is allowed; http only for localhost / RFC1918 / *.localhost; about:blank is allowed. file: is denied unless under the session folder."
    )]
    pub url: String,
}

#[derive(Debug, Default)]
pub struct BrowserNavigateTool;

impl crate::types::tool_metadata::ToolMetadata for BrowserNavigateTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Navigate the Turbo Agent WebView to a URL. Use this instead of inventing page contents when the user needs a real browser (JS, login UI, or interactive docs). Prefer ${{ tools.by_kind.web_fetch }} for static pages. Never automate passwords or 2FA — the human signs in in the Agent window if needed."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for BrowserNavigateTool {
    type Args = BrowserNavigateInput;
    type Output = ToolOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(BROWSER_NAVIGATE_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            BROWSER_NAVIGATE_TOOL_NAME,
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

    #[tracing::instrument(name = "tool.browser_navigate", skip_all, fields(url = %input.url))]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: BrowserNavigateInput,
    ) -> Result<ToolOutput, xai_tool_runtime::ToolError> {
        let handle = super::require_handle(&ctx).await?;
        let result = handle.navigate(input.url).await?;
        Ok(super::text_output(format!(
            "Navigated to {} (title: {})",
            result.url, result.title
        )))
    }
}
