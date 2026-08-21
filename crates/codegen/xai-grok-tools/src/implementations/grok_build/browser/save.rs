//! `browser_save` — broker the current document (or an explicit URL) into
//! the session-scoped downloads folder.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::types::output::ToolOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

use xai_grok_browser::{DownloadInfo, DownloadsResult, check_url_in_session};

pub const BROWSER_SAVE_TOOL_NAME: &str = "browser_save";

const MAX_SAVE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct BrowserSaveInput {
    #[schemars(
        description = "Optional URL to save. Defaults to the current tab URL. https is allowed; \
            http only for localhost / RFC1918 / *.localhost; file: only under the session folder."
    )]
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Default)]
pub struct BrowserSaveTool;

impl crate::types::tool_metadata::ToolMetadata for BrowserSaveTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Save the currently viewed page (or an explicit URL) into the session-scoped Agent WebView downloads folder. Use this when a PDF/guide opened inline and there is no snapshot uid for Save. Returns the brokered file path. Does not open or execute the file."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for BrowserSaveTool {
    type Args = BrowserSaveInput;
    type Output = ToolOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(BROWSER_SAVE_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            BROWSER_SAVE_TOOL_NAME,
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

    #[tracing::instrument(name = "tool.browser_save", skip_all)]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: BrowserSaveInput,
    ) -> Result<ToolOutput, xai_tool_runtime::ToolError> {
        let handle = super::require_handle(&ctx).await?;
        let url = match input.url.filter(|u| !u.trim().is_empty()) {
            Some(url) => url,
            None => {
                let tabs = handle.tabs().await?;
                let active = tabs.tabs.iter().find(|t| t.active).cloned();
                let first = tabs.tabs.into_iter().next();
                active
                    .or(first)
                    .map(|t| t.url)
                    .filter(|u| !u.is_empty() && u != "about:blank")
                    .ok_or_else(|| {
                        xai_tool_runtime::ToolError::invalid_arguments(
                            "browser_save: no current page URL; pass `url` or navigate first",
                        )
                    })?
            }
        };
        check_url_in_session(&url, handle.session_folder()).map_err(|e| {
            xai_tool_runtime::ToolError::invalid_arguments(e.to_string())
        })?;
        let session_folder = handle.session_folder().ok_or_else(|| {
            xai_tool_runtime::ToolError::invalid_arguments(
                "browser_save: no session folder is configured",
            )
        })?;
        let dest = broker_http_or_file(&url, session_folder).await?;
        let meta = std::fs::metadata(&dest).map_err(|e| {
            xai_tool_runtime::ToolError::custom(
                "browser_error",
                format!("browser_save: cannot stat {}: {e}", dest.display()),
            )
        })?;
        let name = dest
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("download.bin")
            .to_owned();
        let result = DownloadsResult {
            downloads: vec![DownloadInfo {
                name,
                path: dest.to_string_lossy().into_owned(),
                bytes: meta.len(),
                completed: true,
            }],
        };
        Ok(super::json_output(&result))
    }
}

async fn broker_http_or_file(
    url: &str,
    session_folder: &Path,
) -> Result<PathBuf, xai_tool_runtime::ToolError> {
    let downloads = session_folder.join("downloads");
    std::fs::create_dir_all(&downloads).map_err(|e| {
        xai_tool_runtime::ToolError::custom(
            "browser_error",
            format!("browser_save: cannot create {}: {e}", downloads.display()),
        )
    })?;
    let filename = filename_from_url(url);
    let dest = unique_path(&downloads, &filename);
    if let Some(rest) = url.strip_prefix("file:") {
        let src = PathBuf::from(rest.trim_start_matches('/').trim_start_matches('\\'));
        std::fs::copy(&src, &dest).map_err(|e| {
            xai_tool_runtime::ToolError::custom(
                "browser_error",
                format!("browser_save: cannot copy {}: {e}", src.display()),
            )
        })?;
        return Ok(dest);
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| {
            xai_tool_runtime::ToolError::custom("browser_error", format!("browser_save: http client: {e}"))
        })?;
    let response = client.get(url).send().await.map_err(|e| {
        xai_tool_runtime::ToolError::custom(
            "browser_error",
            format!("browser_save: GET {url} failed: {e}"),
        )
    })?;
    if !response.status().is_success() {
        return Err(xai_tool_runtime::ToolError::custom(
            "browser_error",
            format!("browser_save: GET {url} returned HTTP {}", response.status()),
        ));
    }
    if let Some(len) = response.content_length()
        && len > MAX_SAVE_BYTES
    {
        return Err(xai_tool_runtime::ToolError::invalid_arguments(format!(
            "browser_save: remote file is {len} bytes (limit {MAX_SAVE_BYTES})"
        )));
    }
    let bytes = response.bytes().await.map_err(|e| {
        xai_tool_runtime::ToolError::custom(
            "browser_error",
            format!("browser_save: read body failed: {e}"),
        )
    })?;
    if bytes.len() as u64 > MAX_SAVE_BYTES {
        return Err(xai_tool_runtime::ToolError::invalid_arguments(format!(
            "browser_save: body is {} bytes (limit {MAX_SAVE_BYTES})",
            bytes.len()
        )));
    }
    std::fs::write(&dest, &bytes).map_err(|e| {
        xai_tool_runtime::ToolError::custom(
            "browser_error",
            format!("browser_save: cannot write {}: {e}", dest.display()),
        )
    })?;
    Ok(dest)
}

fn filename_from_url(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    let name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("download.bin")
        .trim();
    let safe = name
        .chars()
        .filter(|ch| !ch.is_control() && !matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .take(180)
        .collect::<String>();
    if safe.is_empty() || safe == "." || safe == ".." {
        "download.bin".into()
    } else {
        safe
    }
}

fn unique_path(folder: &Path, filename: &str) -> PathBuf {
    let candidate = folder.join(filename);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(filename);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("download");
    let extension = path.extension().and_then(|s| s.to_str());
    for index in 1..=10_000u32 {
        let name = match extension {
            Some(ext) => format!("{stem} ({index}).{ext}"),
            None => format!("{stem} ({index})"),
        };
        let candidate = folder.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    folder.join("download.bin")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_from_pdf_url() {
        assert_eq!(
            filename_from_url("https://lists.w3.org/a/wcag-rawgit.pdf?dl=1"),
            "wcag-rawgit.pdf"
        );
    }
}
