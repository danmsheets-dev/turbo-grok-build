//! `browser_wait` — poll until text or a URL substring is present.

use crate::types::output::ToolOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

pub const BROWSER_WAIT_TOOL_NAME: &str = "browser_wait";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct BrowserWaitInput {
    #[serde(default)]
    #[schemars(description = "Case-insensitive text that must appear in the page.")]
    pub text: Option<String>,
    #[serde(default)]
    #[schemars(description = "Substring that must appear in the current URL.")]
    pub url_substring: Option<String>,
    #[serde(default)]
    #[schemars(description = "Timeout in milliseconds (default 15000, max 60000).")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Default)]
pub struct BrowserWaitTool;

impl crate::types::tool_metadata::ToolMetadata for BrowserWaitTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Wait until text is visible on the page or the URL contains a substring. Use after a click that loads SPA results. Does not require confirm."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

impl xai_tool_runtime::Tool for BrowserWaitTool {
    type Args = BrowserWaitInput;
    type Output = ToolOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(BROWSER_WAIT_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            BROWSER_WAIT_TOOL_NAME,
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

    #[tracing::instrument(name = "tool.browser_wait", skip_all)]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: BrowserWaitInput,
    ) -> Result<ToolOutput, xai_tool_runtime::ToolError> {
        if input.text.as_ref().is_none_or(|s| s.trim().is_empty())
            && input
                .url_substring
                .as_ref()
                .is_none_or(|s| s.trim().is_empty())
        {
            return Err(xai_tool_runtime::ToolError::invalid_arguments(
                "browser_wait requires text or url_substring".to_owned(),
            ));
        }
        let handle = super::require_handle(&ctx).await?;
        let result = handle
            .wait(input.text, input.url_substring, input.timeout_ms)
            .await?;
        Ok(super::text_output(format!(
            "Waited — now at {} (title: {})",
            result.url, result.title
        )))
    }
}
