//! `WebFetchClient` - shared HTTP client with cache, HTML-to-markdown
//! conversion, URL validation, and SSRF protection.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::StreamExt;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, USER_AGENT};
use url::Url;

use super::cache::FetchCache;
use super::config::{ACCEPT_HEADER, MAX_REDIRECTS, MAX_URL_LENGTH, WebFetchParams};
use super::domain::DomainMatcher;
use super::error::WebFetchError;
use super::extract::{self, ExtractMode};
use super::http::HttpClient;
use super::overflow::{OverflowHandler, RecoveryTools, inline_budget};
use super::ssrf;
use crate::implementations::grok_build::storage::SessionFileWriter;
use crate::types::output::{WebFetchContent, WebFetchOutput, WebFetchSourceArtifact};

const DEFAULT_DOWNLOAD_DIR: &str = "downloads";
/// Max characters of error-page body included in non-2xx messages.
const ERROR_BODY_PREVIEW_CHARS: usize = 500;
const DEFAULT_JS_SOFT_NOTE: &str =
    "\n\n[web_fetch note: large JavaScript response truncated by max_js_bytes]";

/// Per-call options for [`WebFetchClient::fetch`].
#[derive(Debug, Clone, Default)]
pub struct FetchOptions<'a> {
    pub extract_mode: Option<&'a str>,
    pub start_offset: Option<usize>,
    pub max_chars: Option<usize>,
    pub include_links: bool,
}

/// Shared HTTP client and cache for web fetching.
#[derive(Clone)]
pub struct WebFetchClient {
    http: HttpClient,
    cache: Arc<parking_lot::RwLock<FetchCache>>,
    converter: Arc<htmd::HtmlToMarkdown>,
    params: WebFetchParams,
    /// `None` = open-public (any host that passes SSRF). `Some` = enforce list.
    domain_matcher: Option<DomainMatcher>,
    download_writer: SessionFileWriter,
    image_writer: SessionFileWriter,
    video_writer: SessionFileWriter,
    overflow: OverflowHandler,
}

struct ProcessedText {
    content: String,
    content_type: String,
    bytes: usize,
    was_truncated: bool,
    artifact_path: Option<PathBuf>,
    inline_fallback: Option<String>,
}

impl WebFetchClient {
    pub fn new(params: &WebFetchParams) -> Result<Self, WebFetchError> {
        let converter = Arc::new(
            htmd::HtmlToMarkdown::builder()
                .skip_tags(vec![
                    "script", "style", "noscript", "svg", "iframe", "object", "embed",
                ])
                .build(),
        );

        let domain_matcher = params.domain_allowlist().map(DomainMatcher::new);

        Ok(Self {
            // Reqwest client can fail to build.
            http: HttpClient::new(params)?,
            cache: Arc::new(parking_lot::RwLock::new(FetchCache::new(
                params.cache_ttl_secs(),
                params.max_cache_entries(),
            ))),
            converter,
            params: params.clone(),
            domain_matcher,
            download_writer: SessionFileWriter::new(DEFAULT_DOWNLOAD_DIR, "pdf"),
            image_writer: SessionFileWriter::new("images", "jpg"),
            video_writer: SessionFileWriter::new("videos", "mp4"),
            overflow: OverflowHandler::new(),
        })
    }

    /// Fetch a URL and return its content as markdown.
    ///
    /// Handles: domain allowlist, validation, HTTPS upgrade, SSRF check, HTTP
    /// fetch with same-host redirects, HTML-to-markdown conversion, truncation,
    /// and caching. On transport errors, the HTTP client is invalidated so
    /// the next call gets a fresh connection pool (see [`HttpClient`]).
    pub async fn fetch(
        &self,
        raw_url: &str,
        session_folder: Option<&Path>,
        read_tool_name: Option<&str>,
        execute_tool_name: Option<&str>,
        options: FetchOptions<'_>,
    ) -> Result<WebFetchOutput, WebFetchError> {
        let mut url = validate_url(raw_url)?;
        upgrade_to_https(&mut url);

        let url_str = url.to_string();
        let mode = ExtractMode::parse(options.extract_mode);
        if mode == ExtractMode::Headless {
            return Ok(WebFetchOutput::Error {
                url: Some(url_str),
                message: "extract_mode=headless is not supported by static web_fetch \
                    (no JS runtime). Use a browser MCP tool, or fetch a pre-rendered \
                    HTML/docs URL with extract_mode=article|full."
                    .into(),
            });
        }
        // Mode + windowing are part of the cache key so variants do not collide.
        let cache_key = format!(
            "{url_str}\0{}\0{}\0{}\0{}",
            mode.as_str(),
            options.start_offset.unwrap_or(0),
            options.max_chars.unwrap_or(0),
            options.include_links,
        );

        // Check cache (keyed by URL + extract options).
        {
            let mut cache = self.cache.write();
            if let Some(cached) = cache.get(&cache_key) {
                tracing::debug!("web_fetch cache hit for {url_str}");
                return Ok(cached);
            }
        }

        // Domain allowlist before any network I/O.
        if let Some(matcher) = &self.domain_matcher
            && let Some(blocked) = matcher.check(&url)
        {
            return Ok(blocked);
        }

        // Make request and build output (SSRF + DNS pin inside fetch_url).
        let result = match fetch_url(
            &self.http,
            &self.params,
            &url,
            self.params.max_content_length(),
            self.params.allow_local(),
            self.domain_matcher.as_ref(),
        )
        .await
        {
            Ok(result) => result,
            Err(e @ WebFetchError::HttpRequest(_)) => {
                self.http.invalidate();
                return Err(e);
            }
            Err(e) => return Err(e),
        };

        let (body, mut content_type, final_url, status_code) = match result {
            FetchResult::Content {
                body,
                content_type,
                final_url,
                status_code,
            } => (body, content_type, final_url, status_code),
            FetchResult::CrossHostRedirect {
                original_host,
                redirect_url,
            } => {
                return Ok(WebFetchOutput::CrossHostRedirect {
                    original_host,
                    redirect_url,
                });
            }
            FetchResult::DomainNotAllowed(domain) => {
                return Ok(WebFetchOutput::DomainNotAllowed(domain));
            }
        };

        // Infer safer content type when the server omitted or lied about it.
        content_type = refine_content_type(&content_type, &body);

        // 304 and other empty not-modified responses.
        if status_code == 304 {
            return Ok(WebFetchOutput::Error {
                url: Some(final_url),
                message: "HTTP 304 Not Modified (no body). Re-fetch without validators \
                    or use a different URL."
                    .into(),
            });
        }

        // Bot / challenge interstitials often return HTTP 200 with little real content.
        if is_html(&content_type) || content_type.is_empty() {
            let peek = decode_body(&body, &content_type);
            if extract::looks_like_bot_challenge(&peek) {
                return Ok(WebFetchOutput::Error {
                    url: Some(final_url),
                    message: format!(
                        "HTTP {status_code}: page looks like a bot/challenge interstitial \
                         (Cloudflare/WAF), not real content. Try a different URL, official docs, \
                         or a browser MCP if you need a live session."
                    ),
                });
            }
        }

        // Non-2xx: surface as structured error (do not pretend success).
        if !(200..300).contains(&status_code) {
            let preview = error_body_preview(&body, &content_type);
            return Ok(WebFetchOutput::Error {
                url: Some(final_url),
                message: format!(
                    "HTTP {status_code}{}{}",
                    if preview.is_empty() { "" } else { ": " },
                    preview
                ),
            });
        }

        // PDF by content-type or magic bytes.
        if is_pdf(&content_type) || body.starts_with(b"%PDF") {
            if !body.starts_with(b"%PDF") {
                return Err(WebFetchError::ContentTypeMismatch {
                    content_type,
                    url: final_url,
                });
            }
            let media_session_folder = require_media_session_folder(session_folder)?;
            let output = save_pdf(
                &self.download_writer,
                media_session_folder,
                &body,
                final_url,
                if is_pdf(&content_type) {
                    content_type
                } else {
                    "application/pdf".into()
                },
                status_code,
                read_tool_name,
            )
            .await?;
            return Ok(output);
        }

        // Image: validate magic bytes, save to disk.
        if is_image(&content_type) {
            if !validate_media_magic_bytes(&content_type, &body) {
                return Err(WebFetchError::ContentTypeMismatch {
                    content_type,
                    url: final_url,
                });
            }
            let media_session_folder = require_media_session_folder(session_folder)?;
            let output = save_image(
                &self.image_writer,
                media_session_folder,
                &body,
                final_url,
                content_type,
                status_code,
                read_tool_name,
            )
            .await?;
            return Ok(output);
        }

        // Video: validate magic bytes, save to disk.
        if is_video(&content_type) {
            if !validate_media_magic_bytes(&content_type, &body) {
                return Err(WebFetchError::ContentTypeMismatch {
                    content_type,
                    url: final_url,
                });
            }
            let media_session_folder = require_media_session_folder(session_folder)?;
            let output = save_video(
                &self.video_writer,
                media_session_folder,
                &body,
                final_url,
                content_type,
                status_code,
            )
            .await?;
            return Ok(output);
        }

        // Reject binary content types that would produce garbage through lossy UTF-8.
        // SVG is allowed through the text path (scripts stripped during conversion).
        if is_binary_content_type(&content_type) && !is_svg_xml(&content_type) {
            return Err(WebFetchError::UnsupportedContentType {
                content_type,
                url: final_url,
            });
        }

        let processed = self
            .process_text_content(
                &body,
                &content_type,
                session_folder,
                RecoveryTools {
                    read: read_tool_name,
                    execute: execute_tool_name,
                },
                mode,
                &final_url,
                status_code,
                options.start_offset.unwrap_or(0),
                options.max_chars,
                options.include_links,
            )
            .await;
        let was_truncated = processed.was_truncated;

        let output = WebFetchOutput::Content(WebFetchContent {
            url: final_url,
            content: processed.content,
            content_type: processed.content_type,
            status_code,
            bytes: processed.bytes,
            source_artifact: processed
                .artifact_path
                .map(|path| WebFetchSourceArtifact { path }),
            inline_fallback: processed.inline_fallback,
            output_location: None,
        });

        // Insert into cache.
        {
            let mut cache = self.cache.write();
            cache.insert_text(cache_key, output.clone(), was_truncated);
        }

        Ok(output)
    }

    async fn process_text_content(
        &self,
        body: &[u8],
        content_type: &str,
        session_folder: Option<&Path>,
        tools: RecoveryTools<'_>,
        mode: ExtractMode,
        final_url: &str,
        status_code: u16,
        start_offset: usize,
        max_chars: Option<usize>,
        include_links: bool,
    ) -> ProcessedText {
        let raw_content = decode_body(body, content_type);
        let (mut body_text, meta_mode, prepared_html) = if is_html(content_type) {
            let (prepared, used, spa) = extract::prepare_html(&raw_content, mode);
            let md = self
                .converter
                .convert(&prepared)
                .unwrap_or_else(|_| prepared.clone());
            let mut md = strip_base64_data_uris(md);
            if spa || extract::markdown_looks_empty(&md) {
                md.push_str(
                    "\n\n[web_fetch note: little extractable text / possible SPA shell. \
                     Use browser MCP for JS-rendered pages.]",
                );
            }
            (md, used.as_str(), Some(prepared))
        } else if is_svg_xml(content_type) {
            // SVG can embed scripts; treat like HTML for conversion/stripping.
            let cleaned = extract::clean_chrome(&raw_content);
            let md = self
                .converter
                .convert(&cleaned)
                .unwrap_or_else(|_| "[svg content]".to_string());
            (strip_base64_data_uris(md), "svg", None)
        } else {
            let mut text = strip_base64_data_uris(raw_content);
            let max_js = self.params.max_js_bytes();
            if is_javascript(content_type) && text.len() > max_js {
                // Truncate by UTF-8 bytes (not chars) to honor max_js_bytes.
                let mut end = max_js.min(text.len());
                while end > 0 && !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
                text.push_str(DEFAULT_JS_SOFT_NOTE);
            }
            (text, "text", None)
        };
        // Window body first so links never displace main content under max_chars.
        body_text = apply_text_window(&body_text, start_offset, max_chars);
        if include_links {
            if let Some(ref prepared) = prepared_html {
                body_text.push_str(&extract::extract_link_summary_budgeted(
                    prepared,
                    final_url,
                    extract::LINK_SUMMARY_MAX_COUNT,
                    extract::LINK_SUMMARY_MAX_BYTES,
                ));
            }
        }

        // Reserve room for the status header inside the overflow budget so
        // long URLs cannot push output past the documented cap.
        // Keep status header short enough to leave room for body under the cap.
        let mut header =
            format!("Status: {status_code} | extract={meta_mode} | url={final_url}\n\n");
        let hard_cap = self.params.max_markdown_length().max(64);
        if header.len() > hard_cap / 2 {
            let room = (hard_cap / 2).saturating_sub(40);
            let short_url: String = final_url.chars().take(room).collect();
            header = format!("Status: {status_code} | extract={meta_mode} | url={short_url}…\n\n");
        }
        let mut budget = inline_budget(
            self.params.context_window_tokens(),
            self.params.max_markdown_length(),
        );
        budget.reserve_prefix(header.len());

        let bytes = body_text.len();
        let output_content_type = if is_html(content_type) || is_svg_xml(content_type) {
            "markdown".to_string()
        } else {
            content_type.to_owned()
        };
        let overflow = self
            .overflow
            .process(
                body_text,
                budget,
                session_folder,
                &output_content_type,
                tools,
            )
            .await;
        let content = format!("{header}{}", overflow.content);
        let inline_fallback = overflow
            .path_free_content
            .map(|body| format!("{header}{body}"));
        ProcessedText {
            content,
            content_type: output_content_type,
            bytes,
            was_truncated: overflow.was_truncated,
            artifact_path: overflow.artifact_path,
            inline_fallback,
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// URL Validation
// ───────────────────────────────────────────────────────────────────────────

/// Validates URL scheme, length, credentials, and hostname labels.
fn validate_url(raw: &str) -> Result<Url, WebFetchError> {
    if raw.len() > MAX_URL_LENGTH {
        return Err(WebFetchError::UrlTooLong {
            max: MAX_URL_LENGTH,
        });
    }

    let parsed = Url::parse(raw)?; // uses #[from] url::ParseError

    match parsed.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(WebFetchError::UnsupportedScheme {
                scheme: scheme.to_string(),
            });
        }
    }

    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(WebFetchError::CredentialsInUrl);
    }

    if let Some(host) = parsed.host_str() {
        // IP literals (v4/v6) are not DNS labels — skip the multi-label rule.
        // SSRF still applies to the address later.
        let is_ip = host
            .trim_matches(|c| c == '[' || c == ']')
            .parse::<std::net::IpAddr>()
            .is_ok();
        if !is_ip
            && host.split('.').count() < 2
            // `localhost` is a single-label name; SSRF still requires
            // allow_local for explicit local hosts.
            && !ssrf::is_explicit_local_host(host)
        {
            return Err(WebFetchError::SingleLabelHost {
                host: host.to_string(),
            });
        }
    }

    Ok(parsed)
}

/// Upgrade `http://` to `https://`, except for explicit loopback hosts.
///
/// Local dev servers almost always speak plain HTTP; forcing TLS would break
/// `http://127.0.0.1` / `http://localhost` when local binding is opted in.
fn upgrade_to_https(url: &mut Url) {
    if url.scheme() != "http" {
        return;
    }
    if let Some(host) = url.host_str()
        && ssrf::is_explicit_local_host(host)
    {
        return;
    }
    let _ = url.set_scheme("https");
}

// ───────────────────────────────────────────────────────────────────────────
// HTTP Fetching
// ───────────────────────────────────────────────────────────────────────────

enum FetchResult {
    Content {
        body: Vec<u8>,
        content_type: String,
        final_url: String,
        status_code: u16,
    },
    CrossHostRedirect {
        original_host: String,
        redirect_url: String,
    },
    DomainNotAllowed(String),
}

/// Fetch a URL with manual same-host redirect handling.
///
/// Re-runs SSRF + DNS resolution on every hop and pins resolved addresses on
/// the client (when not using an egress proxy) so rebinding between check and
/// connect cannot open private IPs. Streams the body and aborts once
/// `max_content_length` is exceeded.
async fn fetch_url(
    shared_http: &HttpClient,
    params: &WebFetchParams,
    url: &Url,
    max_content_length: usize,
    allow_local: bool,
    domain_matcher: Option<&DomainMatcher>,
) -> Result<FetchResult, WebFetchError> {
    let mut current_url = url.clone();
    let mut hops = 0;
    let use_proxy = params.proxy_endpoint.is_some();

    // Loop to follow redirects under the same host.
    loop {
        // Re-check on every hop (including the first) so a rebinding name that
        // was public at the pre-fetch check cannot become loopback/private here.
        if let Some(matcher) = domain_matcher
            && let Some(WebFetchOutput::DomainNotAllowed(domain)) = matcher.check(&current_url)
        {
            return Ok(FetchResult::DomainNotAllowed(domain));
        }

        // Direct egress: resolve + SSRF + DNS pin. Proxy egress: skip local DNS
        // so split-horizon / proxy-only names still work (proxy is the resolver).
        let client = if use_proxy {
            shared_http.get_or_rebuild()?
        } else {
            let addrs = ssrf::resolve_and_check_ssrf(&current_url, allow_local).await?;
            let host = current_url.host_str().unwrap_or("");
            Arc::new(HttpClient::build_with_dns_pin(params, host, &addrs)?)
        };

        let resp = client
            .get(current_url.as_str())
            .header(USER_AGENT, params.user_agent())
            .header(ACCEPT, ACCEPT_HEADER)
            .header(ACCEPT_LANGUAGE, params.accept_language())
            // Browser-like document navigation headers (compatibility, not stealth).
            .header("Upgrade-Insecure-Requests", "1")
            .header("Sec-Fetch-Dest", "document")
            .header("Sec-Fetch-Mode", "navigate")
            .header("Sec-Fetch-Site", "none")
            .header("Sec-Fetch-User", "?1")
            .send()
            .await?;

        let status = resp.status();

        // 304 is not a redirect; treat as a normal (empty) response path.
        if status.is_redirection() && status.as_u16() != 304 {
            hops += 1;
            if hops > MAX_REDIRECTS {
                return Err(WebFetchError::TooManyRedirects { max: MAX_REDIRECTS });
            }

            // 3xx without Location is not a usable redirect.
            let Some(location) = resp.headers().get("location") else {
                return Err(WebFetchError::InvalidRedirect(
                    "redirect response missing Location header".into(),
                ));
            };
            let location_str = location.to_str().unwrap_or("");
            if location_str.is_empty() {
                return Err(WebFetchError::InvalidRedirect(
                    "redirect Location header is empty".into(),
                ));
            }
            let mut next_url = current_url
                .join(location_str)
                .map_err(|e| WebFetchError::InvalidRedirect(format!("{e}")))?;
            // Re-apply URL policy on redirect targets (credentials, scheme, host form).
            next_url = validate_url(next_url.as_str()).map_err(|e| {
                WebFetchError::InvalidRedirect(format!("redirect target rejected: {e}"))
            })?;
            // Safe auto-follow: exact same host, or apex ↔ www (and trailing-dot)
            // after full re-validation / allowlist / SSRF on the next loop.
            if is_same_host(&current_url, &next_url)
                || is_www_apex_equivalent(&current_url, &next_url)
            {
                // Re-apply https upgrade on every hop: Location may be
                // absolute `http://…` and would otherwise silently
                // downgrade an https fetch. Local hosts still skip TLS.
                upgrade_to_https(&mut next_url);
                // check_ssrf / allowlist run at the top of the next loop iteration.
                current_url = next_url;
                continue;
            }
            return Ok(FetchResult::CrossHostRedirect {
                original_host: current_url.host_str().unwrap_or("unknown").to_string(),
                redirect_url: next_url.to_string(),
            });
        }

        let content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let final_url = resp.url().to_string();
        let status_code = status.as_u16();

        // Early reject when Content-Length already exceeds the budget.
        if let Some(len) = resp.content_length()
            && len as usize > max_content_length
        {
            return Err(WebFetchError::ResponseTooLarge {
                max: max_content_length,
            });
        }

        let body = read_body_bounded(resp, max_content_length).await?;

        return Ok(FetchResult::Content {
            body,
            content_type,
            final_url,
            status_code,
        });
    }
}

/// Stream response body, aborting once `max_content_length` is exceeded.
async fn read_body_bounded(
    resp: reqwest::Response,
    max_content_length: usize,
) -> Result<Vec<u8>, WebFetchError> {
    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let next_len =
            body.len()
                .checked_add(chunk.len())
                .ok_or(WebFetchError::ResponseTooLarge {
                    max: max_content_length,
                })?;
        if next_len > max_content_length {
            return Err(WebFetchError::ResponseTooLarge {
                max: max_content_length,
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Exact host equality (raw host_str, including brackets for IPv6).
fn is_same_host(a: &Url, b: &Url) -> bool {
    a.host_str() == b.host_str()
}

/// True when hosts differ only by a single `www.` label and/or a trailing dot.
/// Used to auto-follow the common apex ↔ www CDN canonicalization without
/// following arbitrary cross-host redirects.
fn is_www_apex_equivalent(a: &Url, b: &Url) -> bool {
    let Some(ha) = a.host_str() else {
        return false;
    };
    let Some(hb) = b.host_str() else {
        return false;
    };
    // Never equate IP literals via www logic.
    if ha.parse::<std::net::IpAddr>().is_ok()
        || hb.parse::<std::net::IpAddr>().is_ok()
        || ha
            .trim_matches(|c| c == '[' || c == ']')
            .parse::<std::net::IpAddr>()
            .is_ok()
        || hb
            .trim_matches(|c| c == '[' || c == ']')
            .parse::<std::net::IpAddr>()
            .is_ok()
    {
        return false;
    }
    let na = normalize_host_for_www(ha);
    let nb = normalize_host_for_www(hb);
    na == nb && !na.is_empty()
}

fn normalize_host_for_www(host: &str) -> String {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    h.strip_prefix("www.").unwrap_or(&h).to_string()
}

// ───────────────────────────────────────────────────────────────────────────
// Content Processing
// ───────────────────────────────────────────────────────────────────────────

fn require_media_session_folder(session_folder: Option<&Path>) -> Result<&Path, WebFetchError> {
    session_folder.ok_or_else(|| {
        WebFetchError::IoError(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "session folder is unavailable",
        ))
    })
}

fn is_html(content_type: &str) -> bool {
    content_type.contains("text/html") || content_type.contains("application/xhtml")
}

fn is_javascript(content_type: &str) -> bool {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    matches!(
        mime.as_str(),
        "application/javascript"
            | "application/ecmascript"
            | "application/x-javascript"
            | "text/javascript"
            | "text/ecmascript"
    )
}

/// When Content-Type is missing or generic, sniff body for PDF/HTML/binary.
fn refine_content_type(content_type: &str, body: &[u8]) -> String {
    let trimmed = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim();
    if !trimmed.is_empty()
        && trimmed != "application/octet-stream"
        && trimmed != "binary/octet-stream"
    {
        return content_type.to_string();
    }
    if body.starts_with(b"%PDF") {
        return "application/pdf".into();
    }
    let sample = &body[..body.len().min(512)];
    let textish = sample
        .iter()
        .filter(|b| b.is_ascii_graphic() || b.is_ascii_whitespace())
        .count();
    if !sample.is_empty() && textish * 100 / sample.len() < 85 {
        return "application/octet-stream".into();
    }
    let head = String::from_utf8_lossy(sample).to_ascii_lowercase();
    if head.contains("<html") || head.contains("<!doctype html") || head.contains("<head") {
        return "text/html; charset=utf-8".into();
    }
    if trimmed.is_empty() {
        "text/plain; charset=utf-8".into()
    } else {
        content_type.to_string()
    }
}

fn apply_text_window(text: &str, start_offset: usize, max_chars: Option<usize>) -> String {
    let total = text.chars().count();
    let start = start_offset.min(total);
    let sliced: String = text.chars().skip(start).collect();
    let Some(max) = max_chars.filter(|m| *m > 0) else {
        return if start == 0 {
            sliced
        } else {
            format!("{sliced}\n\n[web_fetch window: offset={start} of {total} chars]")
        };
    };
    let taken: String = sliced.chars().take(max).collect();
    let remaining = sliced.chars().count().saturating_sub(max);
    if remaining == 0 && start == 0 {
        taken
    } else {
        format!(
            "{taken}\n\n[web_fetch window: showing {shown} chars from offset {start} \
             ({remaining} chars after window; total {total})]",
            shown = taken.chars().count(),
        )
    }
}

fn is_svg_xml(content_type: &str) -> bool {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    mime == "image/svg+xml" || mime == "application/svg+xml"
}

/// Decode body using Content-Type charset, then HTML meta charset, else UTF-8.
fn decode_body(body: &[u8], content_type: &str) -> String {
    if let Some(charset) = parse_charset(content_type)
        && let Some(enc) = encoding_rs::Encoding::for_label(charset.as_bytes())
    {
        let (cow, _, _) = enc.decode(body);
        return cow.into_owned();
    }
    // UTF-8 BOM
    if body.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&body[3..]).into_owned();
    }
    // HTML <meta charset="…"> / http-equiv content-type when header lacked charset.
    if is_html(content_type) || content_type.is_empty() {
        if let Some(charset) = sniff_html_meta_charset(body)
            && let Some(enc) = encoding_rs::Encoding::for_label(charset.as_bytes())
        {
            let (cow, _, _) = enc.decode(body);
            return cow.into_owned();
        }
    }
    String::from_utf8_lossy(body).into_owned()
}

/// Cheap scan of the first ~4 KiB for HTML meta charset declarations.
fn sniff_html_meta_charset(body: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&body[..body.len().min(4096)]).to_ascii_lowercase();
    // <meta charset="utf-8">
    if let Some(i) = head.find("charset=") {
        let rest = &head[i + "charset=".len()..];
        let rest = rest.trim_start_matches(['"', '\'', ' ']);
        let end = rest
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
            .unwrap_or(rest.len());
        let label = rest[..end].trim();
        if !label.is_empty() {
            return Some(label.to_string());
        }
    }
    None
}

fn parse_charset(content_type: &str) -> Option<String> {
    for part in content_type.split(';').skip(1) {
        let part = part.trim();
        let Some(eq) = part.find('=') else {
            continue;
        };
        let key = part[..eq].trim();
        if !key.eq_ignore_ascii_case("charset") {
            continue;
        }
        let val = part[eq + 1..].trim().trim_matches('"').trim_matches('\'');
        if !val.is_empty() {
            return Some(val.to_string());
        }
    }
    None
}

fn error_body_preview(body: &[u8], content_type: &str) -> String {
    let text = decode_body(body, content_type);
    let text = if is_html(content_type) {
        // Prefer a little cleaned text over raw tags in error messages.
        let (prepared, _, _) = extract::prepare_html(&text, ExtractMode::Full);
        self_converter().convert(&prepared).unwrap_or(text)
    } else {
        text
    };
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= ERROR_BODY_PREVIEW_CHARS {
        collapsed
    } else {
        let truncated: String = collapsed.chars().take(ERROR_BODY_PREVIEW_CHARS).collect();
        format!("{truncated}…")
    }
}

fn self_converter() -> htmd::HtmlToMarkdown {
    htmd::HtmlToMarkdown::builder()
        .skip_tags(vec![
            "script", "style", "noscript", "svg", "iframe", "object", "embed",
        ])
        .build()
}

fn is_pdf(content_type: &str) -> bool {
    content_type
        .to_ascii_lowercase()
        .contains("application/pdf")
}

/// Returns `true` for image content types, excluding SVG (which can contain
/// `<script>` tags and event handlers — an XSS vector if saved and opened).
fn is_image(content_type: &str) -> bool {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    mime.starts_with("image/") && mime != "image/svg+xml"
}

fn is_video(content_type: &str) -> bool {
    content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase()
        .starts_with("video/")
}

/// Validate that the first bytes of `body` match the magic bytes expected for
/// the claimed `content_type`. Returns `true` when the bytes match or when the
/// subtype is unknown (fail-open for niche formats).
fn validate_media_magic_bytes(content_type: &str, body: &[u8]) -> bool {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    match mime.as_str() {
        "image/png" => body.starts_with(&[0x89, 0x50, 0x4E, 0x47]),
        "image/jpeg" => body.starts_with(&[0xFF, 0xD8, 0xFF]),
        "image/gif" => body.starts_with(b"GIF8"),
        "image/webp" => body.len() >= 12 && &body[..4] == b"RIFF" && &body[8..12] == b"WEBP",
        "video/mp4" => body.len() >= 8 && &body[4..8] == b"ftyp",
        "video/webm" => body.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]),
        _ => true, // unknown subtypes: allow (fail-open for niche formats)
    }
}

/// Map a media Content-Type to the correct file extension.
fn media_extension(content_type: &str) -> &'static str {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    match mime.as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        "video/x-msvideo" => "avi",
        _ => "bin",
    }
}

/// Returns `true` for content types that are binary and would produce garbage
/// through `String::from_utf8_lossy`. Text-like types (`text/*`,
/// `application/json`, `application/xml`, `application/javascript`, etc.)
/// return `false`.
fn is_binary_content_type(content_type: &str) -> bool {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();

    if mime.starts_with("text/") {
        return false;
    }
    !matches!(
        mime.as_str(),
        "application/json"
            | "application/xml"
            | "application/javascript"
            | "application/ecmascript"
            | "application/x-javascript"
            | "application/xhtml+xml"
            | "application/rss+xml"
            | "application/atom+xml"
            | "application/soap+xml"
            | "application/xslt+xml"
            | "application/mathml+xml"
            // SVG handled on a dedicated path (can embed scripts).
            | "application/x-www-form-urlencoded"
            | "application/graphql"
            | "application/ld+json"
            | "application/schema+json"
            | "application/vnd.api+json"
            | "application/x-yaml"
            | "application/yaml"
            | "application/toml"
    )
}

/// Save fetched PDF bytes to the session download directory and return a
/// `WebFetchOutput` pointing the model at the saved file.
async fn save_pdf(
    writer: &SessionFileWriter,
    session_folder: &Path,
    body: &[u8],
    final_url: String,
    content_type: String,
    status_code: u16,
    read_tool_name: Option<&str>,
) -> Result<WebFetchOutput, WebFetchError> {
    let path = writer
        .save(session_folder, body, None)
        .await
        .map_err(|e| WebFetchError::IoError(std::io::Error::other(e.to_string())))?;

    tracing::info!(
        path = %path.display(),
        bytes = body.len(),
        "PDF saved to disk"
    );

    let read_hint = read_tool_name.map_or_else(String::new, |name| {
        format!(" Use the {name} tool to view its contents.")
    });
    let content = format!(
        "PDF downloaded ({} bytes) and saved to {}.{}",
        body.len(),
        path.display(),
        read_hint,
    );
    Ok(WebFetchOutput::Content(WebFetchContent {
        url: final_url,
        content: content.clone(),
        content_type,
        status_code,
        bytes: body.len(),
        source_artifact: None,
        inline_fallback: Some(content),
        output_location: None,
    }))
}

/// Save fetched image bytes to the session images directory and return a
/// `WebFetchOutput` pointing the model at the saved file.
async fn save_image(
    writer: &SessionFileWriter,
    session_folder: &Path,
    body: &[u8],
    final_url: String,
    content_type: String,
    status_code: u16,
    read_tool_name: Option<&str>,
) -> Result<WebFetchOutput, WebFetchError> {
    let ext = media_extension(&content_type);
    let path = writer
        .save(session_folder, body, Some(ext))
        .await
        .map_err(|e| WebFetchError::IoError(std::io::Error::other(e.to_string())))?;

    tracing::info!(
        path = %path.display(),
        bytes = body.len(),
        "Image saved to disk"
    );

    let read_hint = read_tool_name.map_or_else(String::new, |name| {
        format!(" Use the {name} tool to view its contents.")
    });
    let content = format!(
        "Image downloaded ({} bytes, {}) and saved to {}.{}",
        body.len(),
        content_type,
        path.display(),
        read_hint,
    );
    Ok(WebFetchOutput::Content(WebFetchContent {
        url: final_url,
        content: content.clone(),
        content_type,
        status_code,
        bytes: body.len(),
        source_artifact: None,
        inline_fallback: Some(content),
        output_location: None,
    }))
}

/// Save fetched video bytes to the session videos directory and return a
/// `WebFetchOutput` pointing the model at the saved file.
async fn save_video(
    writer: &SessionFileWriter,
    session_folder: &Path,
    body: &[u8],
    final_url: String,
    content_type: String,
    status_code: u16,
) -> Result<WebFetchOutput, WebFetchError> {
    let ext = media_extension(&content_type);
    let path = writer
        .save(session_folder, body, Some(ext))
        .await
        .map_err(|e| WebFetchError::IoError(std::io::Error::other(e.to_string())))?;

    tracing::info!(
        path = %path.display(),
        bytes = body.len(),
        "Video saved to disk"
    );

    let content = format!(
        "Video downloaded ({} bytes, {}) and saved to {}.",
        body.len(),
        content_type,
        path.display(),
    );
    Ok(WebFetchOutput::Content(WebFetchContent {
        url: final_url,
        content: content.clone(),
        content_type,
        status_code,
        bytes: body.len(),
        source_artifact: None,
        inline_fallback: Some(content),
        output_location: None,
    }))
}

#[cfg(test)]
fn html_to_markdown(converter: &htmd::HtmlToMarkdown, html: &str) -> String {
    let cleaned = extract::clean_chrome(html);
    converter
        .convert(&cleaned)
        .unwrap_or_else(|_| html.to_string())
}

/// Remove common noisy elements from HTML before markdown conversion.
#[cfg(test)]
fn clean_html(html: &str) -> String {
    extract::clean_chrome(html)
}

/// Strip base64 data URIs from content to prevent token bloat.
///
/// Uses manual scanning (`find` + byte matching) instead of regex for
/// lower overhead — no compilation cost and O(n) linear scanning.
fn strip_base64_data_uris(content: String) -> String {
    // A valid base64 quantum is 4 characters; anything shorter is noise.
    const MIN_BASE64_PAYLOAD: usize = 4;
    // RFC 2397 headers are short (MIME + parameters); reject oversized ones
    // to avoid echoing attacker-controlled megabyte strings into the output.
    const MAX_HEADER_LEN: usize = 120;

    if !content.contains("data:") {
        return content;
    }

    let s = content.as_str();
    let mut result = String::with_capacity(s.len());
    let mut last_end = 0;
    let mut search_from = 0;

    while let Some(rel) = s[search_from..].find("data:") {
        let start = search_from + rel;

        // "data:" must look like a URI scheme start, not a substring of
        // another word (e.g. "metadata:", "validata:").
        if start > 0 && s.as_bytes()[start - 1].is_ascii_alphanumeric() {
            search_from = start + 5;
            continue;
        }

        if let Some(rel_comma) = s[start..].find(',') {
            let comma = start + rel_comma;
            let header = &s[start + 5..comma];

            // RFC 2397 forbids whitespace in the header, and real headers
            // are short ASCII. Reject anything that violates this.
            if header.len() > MAX_HEADER_LEN || header.bytes().any(|b| b.is_ascii_whitespace()) {
                search_from = start + 5;
                continue;
            }

            let mut parts = header.split(';');
            let mime = parts
                .next()
                .expect("split always yields at least one element");
            let mime = if mime.is_empty() { "unknown" } else { mime };

            if parts.any(|p| p.eq_ignore_ascii_case("base64")) {
                // Consume valid base64 characters after the comma.
                let payload_start = comma + 1;
                let payload_len = s[payload_start..]
                    .bytes()
                    .take_while(|b| {
                        matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/' | b'=')
                    })
                    .count();

                if payload_len >= MIN_BASE64_PAYLOAD {
                    result.push_str(&s[last_end..start]);
                    result.push_str("[base64 ");
                    result.push_str(mime);
                    result.push_str(" data removed]");
                    last_end = payload_start + payload_len;
                    search_from = last_end;
                    continue;
                }
            }
        }

        search_from = start + 5;
    }

    if last_end == 0 {
        return content;
    }
    result.push_str(&s[last_end..]);
    result
}

// ───────────────────────────────────────────────────────────────────────────
// Tests
// ───────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_converter() -> htmd::HtmlToMarkdown {
        htmd::HtmlToMarkdown::builder()
            .skip_tags(vec![
                "script", "style", "noscript", "svg", "iframe", "object", "embed",
            ])
            .build()
    }

    #[tokio::test]
    async fn oversized_html_persists_exact_pre_truncation_markdown() {
        let tmp = tempfile::tempdir().unwrap();
        let client = WebFetchClient::new(&WebFetchParams {
            context_window_tokens: Some(100),
            ..WebFetchParams::default()
        })
        .unwrap();
        let tail = "TAIL-MUST-REMAIN-RECOVERABLE";
        let html = format!(
            "<h1>Heading</h1>{}<p>{tail}</p>",
            "<p>body line</p>".repeat(1_000)
        );
        let expected = strip_base64_data_uris(html_to_markdown(&test_converter(), &html));

        let processed = client
            .process_text_content(
                html.as_bytes(),
                "text/html; charset=utf-8",
                Some(tmp.path()),
                RecoveryTools {
                    read: Some("ReadAsset"),
                    execute: Some("ExecuteAsset"),
                },
                ExtractMode::Full,
                "https://example.com/long",
                200,
                0,
                None,
                false,
            )
            .await;

        assert!(processed.was_truncated);
        assert_eq!(processed.content_type, "markdown");
        assert_eq!(processed.bytes, expected.len());
        assert!(!processed.content.contains(tail));
        assert!(expected.contains(tail));
        assert!(processed.content.contains("Status: 200"));
        let artifact = tmp.path().join("web_fetch").join("1.md");
        assert!(
            processed.content.contains("Full content saved")
                && (processed.content.contains("1.md") || processed.content.contains("web_fetch")),
            "expected path recovery hint in: {}",
            processed.content
        );
        // Artifact is pure body (no status header) for exact recovery.
        assert_eq!(tokio::fs::read_to_string(artifact).await.unwrap(), expected);
    }

    #[test]
    fn domain_allowlist_blocks_before_network() {
        let client = WebFetchClient::new(&WebFetchParams {
            allowed_domains: Some(vec!["docs.rs".into()]),
            ..WebFetchParams::default()
        })
        .unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let out = rt
            .block_on(client.fetch(
                "https://evil.example.com/steal",
                None,
                None,
                None,
                FetchOptions::default(),
            ))
            .unwrap();
        assert!(matches!(
            out,
            WebFetchOutput::DomainNotAllowed(ref d) if d == "evil.example.com"
        ));
    }

    #[test]
    fn headless_mode_refuses_without_network() {
        let client = WebFetchClient::new(&WebFetchParams::default()).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let out = rt
            .block_on(client.fetch(
                "https://example.com/",
                None,
                None,
                None,
                FetchOptions {
                    extract_mode: Some("headless"),
                    ..FetchOptions::default()
                },
            ))
            .unwrap();
        assert!(matches!(out, WebFetchOutput::Error { .. }));
    }

    #[test]
    fn apply_text_window_respects_offset_and_max() {
        let s = apply_text_window("abcdefghij", 2, Some(3));
        assert!(s.starts_with("cde"));
        assert!(s.contains("offset 2") || s.contains("offset=2"));
    }

    #[test]
    fn refine_content_type_sniffs_pdf_and_html() {
        assert_eq!(refine_content_type("", b"%PDF-1.4"), "application/pdf");
        assert!(refine_content_type("", b"<!DOCTYPE html><html>").contains("html"));
    }

    #[test]
    fn sniff_meta_charset() {
        let html = br#"<html><head><meta charset="iso-8859-1"></head><body>x</body></html>"#;
        assert_eq!(sniff_html_meta_charset(html).as_deref(), Some("iso-8859-1"));
    }

    #[test]
    fn open_public_has_no_matcher() {
        let client = WebFetchClient::new(&WebFetchParams::default()).unwrap();
        assert!(client.domain_matcher.is_none());
    }

    #[test]
    fn parse_charset_from_content_type() {
        assert_eq!(
            parse_charset("text/html; charset=utf-8"),
            Some("utf-8".into())
        );
        assert_eq!(
            parse_charset("text/html; CHARSET=\"ISO-8859-1\""),
            Some("ISO-8859-1".into())
        );
        assert_eq!(parse_charset("text/html"), None);
    }

    #[test]
    fn decode_body_utf8() {
        let s = decode_body(b"hello", "text/plain; charset=utf-8");
        assert_eq!(s, "hello");
    }

    // ── URL validation ──────────────────────────────────────────────────

    #[test]
    fn validate_url_accepts_valid() {
        assert!(validate_url("https://docs.rs/reqwest/latest").is_ok());
        assert!(validate_url("https://github.com/seanmonstar/reqwest").is_ok());
        assert!(validate_url("http://example.com/path?q=1#frag").is_ok());
    }

    #[test]
    fn validate_url_rejects_single_label_hosts() {
        // localhost is an explicit local host; SSRF still blocks it unless
        // allow_local is set on tool params.
        assert!(validate_url("http://localhost:8080/foo").is_ok());
        assert!(validate_url("http://intranet/foo").is_err());
        assert!(validate_url("http://metadata/computeMetadata").is_err());
    }

    #[test]
    fn validate_url_accepts_ipv6_literals() {
        assert!(validate_url("https://[2001:db8::1]/").is_ok());
        assert!(validate_url("http://[::1]/").is_ok());
    }

    #[test]
    fn upgrade_to_https_skips_explicit_local_hosts() {
        let mut local = Url::parse("http://127.0.0.1:8080/").unwrap();
        upgrade_to_https(&mut local);
        assert_eq!(local.scheme(), "http");

        let mut localhost = Url::parse("http://localhost:3000/").unwrap();
        upgrade_to_https(&mut localhost);
        assert_eq!(localhost.scheme(), "http");

        let mut public = Url::parse("http://example.com/").unwrap();
        upgrade_to_https(&mut public);
        assert_eq!(public.scheme(), "https");
    }

    #[test]
    fn validate_url_rejects_credentials() {
        assert!(validate_url("https://user:pass@example.com/foo").is_err());
        assert!(validate_url("https://admin@example.com/foo").is_err());
    }

    #[test]
    fn validate_url_rejects_long_urls() {
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_LENGTH));
        assert!(validate_url(&long).is_err());
    }

    #[test]
    fn validate_url_rejects_unsupported_schemes() {
        assert!(validate_url("ftp://example.com/file.txt").is_err());
        assert!(validate_url("file:///etc/passwd").is_err());
        assert!(validate_url("data:text/html,<h1>hi</h1>").is_err());
    }

    #[test]
    fn validate_url_rejects_garbage() {
        assert!(validate_url("not a url").is_err());
        assert!(validate_url("").is_err());
    }

    #[test]
    fn upgrade_http_to_https() {
        let mut url = Url::parse("http://example.com/path").unwrap();
        upgrade_to_https(&mut url);
        assert_eq!(url.scheme(), "https");
    }

    #[test]
    fn upgrade_https_is_noop() {
        let mut url = Url::parse("https://example.com/path").unwrap();
        upgrade_to_https(&mut url);
        assert_eq!(url.scheme(), "https");
    }

    // ── Same-host redirect check ────────────────────────────────────────

    #[test]
    fn same_host_exact_match() {
        let a = Url::parse("https://example.com/a").unwrap();
        let b = Url::parse("https://example.com/b").unwrap();
        assert!(is_same_host(&a, &b));
    }

    #[test]
    fn www_subdomain_is_cross_host() {
        let a = Url::parse("https://example.com/a").unwrap();
        let c = Url::parse("https://www.example.com/a").unwrap();
        assert!(!is_same_host(&a, &c));
        assert!(!is_same_host(&c, &a));
    }

    #[test]
    fn www_apex_equivalent_for_safe_follow() {
        let a = Url::parse("https://example.com/a").unwrap();
        let c = Url::parse("https://www.example.com/a").unwrap();
        assert!(is_www_apex_equivalent(&a, &c));
        assert!(is_www_apex_equivalent(&c, &a));
        let d = Url::parse("https://example.com./a").unwrap();
        assert!(is_www_apex_equivalent(&a, &d));
        let other = Url::parse("https://other.com/a").unwrap();
        assert!(!is_www_apex_equivalent(&a, &other));
        let sub = Url::parse("https://api.example.com/a").unwrap();
        assert!(!is_www_apex_equivalent(&a, &sub));
    }

    #[test]
    fn different_host_rejected() {
        let a = Url::parse("https://example.com/a").unwrap();
        let d = Url::parse("https://other.com/a").unwrap();
        assert!(!is_same_host(&a, &d));
    }

    #[test]
    fn same_host_redirect_location_reupgrades_http() {
        // Absolute http Location on an https origin must not stay http when
        // followed as a same-host hop (upgrade_to_https reapplied each hop).
        let origin = Url::parse("https://example.com/start").unwrap();
        let mut next = origin.join("http://example.com/next").unwrap();
        assert_eq!(next.scheme(), "http");
        assert!(is_same_host(&origin, &next));
        upgrade_to_https(&mut next);
        assert_eq!(next.scheme(), "https");
        assert_eq!(next.as_str(), "https://example.com/next");
    }

    // ── Content type detection ──────────────────────────────────────────

    #[test]
    fn is_html_detects_html_types() {
        assert!(is_html("text/html"));
        assert!(is_html("text/html; charset=utf-8"));
        assert!(is_html("application/xhtml+xml"));
    }

    #[test]
    fn is_html_rejects_non_html() {
        assert!(!is_html("text/plain"));
        assert!(!is_html("text/markdown"));
        assert!(!is_html("application/json"));
        assert!(!is_html("application/pdf"));
    }

    // ── PDF content type detection ────────────────────────────────────

    #[test]
    fn is_pdf_detects_pdf_types() {
        assert!(is_pdf("application/pdf"));
        assert!(is_pdf("application/pdf; charset=utf-8"));
    }

    #[test]
    fn is_pdf_rejects_non_pdf() {
        assert!(!is_pdf("text/html"));
        assert!(!is_pdf("text/plain"));
        assert!(!is_pdf("application/json"));
        assert!(!is_pdf("application/octet-stream"));
    }

    // ── Binary content type detection ────────────────────────────────

    #[test]
    fn binary_detects_images_and_media() {
        assert!(is_binary_content_type("image/png"));
        assert!(is_binary_content_type("image/jpeg"));
        assert!(is_binary_content_type("image/gif"));
        assert!(is_binary_content_type("audio/mpeg"));
        assert!(is_binary_content_type("video/mp4"));
        assert!(is_binary_content_type("application/octet-stream"));
        assert!(is_binary_content_type("application/zip"));
        assert!(is_binary_content_type("application/wasm"));
        assert!(is_binary_content_type("font/woff2"));
    }

    #[test]
    fn binary_allows_text_types() {
        assert!(!is_binary_content_type("text/plain"));
        assert!(!is_binary_content_type("text/html"));
        assert!(!is_binary_content_type("text/html; charset=utf-8"));
        assert!(!is_binary_content_type("text/markdown"));
        assert!(!is_binary_content_type("text/csv"));
        assert!(!is_binary_content_type("text/xml"));
    }

    #[test]
    fn binary_allows_text_like_application_types() {
        assert!(!is_binary_content_type("application/json"));
        assert!(!is_binary_content_type("application/json; charset=utf-8"));
        assert!(!is_binary_content_type("application/xml"));
        assert!(!is_binary_content_type("application/javascript"));
        assert!(!is_binary_content_type("application/xhtml+xml"));
        // SVG is rejected by is_binary but admitted via is_svg_xml on the fetch path.
        assert!(is_binary_content_type("application/svg+xml"));
        assert!(is_svg_xml("image/svg+xml"));
        assert!(!is_binary_content_type("application/ld+json"));
        assert!(!is_binary_content_type("application/yaml"));
        assert!(!is_binary_content_type("application/graphql"));
    }

    // ── HTML to markdown conversion ─────────────────────────────────────

    #[test]
    fn html_to_markdown_basic() {
        let md = html_to_markdown(&test_converter(), "<h1>Hello</h1><p>World</p>");
        assert!(md.contains("Hello"));
        assert!(md.contains("World"));
    }

    #[test]
    fn html_to_markdown_strips_script_and_style() {
        let html = r#"<h1>Title</h1>
            <script>var x = 1; alert("hi");</script>
            <style>body { color: red; }</style>
            <p>Content</p>"#;
        let md = html_to_markdown(&test_converter(), html);
        assert!(md.contains("Title"));
        assert!(md.contains("Content"));
        assert!(!md.contains("alert"), "script content should be stripped");
        assert!(
            !md.contains("color: red"),
            "style content should be stripped"
        );
    }

    #[test]
    fn html_to_markdown_strips_noscript() {
        let html = "<p>Visible</p><noscript>Enable JavaScript</noscript>";
        let md = html_to_markdown(&test_converter(), html);
        assert!(md.contains("Visible"));
        assert!(
            !md.contains("Enable JavaScript"),
            "noscript content should be stripped"
        );
    }

    #[test]
    fn html_to_markdown_strips_svg() {
        let html = r#"<p>Text</p><svg xmlns="http://www.w3.org/2000/svg"><circle r="5"/></svg>"#;
        let md = html_to_markdown(&test_converter(), html);
        assert!(md.contains("Text"));
        assert!(!md.contains("circle"), "SVG content should be stripped");
    }

    #[test]
    fn html_to_markdown_preserves_links() {
        let html = r#"<a href="https://example.com">Click here</a>"#;
        let md = html_to_markdown(&test_converter(), html);
        assert!(md.contains("[Click here]"));
        assert!(md.contains("(https://example.com)"));
    }

    #[test]
    fn html_to_markdown_preserves_code_blocks() {
        let html = "<pre><code>fn main() {}</code></pre>";
        let md = html_to_markdown(&test_converter(), html);
        assert!(md.contains("fn main() {}"));
    }

    #[test]
    fn html_to_markdown_preserves_lists() {
        let html = "<ul><li>One</li><li>Two</li><li>Three</li></ul>";
        let md = html_to_markdown(&test_converter(), html);
        assert!(md.contains("One"));
        assert!(md.contains("Two"));
        assert!(md.contains("Three"));
    }

    #[test]
    fn html_to_markdown_handles_nested_elements() {
        let html = r#"<p>This is <strong>bold <em>and italic</em></strong> text.</p>"#;
        let md = html_to_markdown(&test_converter(), html);
        assert!(md.contains("bold"));
        assert!(md.contains("and italic"));
    }

    #[test]
    fn html_to_markdown_handles_empty_input() {
        let md = html_to_markdown(&test_converter(), "");
        assert!(md.is_empty() || md.trim().is_empty());
    }

    #[test]
    fn html_to_markdown_strips_iframe() {
        let html = r#"<p>Content</p><iframe src="https://evil.com/embed"></iframe><p>More</p>"#;
        let md = html_to_markdown(&test_converter(), html);
        assert!(md.contains("Content"));
        assert!(md.contains("More"));
        assert!(!md.contains("evil.com"), "iframe should be stripped");
    }

    #[test]
    fn html_to_markdown_full_page_strips_boilerplate() {
        let html = r#"<!DOCTYPE html>
<html>
<head>
  <title>Test Page</title>
  <script>var tracking = true;</script>
  <style>body { margin: 0; }</style>
</head>
<body>
  <nav><a href="/">Home</a><a href="/about">About</a></nav>
  <main>
    <h1>Article Title</h1>
    <p>This is the main content of the article.</p>
  </main>
  <footer>Copyright 2025</footer>
  <script>analytics.track('pageview');</script>
</body>
</html>"#;
        let md = html_to_markdown(&test_converter(), html);
        assert!(md.contains("Article Title"), "article title should survive");
        assert!(md.contains("main content"), "article body should survive");
        assert!(
            !md.contains("tracking"),
            "head script content should be stripped"
        );
        assert!(
            !md.contains("analytics.track"),
            "body script content should be stripped"
        );
        assert!(
            !md.contains("margin: 0"),
            "style content should be stripped"
        );
    }

    #[test]
    fn html_to_markdown_handles_tables() {
        let html = r#"<table>
            <tr><th>Name</th><th>Value</th></tr>
            <tr><td>Alpha</td><td>1</td></tr>
            <tr><td>Beta</td><td>2</td></tr>
        </table>"#;
        let md = html_to_markdown(&test_converter(), html);
        assert!(md.contains("Name"));
        assert!(md.contains("Alpha"));
        assert!(md.contains("Beta"));
    }

    // ── Base64 data URI stripping ─────────────────────────────────────

    /// Golden test: verify exact output format for the most common case.
    #[test]
    fn strip_base64_output_format() {
        let result = strip_base64_data_uris(
            "Before ![logo](data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==) after".to_owned(),
        );
        assert_eq!(
            result,
            "Before ![logo]([base64 image/png data removed]) after"
        );
    }

    /// Inputs the scanner must leave untouched.
    #[test]
    fn strip_base64_rejects_invalid() {
        let cases: &[&str] = &[
            // whitespace in header (RFC 2397 forbids)
            "data: image/png ;base64,AAAA== end",
            // payload too short (< 4 chars)
            "data:image/png;base64,AA= end",
            // empty payload
            "data:image/png;base64, end",
            // no comma after data:
            "data:image/png;base64 with no comma",
            // truncated data: at EOF
            "trailing data:",
        ];
        for input in cases {
            assert_eq!(
                strip_base64_data_uris(input.to_string()),
                *input,
                "should not strip: {input:?}"
            );
        }
    }

    /// Oversized header must be rejected (can't be a static string).
    #[test]
    fn strip_base64_rejects_oversized_header() {
        let huge_mime = "x".repeat(10_000);
        let input = format!("data:{huge_mime};base64,AAAA== end");
        assert_eq!(strip_base64_data_uris(input.clone()), input);
    }

    /// Multi-param headers and case-insensitive marker — capabilities
    /// the manual scanner has beyond the original regex.
    #[test]
    fn strip_base64_extended_headers() {
        assert_eq!(
            strip_base64_data_uris("data:text/plain;charset=utf-8;base64,SGVsbG8= end".to_owned()),
            "[base64 text/plain data removed] end"
        );
        assert!(
            strip_base64_data_uris("data:image/png;Base64,AAAA==".to_owned())
                .contains("[base64 image/png data removed]")
        );
    }

    // ── Regex equivalence ──────────────────────────────────────────────

    /// Reference implementation: the original regex-based stripper.
    fn strip_base64_data_uris_regex(content: &str) -> String {
        let re = regex::Regex::new(r"data:([^;,\s]{1,80});base64,[A-Za-z0-9+/=]+")
            .expect("valid base64 data URI regex");
        re.replace_all(content, |caps: &regex::Captures| {
            let mime = caps.get(1).map_or("unknown", |m| m.as_str());
            format!("[base64 {mime} data removed]")
        })
        .into_owned()
    }

    /// Both implementations must produce identical output on all realistic
    /// inputs. Covers: markdown images, standalone URIs, multiple URIs,
    /// normal URLs, non-base64 data URIs, HTML/CSS contexts, various
    /// positions, and real-world payloads.
    #[test]
    fn strip_base64_equivalence_with_regex() {
        let cases: &[&str] = &[
            "no data URIs here",
            "![logo](data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==) after",
            "See data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0 here",
            "![a](data:image/png;base64,AAAA==) text ![b](data:image/jpeg;base64,/9j/4AAQ==)",
            "![img](https://example.com/logo.png) and [link](https://example.com)",
            "data:text/html,%3Ch1%3EHello%3C%2Fh1%3E",
            "Just normal markdown with **bold** and [link](https://x.com)",
            r#"<img src="data:image/png;base64,iVBORw0KGgo="> text"#,
            "background: url(data:image/svg+xml;base64,PHN2Zz4=) no-repeat",
            "before data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7 after",
            "data:image/png;base64,AA==",
            "data:application/octet-stream;base64,AQID",
            "x data:a/b;base64,AAAA== y data:c/d;base64,BBBB== z",
            "data:image/png;base64,AAAA== rest",
            "rest data:image/png;base64,AAAA==",
            "data:image/png;base64,AAAA==",
            "data:image/x-icon;base64,AAABAAEAEBAQAAEABAAoAQAAFgAAACgAAAAQAAAAIAAAAAEABAAAAAAAgAAA",
        ];
        for input in cases {
            let regex_out = strip_base64_data_uris_regex(input);
            let manual_out = strip_base64_data_uris(input.to_string());
            assert_eq!(manual_out, regex_out, "mismatch on input: {input:?}");
        }
    }

    /// The manual scanner correctly rejects word-internal "data:" prefixes
    /// that the regex would false-positive on.
    #[test]
    fn strip_base64_rejects_word_internal_data_prefix() {
        for input in ["metadata:foo;base64,AAAA==", "validata:bar;base64,BBBB=="] {
            let manual_out = strip_base64_data_uris(input.to_string());
            let regex_out = strip_base64_data_uris_regex(input);
            assert_eq!(
                manual_out, input,
                "manual scanner should NOT strip: {input:?}"
            );
            assert_ne!(
                regex_out, manual_out,
                "expected divergence from regex on: {input:?}"
            );
        }
    }

    // ── Proxy configs ─────────────────────────────────────

    #[test]
    fn proxy_endpoint_round_trips_through_config() {
        let json = r#"{"proxy_endpoint": "https://proxy.corp.example.com", "allowed_domains": ["example.com"]}"#;
        let params: WebFetchParams = serde_json::from_str(json).unwrap();
        assert_eq!(
            params.proxy_endpoint.as_deref(),
            Some("https://proxy.corp.example.com")
        );

        // Client builds successfully with the proxy endpoint set.
        let client = WebFetchClient::new(&params);
        assert!(client.is_ok());
    }

    #[test]
    fn proxy_endpoint_defaults_to_none() {
        let json = r#"{"allowed_domains": ["example.com"]}"#;
        let params: WebFetchParams = serde_json::from_str(json).unwrap();
        assert!(params.proxy_endpoint.is_none());
    }

    // ── HTML cleaning (scraper-based) ─────────────────────────────────

    #[test]
    fn clean_html_removes_boilerplate_tags() {
        let html = r#"<body><nav>Menu</nav><header>Header</header><main>Content</main><footer>Footer</footer></body>"#;
        let cleaned = clean_html(html);
        assert!(cleaned.contains("Content"));
        assert!(!cleaned.contains("Menu"));
        assert!(!cleaned.contains("Header"));
        assert!(!cleaned.contains("Footer"));
    }

    #[test]
    fn clean_html_removes_noisy_classes_and_ids() {
        let html = r#"<div class="cookie-banner">cookies</div><aside id="sidebar">side</aside><div class="ad-banner">ad</div>"#;
        let cleaned = clean_html(html);
        assert!(!cleaned.contains("cookie-banner"));
        assert!(!cleaned.contains("sidebar"));
        assert!(!cleaned.contains("ad-banner"));
    }

    #[test]
    fn clean_html_keeps_root_when_it_matches_selector() {
        let html =
            r#"<html class="advert"><body><nav>Menu</nav><main>Content</main></body></html>"#;
        let cleaned = clean_html(html);
        assert!(cleaned.contains("Content"));
        assert!(!cleaned.contains("Menu"));
    }

    // ── Image content type detection ──────────────────────────────────

    #[test]
    fn is_image_detects_image_types() {
        assert!(is_image("image/png"));
        assert!(is_image("image/jpeg"));
        assert!(is_image("image/gif"));
        assert!(is_image("image/webp"));
        assert!(is_image("image/bmp"));
        assert!(is_image("image/tiff"));
        assert!(is_image("image/png; charset=utf-8"));
    }

    #[test]
    fn is_image_rejects_svg() {
        assert!(!is_image("image/svg+xml"));
        assert!(!is_image("image/svg+xml; charset=utf-8"));
    }

    #[test]
    fn is_image_rejects_non_image() {
        assert!(!is_image("text/html"));
        assert!(!is_image("video/mp4"));
        assert!(!is_image("application/pdf"));
        assert!(!is_image("application/octet-stream"));
    }

    // ── Video content type detection ──────────────────────────────────

    #[test]
    fn is_video_detects_video_types() {
        assert!(is_video("video/mp4"));
        assert!(is_video("video/webm"));
        assert!(is_video("video/quicktime"));
        assert!(is_video("video/x-msvideo"));
        assert!(is_video("video/mp4; codecs=avc1"));
    }

    #[test]
    fn is_video_rejects_non_video() {
        assert!(!is_video("text/html"));
        assert!(!is_video("image/png"));
        assert!(!is_video("application/pdf"));
        assert!(!is_video("audio/mpeg"));
    }

    // ── Magic byte validation ─────────────────────────────────────────

    #[test]
    fn magic_bytes_valid_png() {
        let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        assert!(validate_media_magic_bytes("image/png", &png));
    }

    #[test]
    fn magic_bytes_valid_jpeg() {
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        assert!(validate_media_magic_bytes("image/jpeg", &jpeg));
    }

    #[test]
    fn magic_bytes_valid_gif() {
        assert!(validate_media_magic_bytes("image/gif", b"GIF89a\x01\x00"));
        assert!(validate_media_magic_bytes("image/gif", b"GIF87a\x01\x00"));
    }

    #[test]
    fn magic_bytes_valid_webp() {
        let mut webp = vec![0u8; 12];
        webp[..4].copy_from_slice(b"RIFF");
        webp[8..12].copy_from_slice(b"WEBP");
        assert!(validate_media_magic_bytes("image/webp", &webp));
    }

    #[test]
    fn magic_bytes_valid_mp4() {
        let mut mp4 = vec![0u8; 12];
        mp4[4..8].copy_from_slice(b"ftyp");
        assert!(validate_media_magic_bytes("video/mp4", &mp4));
    }

    #[test]
    fn magic_bytes_valid_webm() {
        let webm = [0x1A, 0x45, 0xDF, 0xA3, 0x01, 0x00];
        assert!(validate_media_magic_bytes("video/webm", &webm));
    }

    #[test]
    fn magic_bytes_rejects_mismatch() {
        // HTML content masquerading as PNG
        assert!(!validate_media_magic_bytes("image/png", b"<html>"));
        // PNG bytes claimed as JPEG
        assert!(!validate_media_magic_bytes(
            "image/jpeg",
            &[0x89, 0x50, 0x4E, 0x47]
        ));
        // Empty body
        assert!(!validate_media_magic_bytes("image/png", &[]));
        // Text claimed as video
        assert!(!validate_media_magic_bytes("video/mp4", b"not video"));
    }

    #[test]
    fn magic_bytes_allows_unknown_subtypes() {
        // Unknown image/video subtypes should pass (fail-open)
        assert!(validate_media_magic_bytes("image/x-custom", b"anything"));
        assert!(validate_media_magic_bytes("video/x-custom", b"anything"));
    }

    // ── Media extension mapping ───────────────────────────────────────

    #[test]
    fn media_extension_maps_known_types() {
        assert_eq!(media_extension("image/png"), "png");
        assert_eq!(media_extension("image/jpeg"), "jpg");
        assert_eq!(media_extension("image/gif"), "gif");
        assert_eq!(media_extension("image/webp"), "webp");
        assert_eq!(media_extension("image/bmp"), "bmp");
        assert_eq!(media_extension("image/tiff"), "tiff");
        assert_eq!(media_extension("video/mp4"), "mp4");
        assert_eq!(media_extension("video/webm"), "webm");
        assert_eq!(media_extension("video/quicktime"), "mov");
        assert_eq!(media_extension("video/x-msvideo"), "avi");
    }

    #[test]
    fn media_extension_handles_params() {
        assert_eq!(media_extension("image/png; charset=utf-8"), "png");
        assert_eq!(media_extension("video/mp4; codecs=avc1"), "mp4");
    }

    #[test]
    fn media_extension_unknown_falls_back_to_bin() {
        assert_eq!(media_extension("image/x-custom"), "bin");
        assert_eq!(media_extension("video/x-custom"), "bin");
        assert_eq!(media_extension("application/octet-stream"), "bin");
    }
}
