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
        check_url_in_session(&url, handle.session_folder())
            .map_err(|e| xai_tool_runtime::ToolError::invalid_arguments(e.to_string()))?;
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

pub(crate) async fn broker_http_or_file(
    url: &str,
    session_folder: &Path,
) -> Result<PathBuf, xai_tool_runtime::ToolError> {
    let downloads = prepare_session_downloads(session_folder)?;
    if let Some(rest) = url.strip_prefix("file:") {
        let filename = filename_from_url(url);
        let dest = unique_path(&downloads, &filename);
        let src = PathBuf::from(rest.trim_start_matches('/').trim_start_matches('\\'));
        let meta = std::fs::metadata(&src).map_err(|e| {
            xai_tool_runtime::ToolError::custom(
                "browser_error",
                format!("browser_save: cannot stat {}: {e}", src.display()),
            )
        })?;
        if meta.len() > MAX_SAVE_BYTES {
            return Err(xai_tool_runtime::ToolError::invalid_arguments(format!(
                "browser_save: file is {} bytes (limit {MAX_SAVE_BYTES})",
                meta.len()
            )));
        }
        std::fs::copy(&src, &dest).map_err(|e| {
            xai_tool_runtime::ToolError::custom(
                "browser_error",
                format!("browser_save: cannot copy {}: {e}", src.display()),
            )
        })?;
        return Ok(dest);
    }
    let folder = session_folder.to_path_buf();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            let next = attempt.url().as_str();
            match redirect_hop_action(next, attempt.previous().len() + 1, Some(folder.as_path())) {
                Ok(()) => attempt.follow(),
                Err(err) => attempt.error(err),
            }
        }))
        .build()
        .map_err(|e| {
            xai_tool_runtime::ToolError::custom(
                "browser_error",
                format!("browser_save: http client: {e}"),
            )
        })?;
    let response = client.get(url).send().await.map_err(|e| {
        xai_tool_runtime::ToolError::custom(
            "browser_error",
            format!("browser_save: GET {url} failed: {e}"),
        )
    })?;
    let final_url = response.url().as_str().to_owned();
    check_url_in_session(&final_url, Some(session_folder)).map_err(|e| {
        xai_tool_runtime::ToolError::invalid_arguments(format!(
            "browser_save: final URL after redirects is not allowed: {e}"
        ))
    })?;
    if !response.status().is_success() {
        return Err(xai_tool_runtime::ToolError::custom(
            "browser_error",
            format!(
                "browser_save: GET {url} returned HTTP {}",
                response.status()
            ),
        ));
    }
    if let Some(len) = response.content_length()
        && len > MAX_SAVE_BYTES
    {
        return Err(xai_tool_runtime::ToolError::invalid_arguments(format!(
            "browser_save: remote file is {len} bytes (limit {MAX_SAVE_BYTES})"
        )));
    }
    let content_type = header_str(response.headers(), reqwest::header::CONTENT_TYPE);
    let content_disposition = header_str(response.headers(), reqwest::header::CONTENT_DISPOSITION);
    let filename = filename_from_headers(
        &final_url,
        content_type.as_deref(),
        content_disposition.as_deref(),
    );
    let dest = unique_path(&downloads, &filename);
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = futures::StreamExt::next(&mut stream).await {
        let chunk = chunk.map_err(|e| {
            xai_tool_runtime::ToolError::custom(
                "browser_error",
                format!("browser_save: read body failed: {e}"),
            )
        })?;
        if bytes.len() as u64 + chunk.len() as u64 > MAX_SAVE_BYTES {
            return Err(xai_tool_runtime::ToolError::invalid_arguments(format!(
                "browser_save: body exceeds {MAX_SAVE_BYTES} bytes"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    std::fs::write(&dest, &bytes).map_err(|e| {
        xai_tool_runtime::ToolError::custom(
            "browser_error",
            format!("browser_save: cannot write {}: {e}", dest.display()),
        )
    })?;
    Ok(dest)
}

const MAX_REDIRECTS: usize = 5;

/// Re-check URL policy on each redirect hop (https / local http / session file).
pub(crate) fn redirect_hop_action(
    next_url: &str,
    hop_count: usize,
    session_folder: Option<&Path>,
) -> Result<(), String> {
    if hop_count > MAX_REDIRECTS {
        return Err("too many redirects (limit 5)".into());
    }
    check_url_in_session(next_url, session_folder).map_err(|e| format!("redirect blocked: {e}"))
}

fn header_str(
    headers: &reqwest::header::HeaderMap,
    name: reqwest::header::HeaderName,
) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// True when the URL path looks like a direct binary/PDF/zip download.
pub(crate) fn url_looks_like_direct_download(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "pdf"
            | "zip"
            | "7z"
            | "rar"
            | "gz"
            | "tgz"
            | "tar"
            | "bz2"
            | "xz"
            | "exe"
            | "msi"
            | "dmg"
            | "pkg"
            | "iso"
            | "bin"
            | "apk"
            | "vsix"
            | "wasm"
            | "docx"
            | "xlsx"
            | "pptx"
            | "odt"
            | "ods"
            | "odp"
    )
}

pub(crate) fn filename_from_headers(
    url: &str,
    content_type: Option<&str>,
    content_disposition: Option<&str>,
) -> String {
    if let Some(name) = content_disposition.and_then(filename_from_content_disposition) {
        let safe = sanitize_filename(&name);
        if safe != "download.bin" {
            return safe;
        }
    }
    let from_url = filename_from_url(url);
    if filename_has_extension(&from_url) && from_url != "download.bin" {
        return from_url;
    }
    let default = default_filename_for_content_type(content_type);
    if from_url != "download.bin" && !from_url.is_empty() {
        if let Some(ext) = Path::new(&default).extension().and_then(|e| e.to_str())
            && !filename_has_extension(&from_url)
        {
            return sanitize_filename(&format!("{from_url}.{ext}"));
        }
        return from_url;
    }
    default
}

fn filename_from_content_disposition(header: &str) -> Option<String> {
    let lower = header.to_ascii_lowercase();
    if let Some(idx) = lower.find("filename*") {
        let rest = header[idx + "filename*".len()..].trim_start();
        let rest = rest.trim_start_matches('=').trim();
        let rest = rest.trim_matches('"');
        if let Some(encoded) = rest.split('\'').nth(2) {
            let decoded = percent_decode_filename(encoded);
            if !decoded.is_empty() {
                return Some(decoded);
            }
        }
    }
    let idx = lower.find("filename")?;
    let rest = header[idx + "filename".len()..].trim_start();
    let rest = rest.trim_start_matches('=').trim();
    let rest = rest
        .split(';')
        .next()
        .unwrap_or(rest)
        .trim()
        .trim_matches('"')
        .trim_matches('\'');
    if rest.is_empty() {
        None
    } else {
        Some(rest.to_owned())
    }
}

fn percent_decode_filename(input: &str) -> String {
    let mut bytes = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '%' && i + 2 < chars.len() {
            let hex: String = chars[i + 1..i + 3].iter().collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                bytes.push(byte);
                i += 3;
                continue;
            }
        }
        let mut buf = [0u8; 4];
        let encoded = chars[i].encode_utf8(&mut buf);
        bytes.extend_from_slice(encoded.as_bytes());
        i += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn default_filename_for_content_type(content_type: Option<&str>) -> String {
    let mime = content_type
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match mime.as_str() {
        "text/html" | "application/xhtml+xml" => "page.html".into(),
        "text/plain" => "download.txt".into(),
        "application/pdf" => "download.pdf".into(),
        "application/zip" | "application/x-zip-compressed" => "download.zip".into(),
        "application/json" => "download.json".into(),
        "application/gzip" | "application/x-gzip" => "download.gz".into(),
        "image/png" => "download.png".into(),
        "image/jpeg" => "download.jpg".into(),
        "image/gif" => "download.gif".into(),
        "image/webp" => "download.webp".into(),
        "application/octet-stream" => "download.bin".into(),
        _ => "download.bin".into(),
    }
}

fn filename_has_extension(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| !e.is_empty())
}

fn prepare_session_downloads(session_folder: &Path) -> Result<PathBuf, xai_tool_runtime::ToolError> {
    let downloads = session_folder.join("downloads");
    std::fs::create_dir_all(&downloads).map_err(|e| {
        xai_tool_runtime::ToolError::custom(
            "browser_error",
            format!("browser_save: cannot create {}: {e}", downloads.display()),
        )
    })?;
    let meta = std::fs::symlink_metadata(&downloads).map_err(|e| {
        xai_tool_runtime::ToolError::custom(
            "browser_error",
            format!("browser_save: cannot inspect {}: {e}", downloads.display()),
        )
    })?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(xai_tool_runtime::ToolError::custom(
            "browser_error",
            "browser_save: downloads folder is not a real directory",
        ));
    }
    let session_root = dunce::canonicalize(session_folder).map_err(|e| {
        xai_tool_runtime::ToolError::custom(
            "browser_error",
            format!("browser_save: cannot canonicalize session folder: {e}"),
        )
    })?;
    let folder = dunce::canonicalize(&downloads).map_err(|e| {
        xai_tool_runtime::ToolError::custom(
            "browser_error",
            format!("browser_save: cannot canonicalize downloads: {e}"),
        )
    })?;
    if !folder.starts_with(&session_root) {
        return Err(xai_tool_runtime::ToolError::custom(
            "browser_error",
            "browser_save: downloads folder escapes the session folder",
        ));
    }
    Ok(folder)
}

fn filename_from_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("download.bin")
        .trim();
    sanitize_filename(name)
}

fn sanitize_filename(name: &str) -> String {
    let safe = name
        .chars()
        .filter(|ch| {
            !ch.is_control() && !matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        .take(180)
        .collect::<String>();
    let trimmed = safe.trim_end_matches([' ', '.']);
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return "download.bin".into();
    }
    let stem = trimmed.split('.').next().unwrap_or_default();
    let upper = stem.to_ascii_uppercase();
    let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.as_bytes()[3].is_ascii_digit());
    if reserved {
        "download.bin".into()
    } else {
        trimmed.to_string()
    }
}

fn unique_path(folder: &Path, filename: &str) -> PathBuf {
    let candidate = folder.join(filename);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("download");
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

    #[test]
    fn filename_from_html_content_type_is_not_bin() {
        assert_eq!(
            filename_from_headers(
                "https://example.com/",
                Some("text/html; charset=utf-8"),
                None
            ),
            "page.html"
        );
        assert_eq!(
            filename_from_headers("https://example.com/guide", Some("text/html"), None),
            "guide.html"
        );
        assert_eq!(
            filename_from_headers(
                "https://example.com/file.zip",
                Some("application/zip"),
                None
            ),
            "file.zip"
        );
        assert_eq!(
            filename_from_headers(
                "https://example.com/dl",
                Some("application/octet-stream"),
                Some(r#"attachment; filename="report.pdf""#)
            ),
            "report.pdf"
        );
    }

    #[test]
    fn redirect_hop_rechecks_url_policy() {
        assert!(redirect_hop_action("https://example.com/next", 1, None).is_ok());
        assert!(redirect_hop_action("http://127.0.0.1/local", 1, None).is_ok());
        let err = redirect_hop_action("http://example.com/secret", 1, None).unwrap_err();
        assert!(
            err.contains("redirect blocked") || err.contains("http"),
            "{err}"
        );
        let err = redirect_hop_action("https://example.com/x", 6, None).unwrap_err();
        assert!(err.contains("too many"), "{err}");
        let err = redirect_hop_action("file:///C:/Windows/notepad.exe", 1, None).unwrap_err();
        assert!(err.contains("file:"), "{err}");
    }

    #[test]
    fn zip_and_pdf_urls_look_like_downloads() {
        assert!(url_looks_like_direct_download("https://example.com/a.zip"));
        assert!(url_looks_like_direct_download(
            "https://lists.w3.org/a/wcag-rawgit.pdf?dl=1"
        ));
        assert!(!url_looks_like_direct_download("https://example.com/jobs"));
        assert!(!url_looks_like_direct_download(
            "https://example.com/index.html"
        ));
    }

    #[tokio::test]
    async fn redirect_to_public_http_is_blocked() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/start"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("Location", "http://example.com/secret"),
            )
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let err = broker_http_or_file(&format!("{}/start", server.uri()), tmp.path())
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("redirect") || msg.contains("blocked") || msg.contains("http"),
            "{msg}"
        );
    }

    #[tokio::test]
    async fn html_body_is_saved_as_page_html() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw("<html>hi</html>", "text/html; charset=utf-8"),
            )
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let dest = broker_http_or_file(&server.uri(), tmp.path())
            .await
            .unwrap();
        assert_eq!(dest.file_name().and_then(|n| n.to_str()), Some("page.html"));
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "<html>hi</html>");
    }
}
