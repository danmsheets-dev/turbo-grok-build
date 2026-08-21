//! `browser_snapshot` — compact accessibility tree from the Agent WebView.

use crate::types::output::ToolOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

pub const BROWSER_SNAPSHOT_TOOL_NAME: &str = "browser_snapshot";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct BrowserSnapshotInput {
    /// Raise the node cap (200 → 800) when true.
    #[serde(default)]
    #[schemars(
        description = "When true, raise the accessibility node cap from 200 to 800. Default false."
    )]
    pub verbose: bool,
    /// Include truncated main-landmark / article text.
    #[serde(default)]
    #[schemars(
        description = "When true, include truncated main/article text so job descriptions and profile Experience survive the node cap."
    )]
    pub include_text: bool,
}

#[derive(Debug, Default)]
pub struct BrowserSnapshotTool;

impl crate::types::tool_metadata::ToolMetadata for BrowserSnapshotTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Take a compact accessibility snapshot of the Turbo Agent WebView (url, title, uid/role/name/value/focused). Use uids with browser_click / browser_fill. Prefer this over guessing the DOM."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

impl xai_tool_runtime::Tool for BrowserSnapshotTool {
    type Args = BrowserSnapshotInput;
    type Output = ToolOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(BROWSER_SNAPSHOT_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            BROWSER_SNAPSHOT_TOOL_NAME,
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

    #[tracing::instrument(name = "tool.browser_snapshot", skip_all, fields(verbose = input.verbose))]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: BrowserSnapshotInput,
    ) -> Result<ToolOutput, xai_tool_runtime::ToolError> {
        let handle = super::require_handle(&ctx).await?;
        let result = handle
            .snapshot_ex(input.verbose, input.include_text)
            .await?;
        let mut lines = vec![
            format!("url: {}", result.url),
            format!("title: {}", result.title),
            format!("nodes: {}", result.nodes.len()),
        ];
        if result.overlay == Some(true) {
            lines.push(
                "note: a dialog/overlay is open. Click its Close uid before interacting with the page underneath."
                    .to_owned(),
            );
        }
        if !result.source.uids_are_actionable() {
            lines.push(
                "note: accessibility-tree fallback — these uids are READ-ONLY and cannot be \
                 used with browser_click or browser_fill. Snapshot again for actionable uids."
                    .to_owned(),
            );
        }
        if let Some(text) = result.text.as_deref().filter(|s| !s.is_empty()) {
            lines.push(format!("text: {text}"));
        }
        for node in result.nodes {
            let mut line = format!("- uid={} role={} name={:?}", node.uid, node.role, node.name);
            if let Some(value) = node.value {
                line.push_str(&format!(" value={value:?}"));
            }
            if node.focused {
                line.push_str(" focused");
            }
            lines.push(line);
        }
        Ok(super::untrusted_page_text(lines.join("\n")))
    }
}
