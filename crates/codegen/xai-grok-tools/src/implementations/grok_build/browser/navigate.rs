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
        "Navigate the Turbo Agent WebView to a URL. Direct zip/pdf/binary URLs are brokered into the session downloads folder instead of a silent no-op. Use this instead of inventing page contents when the user needs a real browser (JS, login UI, or interactive docs). Prefer ${{ tools.by_kind.web_fetch }} for static pages. Never automate passwords or 2FA — the human signs in in the Agent window if needed."
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
        xai_grok_browser::check_url_in_session(&input.url, handle.session_folder())
            .map_err(|e| xai_tool_runtime::ToolError::invalid_arguments(e.to_string()))?;
        let looks_http = {
            let trimmed = input.url.trim();
            let scheme = trimmed.split(':').next().unwrap_or("").to_ascii_lowercase();
            scheme == "http" || scheme == "https"
        };
        if looks_http && super::save::url_looks_like_direct_download(&input.url) {
            let session_folder = handle.session_folder().ok_or_else(|| {
                xai_tool_runtime::ToolError::invalid_arguments(
                    "browser_navigate: binary/PDF URL requires a session folder so it can be brokered as a download",
                )
            })?;
            let dest = super::save::broker_http_or_file(&input.url, session_folder).await?;
            let name = dest
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("download.bin");
            return Ok(super::text_output(format!(
                "Saved download {name} to {} (direct binary/PDF URL; not opened as a page). List with browser_downloads.",
                dest.display()
            )));
        }
        let result = handle.navigate(input.url).await?;
        if result.title.to_ascii_lowercase().contains("saved download")
            || result
                .title
                .to_ascii_lowercase()
                .contains("download in progress")
        {
            return Ok(super::text_output(format!(
                "Navigated to {} (title: {}). A download was brokered into the session folder; list it with browser_downloads.",
                result.url, result.title
            )));
        }
        Ok(super::text_output(format!(
            "Navigated to {} (title: {})",
            result.url, result.title
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::implementations::grok_build::browser::BrowserHandle;
    use crate::types::output::ToolOutput;
    use crate::types::resources::Resources;
    use crate::types::tool_metadata::test_ctx_with_call_id;

    #[tokio::test]
    async fn zip_url_is_brokered_instead_of_silent_navigate() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/zip")
                    .set_body_bytes(b"PK\x03\x04zip"),
            )
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let session = tmp.path().join("session");
        std::fs::create_dir_all(&session).unwrap();
        let resources = {
            let mut resources = Resources::new();
            resources.insert(BrowserHandle::mock_with_folder(
                "sess-nav-zip",
                session.clone(),
            ));
            resources.into_shared()
        };
        let out = xai_tool_runtime::Tool::run(
            &BrowserNavigateTool,
            test_ctx_with_call_id(resources.clone(), "zip"),
            BrowserNavigateInput {
                url: format!("{}/file.zip", server.uri()),
            },
        )
        .await
        .expect("zip navigate should broker");
        let ToolOutput::Text(text) = out else {
            panic!("expected text, got {out:?}");
        };
        assert!(text.text.contains("Saved download"), "{}", text.text);
        assert!(text.text.contains("file.zip"), "{}", text.text);
        let dest = session.join("downloads").join("file.zip");
        assert!(dest.is_file(), "missing {}", dest.display());
        let res = resources.lock().await;
        let handle = res.require::<BrowserHandle>().unwrap();
        assert!(
            handle.mock_host().unwrap().call_log().is_empty(),
            "host navigate must not run for a direct zip URL"
        );
    }
}
