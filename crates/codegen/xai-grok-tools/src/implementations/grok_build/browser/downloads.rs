//! `browser_downloads` — list files saved by the session-scoped download broker.

use crate::types::output::ToolOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

pub const BROWSER_DOWNLOADS_TOOL_NAME: &str = "browser_downloads";

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct BrowserDownloadsInput {
    #[schemars(
        description = "Optional max wait in milliseconds for a completed brokered file to appear \
            (JS download interstitials). Omit or 0 to list immediately."
    )]
    #[serde(default)]
    pub wait_ms: Option<u64>,
    #[schemars(description = "Optional substring filter on the brokered filename.")]
    #[serde(default)]
    pub name_contains: Option<String>,
}

#[derive(Debug, Default)]
pub struct BrowserDownloadsTool;

impl crate::types::tool_metadata::ToolMetadata for BrowserDownloadsTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "List completed files in the session-scoped Agent WebView downloads folder. Downloads are brokered under the session directory; this tool does not open or submit files."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

impl xai_tool_runtime::Tool for BrowserDownloadsTool {
    type Args = BrowserDownloadsInput;
    type Output = ToolOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(BROWSER_DOWNLOADS_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            BROWSER_DOWNLOADS_TOOL_NAME,
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

    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: BrowserDownloadsInput,
    ) -> Result<ToolOutput, xai_tool_runtime::ToolError> {
        let handle = super::require_handle(&ctx).await?;
        let wait_ms = input.wait_ms.unwrap_or(0);
        let filter = input
            .name_contains
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_ascii_lowercase);
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(wait_ms);
        loop {
            let mut result = handle.downloads().await?;
            if let Some(ref needle) = filter {
                result
                    .downloads
                    .retain(|d| d.name.to_ascii_lowercase().contains(needle));
            }
            let has_completed = result.downloads.iter().any(|d| d.completed);
            if has_completed || wait_ms == 0 || std::time::Instant::now() >= deadline {
                return Ok(super::json_output(&result));
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }
}
