//! Main-content extraction and SPA-shell heuristics for `web_fetch`.
//!
//! Modes:
//! - `full` — clean chrome (nav/header/footer/…) then convert whole document
//! - `article` — prefer `<main>` / `<article>` / role=main / high-density blocks
//! - `auto` — article when a solid main block is found, else full
//! - `raw` — skip structural chrome removal (script/style still skipped by htmd)

use scraper::{Html, Selector};
use url::Url;

/// How aggressively to reduce HTML before markdown conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExtractMode {
    /// Prefer article/main when confident; otherwise full cleaned page.
    #[default]
    Auto,
    /// Force main-content extraction; fall back to full if extract is tiny.
    Article,
    /// Whole cleaned document (nav/header/footer stripped).
    Full,
    /// No structural chrome strip; only converter skip-tags apply.
    Raw,
    /// Reserved: JS-rendered pages need a browser MCP — web_fetch refuses.
    Headless,
}

impl ExtractMode {
    pub fn parse(raw: Option<&str>) -> Self {
        let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
            return Self::Auto;
        };
        match s.to_ascii_lowercase().as_str() {
            "article" | "main" | "readability" => Self::Article,
            "full" | "page" | "document" => Self::Full,
            "raw" | "none" | "minimal" => Self::Raw,
            "headless" | "browser" | "js" => Self::Headless,
            "auto" | "default" => Self::Auto,
            _ => Self::Auto,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Article => "article",
            Self::Full => "full",
            Self::Raw => "raw",
            Self::Headless => "headless",
        }
    }
}

/// Prepare HTML for conversion according to `mode`.
///
/// Returns `(html_fragment, mode_used, spa_shell_hint)`.
pub fn prepare_html(html: &str, mode: ExtractMode) -> (String, ExtractMode, bool) {
    let spa = looks_like_spa_shell(html);
    match mode {
        ExtractMode::Headless => (html.to_string(), ExtractMode::Headless, spa),
        ExtractMode::Raw => (html.to_string(), ExtractMode::Raw, spa),
        ExtractMode::Full => (clean_chrome(html), ExtractMode::Full, spa),
        ExtractMode::Article => {
            // Fall back to full chrome-cleaned page when the main fragment is tiny.
            if let Some(main) = extract_main_content(html) {
                if text_len_estimate(&main) >= 200 {
                    (main, ExtractMode::Article, spa)
                } else {
                    (clean_chrome(html), ExtractMode::Full, spa)
                }
            } else {
                (clean_chrome(html), ExtractMode::Full, spa)
            }
        }
        ExtractMode::Auto => {
            if let Some(main) = extract_main_content(html) {
                // Prefer article only when it carries meaningful text.
                if text_len_estimate(&main) >= 200 {
                    (main, ExtractMode::Article, spa)
                } else {
                    (clean_chrome(html), ExtractMode::Full, spa)
                }
            } else {
                (clean_chrome(html), ExtractMode::Full, spa)
            }
        }
    }
}

/// Max total UTF-8 bytes for the optional ## Links section (token bound).
pub const LINK_SUMMARY_MAX_BYTES: usize = 1_500;
/// Max number of links in the summary.
pub const LINK_SUMMARY_MAX_COUNT: usize = 8;

/// Collect unique http(s) links from HTML (prefer post-extract fragment),
/// capped by count and total section bytes.
pub fn extract_link_summary(html: &str, base_url: &str, limit: usize) -> String {
    extract_link_summary_budgeted(html, base_url, limit, LINK_SUMMARY_MAX_BYTES)
}

pub fn extract_link_summary_budgeted(
    html: &str,
    base_url: &str,
    limit: usize,
    max_bytes: usize,
) -> String {
    if limit == 0 || max_bytes == 0 {
        return String::new();
    }
    let document = Html::parse_document(html);
    let Ok(selector) = Selector::parse("a[href]") else {
        return String::new();
    };
    let base = Url::parse(base_url).ok();
    let mut seen = std::collections::HashSet::new();
    let mut lines = Vec::new();
    let mut used = "## Links\n".len();
    for el in document.select(&selector) {
        let href = el.value().attr("href").unwrap_or("").trim();
        if href.is_empty()
            || href.starts_with('#')
            || href.starts_with("javascript:")
            || href.starts_with("mailto:")
            || href.starts_with("tel:")
            || href.starts_with("data:")
        {
            continue;
        }
        let abs = if let Some(ref b) = base {
            b.join(href)
                .map(|u| u.to_string())
                .unwrap_or_else(|_| href.to_string())
        } else {
            href.to_string()
        };
        // Prefer navigable http(s) links; skip assets.
        let lower = abs.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with('/'))
        {
            continue;
        }
        if looks_like_static_asset(&lower) {
            continue;
        }
        // Strip common tracking query noise for token savings.
        let abs = strip_tracking_query(&abs);
        if !seen.insert(abs.clone()) {
            continue;
        }
        let text: String = el
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let label: String = if text.is_empty() {
            abs.chars().take(60).collect()
        } else {
            text.chars().take(60).collect()
        };
        let line = format!("- [{label}]({abs})");
        if used + line.len() + 1 > max_bytes {
            break;
        }
        used += line.len() + 1;
        lines.push(line);
        if lines.len() >= limit {
            break;
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    format!("\n\n## Links\n{}", lines.join("\n"))
}

fn looks_like_static_asset(url_lower: &str) -> bool {
    const EXTS: &[&str] = &[
        ".css", ".js", ".mjs", ".map", ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".ico",
        ".woff", ".woff2", ".ttf", ".eot", ".mp4", ".webm", ".mp3", ".zip", ".gz",
    ];
    let path = url_lower.split('?').next().unwrap_or(url_lower);
    EXTS.iter().any(|e| path.ends_with(e))
}

fn strip_tracking_query(url: &str) -> String {
    let Ok(mut u) = Url::parse(url) else {
        return url.to_string();
    };
    if u.query().is_none() {
        return url.to_string();
    }
    let pairs: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| {
            let k = k.to_ascii_lowercase();
            !(k.starts_with("utm_")
                || k == "fbclid"
                || k == "gclid"
                || k == "mc_cid"
                || k == "mc_eid"
                || k == "ref"
                || k == "ref_src")
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if pairs.is_empty() {
        u.set_query(None);
    } else {
        let q = pairs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        u.set_query(Some(&q));
    }
    u.to_string()
}

/// Detect Cloudflare / bot-challenge interstitials that often return HTTP 200.
pub fn looks_like_bot_challenge(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    let markers = [
        "cf-browser-verification",
        "cf-challenge",
        "challenge-platform",
        "just a moment...",
        "just a moment…",
        "checking your browser before accessing",
        "enable javascript and cookies to continue",
        "attention required! | cloudflare",
        "cdn-cgi/challenge",
        "datadome",
        "_pxhd",
        "captcha-delivery.com",
        "hcaptcha.com",
        "challenges.cloudflare.com",
        "turnstile",
        "why have i been blocked",
        "access denied | cloudflare",
        "please wait while we verify",
        "bot detection",
        "security check",
    ];
    let hits = markers.iter().filter(|m| lower.contains(*m)).count();
    if hits >= 1 && text_len_estimate(html) < 2_500 {
        return true;
    }
    // Strong multi-marker even on longer pages.
    hits >= 2
}

/// True when the HTML looks like a client-rendered shell with little server text.
pub fn looks_like_spa_shell(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    let script_count = lower.matches("<script").count();
    let text_estimate = text_len_estimate(html);

    // Classic empty-root SPA shells.
    let empty_root = (lower.contains("id=\"root\"") || lower.contains("id='root'")
        || lower.contains("id=\"app\"")
        || lower.contains("id='app'"))
        && text_estimate < 400
        && script_count >= 2;

    let enable_js = lower.contains("enable javascript")
        || lower.contains("enable js")
        || lower.contains("requires javascript")
        || lower.contains("you need to enable javascript");

    empty_root || (enable_js && text_estimate < 600)
}

/// True when markdown after conversion is still suspiciously empty for an SPA.
pub fn markdown_looks_empty(md: &str) -> bool {
    let trimmed: String = md
        .chars()
        .filter(|c| !c.is_whitespace())
        .take(80)
        .collect();
    trimmed.len() < 40
}

fn text_len_estimate(html: &str) -> usize {
    // Cheap strip of tags for density scoring — not a full HTML parser.
    let mut out = String::with_capacity(html.len() / 4);
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().map(str::len).sum()
}

/// Prefer semantic main content containers.
fn extract_main_content(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    let candidates: &[&str] = &[
        "main",
        "article",
        "[role='main']",
        "[role=\"main\"]",
        ".markdown-body",
        ".post-content",
        ".article-content",
        ".entry-content",
        "#content",
        "#main-content",
        ".prose",
    ];

    let mut best: Option<(usize, String)> = None;
    for sel in candidates {
        let Ok(selector) = Selector::parse(sel) else {
            continue;
        };
        for el in document.select(&selector) {
            let fragment = el.html();
            let score = score_block(&fragment);
            if score < 120 {
                continue;
            }
            if best.as_ref().is_none_or(|(s, _)| score > *s) {
                best = Some((score, fragment));
            }
        }
    }

    if best.is_none() {
        // Density fallback: largest <div>/<section> by text with low link ratio.
        best = density_fallback(&document);
    }

    best.map(|(_, html)| html)
}

fn density_fallback(document: &Html) -> Option<(usize, String)> {
    let Ok(selector) = Selector::parse("div, section") else {
        return None;
    };
    let mut best: Option<(usize, String)> = None;
    for el in document.select(&selector) {
        let fragment = el.html();
        let score = score_block(&fragment);
        if score < 280 {
            continue;
        }
        if best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, fragment));
        }
    }
    best
}

/// Higher is better: text mass minus link-heavy chrome.
fn score_block(html: &str) -> usize {
    let text = text_len_estimate(html);
    if text == 0 {
        return 0;
    }
    let lower = html.to_ascii_lowercase();
    let links = lower.matches("<a ").count() + lower.matches("<a\n").count();
    let link_penalty = links.saturating_mul(12);
    text.saturating_sub(link_penalty)
}

/// Remove common noisy chrome elements (shared with historical clean_html).
pub fn clean_chrome(html: &str) -> String {
    let mut document = Html::parse_document(html);

    let root_id = document
        .tree
        .root()
        .children()
        .find(|child| child.value().is_element())
        .map(|node| node.id());

    let selectors: Vec<Selector> = [
        "nav",
        "header",
        "footer",
        "aside",
        "[class*='cookie']",
        "[class*='sidebar']",
        "[class*='ad-']",
        "[class*='advert']",
        "[class*='related']",
        "[class*='social']",
        "[class*='share-']",
        "[class*='comments']",
        "[id*='cookie']",
        "[id*='sidebar']",
        "[id*='ad-']",
        "[id*='advert']",
        "[id*='comments']",
        "[id*='related']",
    ]
    .iter()
    .filter_map(|s| Selector::parse(s).ok())
    .collect();

    selectors.iter().for_each(|selector| {
        document
            .select(selector)
            .map(|e| e.id())
            .collect::<Vec<_>>()
            .into_iter()
            .for_each(|id| {
                if Some(id) == root_id {
                    return;
                }
                if let Some(mut node) = document.tree.get_mut(id) {
                    node.detach();
                }
            });
    });

    document.html()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_modes() {
        assert_eq!(ExtractMode::parse(None), ExtractMode::Auto);
        assert_eq!(ExtractMode::parse(Some("article")), ExtractMode::Article);
        assert_eq!(ExtractMode::parse(Some("FULL")), ExtractMode::Full);
        assert_eq!(ExtractMode::parse(Some("raw")), ExtractMode::Raw);
        assert_eq!(ExtractMode::parse(Some("weird")), ExtractMode::Auto);
    }

    #[test]
    fn article_tiny_main_falls_back_to_full() {
        let html = r#"<html><body>
            <main><p>short</p></main>
            <div class="body"><p>This is a long enough secondary body with real documentation
            paragraphs that should survive when main is too small for article mode extraction.</p>
            <p>More content here to pad the full page text estimate substantially for fallback.</p></div>
        </body></html>"#;
        let (out, mode, _) = prepare_html(html, ExtractMode::Article);
        assert_eq!(mode, ExtractMode::Full);
        assert!(out.contains("secondary body") || out.contains("documentation"));
    }

    #[test]
    fn challenge_page_detected() {
        let html = r#"<html><body>
            <h1>Just a moment...</h1>
            <div id="cf-challenge-running">Checking your browser before accessing example.com</div>
            <script src="/cdn-cgi/challenge-platform/h/g/orchestrate/chl_page"></script>
        </body></html>"#;
        assert!(looks_like_bot_challenge(html));
    }

    #[test]
    fn link_summary_skips_assets_and_caps_bytes() {
        let html = r#"<html><body>
            <a href="/docs/a">Docs A</a>
            <a href="/static/app.js">JS</a>
            <a href="mailto:x@y.com">mail</a>
            <a href="https://example.com/b?utm_source=x">B</a>
        </body></html>"#;
        let s = extract_link_summary(html, "https://example.com/", 8);
        assert!(s.contains("Docs A"));
        assert!(!s.contains("app.js"));
        assert!(!s.contains("mailto"));
        assert!(!s.contains("utm_source"));
    }

    #[test]
    fn article_prefers_main() {
        let html = r#"<html><body>
            <nav>Home About</nav>
            <main><h1>Title</h1><p>This is a long enough article body with real paragraphs for scoring purposes and more words here to clear the two hundred character bar.</p>
            <p>Second paragraph continues with still more meaningful content for the density check and ensures the main fragment is selected instead of full fallback.</p>
            <p>Third paragraph adds yet more prose so text_len_estimate stays comfortably above the threshold for article mode.</p></main>
            <footer>Copyright</footer>
        </body></html>"#;
        let (out, mode, _) = prepare_html(html, ExtractMode::Article);
        assert_eq!(mode, ExtractMode::Article);
        assert!(out.contains("Title"));
        assert!(out.contains("long enough article"));
        assert!(!out.contains("Copyright"));
    }

    #[test]
    fn spa_shell_detected() {
        let html = r#"<html><body><div id="root"></div>
            <script src="a.js"></script><script src="b.js"></script>
            <noscript>Enable JavaScript to continue</noscript>
        </body></html>"#;
        assert!(looks_like_spa_shell(html));
    }

    #[test]
    fn full_doc_not_spa() {
        let html = r#"<html><body><main><p>Lots of real documentation text about the API and how to use it effectively in production systems with examples.</p>
        <p>More paragraphs with guidance, warnings, and reference material for developers.</p></main></body></html>"#;
        assert!(!looks_like_spa_shell(html));
    }
}
