//! Domain allowlist matching with precomputed host → path-prefix lookup.
//!
//! Entries are parsed once at construction into a `HashMap<host, HostEntry>`
//! giving O(1) host lookup followed by a tiny linear scan over path prefixes
//! for that host.

use std::collections::HashMap;

use url::Url;

use crate::types::output::WebFetchOutput;

// ───────────────────────────────────────────────────────────────────────────
// Domain normalization
// ───────────────────────────────────────────────────────────────────────────

/// Canonical form for domain comparison: trim whitespace, strip trailing
/// slashes and dots, remove `www.` prefix, and lowercase.
pub fn normalize_domain(raw: &str) -> String {
    let s = raw.trim().trim_end_matches('/').trim_end_matches('.');
    let s = s.to_lowercase();
    // Lowercase first so WWW.example.com → www.example.com → example.com.
    s.strip_prefix("www.").unwrap_or(&s).to_string()
}

// ───────────────────────────────────────────────────────────────────────────
// Precomputed host entry
// ───────────────────────────────────────────────────────────────────────────

/// What a single host is allowed to serve.
#[derive(Debug, Clone)]
enum HostEntry {
    /// Any path on this host is allowed (host-only entry).
    AnyPath,
    /// Only paths matching one of these prefixes are allowed.
    /// Each prefix is normalized (leading `/`, no trailing `/`, lowercased).
    PathPrefixes(Vec<String>),
}

// ───────────────────────────────────────────────────────────────────────────
// DomainMatcher
// ───────────────────────────────────────────────────────────────────────────

/// Precomputed domain allowlist. Built once from the raw allowlist entries,
/// provides O(1) host lookup + small linear scan over path prefixes and
/// suffix wildcards (`*.example.com`).
#[derive(Debug, Clone)]
pub struct DomainMatcher {
    entries: HashMap<String, HostEntry>,
    /// Suffix wildcards: host `api.example.com` matches entry `*.example.com`
    /// (stored as suffix `example.com`).
    suffix_entries: Vec<(String, HostEntry)>,
}

impl DomainMatcher {
    /// Build from raw allowlist entries like `"docs.rs"`, `"vercel.com/docs"`,
    /// or `"*.example.com"`.
    pub fn new(raw_entries: &[String]) -> Self {
        let mut entries: HashMap<String, HostEntry> = HashMap::new();
        let mut suffix_entries: Vec<(String, HostEntry)> = Vec::new();

        for raw in raw_entries {
            let normalized = normalize_domain(raw);
            if normalized.is_empty() {
                continue;
            }

            // Split on first '/' to separate host from optional path.
            let (mut host, path) = match normalized.find('/') {
                Some(i) => (normalized[..i].to_owned(), Some(&normalized[i..])),
                None => (normalized, None),
            };

            let wildcard = if let Some(rest) = host.strip_prefix("*.") {
                host = rest.to_owned();
                true
            } else if host.starts_with('.') {
                host = host.trim_start_matches('.').to_owned();
                true
            } else {
                false
            };

            let entry = match path {
                None => HostEntry::AnyPath,
                Some(raw_path) => {
                    let prefix = raw_path.trim_end_matches('/');
                    if prefix.is_empty() || prefix == "/" {
                        HostEntry::AnyPath
                    } else if prefix.starts_with('/') {
                        HostEntry::PathPrefixes(vec![prefix.to_owned()])
                    } else {
                        HostEntry::PathPrefixes(vec![format!("/{prefix}")])
                    }
                }
            };

            if wildcard {
                merge_suffix(&mut suffix_entries, host, entry);
            } else {
                merge_exact(&mut entries, host, entry);
            }
        }

        Self {
            entries,
            suffix_entries,
        }
    }

    /// Returns `None` if the URL is permitted, or `Some(WebFetchOutput::DomainNotAllowed)`
    /// if it should be blocked. When `entries` is empty, all fetches are blocked.
    pub fn check(&self, url: &Url) -> Option<WebFetchOutput> {
        let Some(raw_host) = url.host_str() else {
            return Some(WebFetchOutput::DomainNotAllowed(String::new()));
        };
        let host = normalize_domain(raw_host);
        let url_path = normalize_url_path(url.path());

        if let Some(entry) = self.entries.get(&host) {
            return path_allowed(entry, &host, &url_path);
        }

        // Prefer the longest (most specific) matching suffix wildcard.
        let mut best: Option<(usize, &HostEntry)> = None;
        for (suffix, entry) in &self.suffix_entries {
            if host == *suffix || host.ends_with(&format!(".{suffix}")) {
                let score = suffix.len();
                if best.as_ref().is_none_or(|(s, _)| score > *s) {
                    best = Some((score, entry));
                }
            }
        }
        if let Some((_, entry)) = best {
            return path_allowed(entry, &host, &url_path);
        }

        tracing::debug!(
            domain = %host,
            allowed_count = self.entries.len() + self.suffix_entries.len(),
            "web_fetch domain not in allowlist"
        );
        Some(WebFetchOutput::DomainNotAllowed(host))
    }
}

fn merge_exact(entries: &mut HashMap<String, HostEntry>, host: String, entry: HostEntry) {
    match entry {
        HostEntry::AnyPath => {
            entries.insert(host, HostEntry::AnyPath);
        }
        HostEntry::PathPrefixes(prefixes) => {
            if matches!(entries.get(&host), Some(HostEntry::AnyPath)) {
                return;
            }
            entries
                .entry(host)
                .and_modify(|e| {
                    if let HostEntry::PathPrefixes(v) = e {
                        for p in &prefixes {
                            if !v.contains(p) {
                                v.push(p.clone());
                            }
                        }
                    }
                })
                .or_insert(HostEntry::PathPrefixes(prefixes));
        }
    }
}

fn merge_suffix(suffix_entries: &mut Vec<(String, HostEntry)>, suffix: String, entry: HostEntry) {
    if let Some((_, existing)) = suffix_entries.iter_mut().find(|(s, _)| *s == suffix) {
        match (existing, entry) {
            (HostEntry::AnyPath, _) => {}
            (existing, HostEntry::AnyPath) => *existing = HostEntry::AnyPath,
            (HostEntry::PathPrefixes(v), HostEntry::PathPrefixes(prefixes)) => {
                for p in prefixes {
                    if !v.contains(&p) {
                        v.push(p);
                    }
                }
            }
        }
    } else {
        suffix_entries.push((suffix, entry));
    }
}

/// Collapse `.` / `..` and lowercase for allowlist path comparison.
fn normalize_url_path(path: &str) -> String {
    let decoded = percent_decode_lite(path).to_ascii_lowercase();
    let mut out: Vec<&str> = Vec::new();
    for seg in decoded.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    format!("/{}", out.join("/"))
}

/// Minimal %XX decode for path segments (best-effort; invalid stays as-is).
fn percent_decode_lite(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            let hi = hex_nibble(bytes[i + 1]);
            let lo = hex_nibble(bytes[i + 2]);
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_nibble(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

fn path_allowed(entry: &HostEntry, host: &str, url_path: &str) -> Option<WebFetchOutput> {
    match entry {
        HostEntry::AnyPath => None,
        HostEntry::PathPrefixes(prefixes) => {
            if prefixes.iter().any(|prefix| {
                url_path == *prefix
                    || (url_path.starts_with(prefix.as_str())
                        && url_path.as_bytes().get(prefix.len()) == Some(&b'/'))
            }) {
                return None;
            }
            tracing::debug!(
                domain = %host,
                path = %url_path,
                allowed_prefixes = ?prefixes,
                "web_fetch path not in allowlist for domain"
            );
            Some(WebFetchOutput::DomainNotAllowed(host.to_string()))
        }
    }
}

/// Extract and normalize the domain from a raw URL string.
///
/// Returns `None` for unparseable URLs or URLs with no host.
pub fn domain_from_url(raw_url: &str) -> Option<String> {
    Url::parse(raw_url)
        .ok()
        .and_then(|u| u.host_str().map(normalize_domain))
}

// ───────────────────────────────────────────────────────────────────────────
// Tests
// ───────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    // ── normalize_domain ─────────────────────────────────────────────────

    #[test]
    fn normalize_strips_www_and_trailing_dot() {
        assert_eq!(normalize_domain("www.Example.COM."), "example.com");
    }

    #[test]
    fn normalize_trims_whitespace() {
        assert_eq!(normalize_domain("  docs.rs  "), "docs.rs");
    }

    // ── Host-only entries ────────────────────────────────────────────────

    #[test]
    fn allows_listed_domain() {
        let m = DomainMatcher::new(&["docs.rs".into(), "Example.Com".into()]);
        assert!(m.check(&url("https://docs.rs/reqwest/latest")).is_none());
        assert!(m.check(&url("https://example.com/page")).is_none());
    }

    #[test]
    fn wildcard_suffix_matches_subdomains() {
        let m = DomainMatcher::new(&["*.example.com".into()]);
        assert!(m.check(&url("https://example.com/")).is_none());
        assert!(m.check(&url("https://api.example.com/v1")).is_none());
        assert!(m.check(&url("https://a.b.example.com/")).is_none());
        assert!(m.check(&url("https://evil.com/")).is_some());
        assert!(m.check(&url("https://example.com.evil.com/")).is_some());
    }

    #[test]
    fn path_dot_segments_cannot_escape_prefix() {
        let m = DomainMatcher::new(&["example.com/docs".into()]);
        assert!(m.check(&url("https://example.com/docs/guide")).is_none());
        assert!(
            m.check(&url("https://example.com/docs/../admin")).is_some(),
            "dot-segment escape should be blocked after normalize"
        );
    }

    #[test]
    fn www_prefix_case_insensitive_in_allowlist_entry() {
        let m = DomainMatcher::new(&["WWW.Example.COM".into()]);
        assert!(m.check(&url("https://example.com/page")).is_none());
    }

    #[test]
    fn most_specific_wildcard_wins() {
        let m = DomainMatcher::new(&["*.com".into(), "*.example.com/docs".into()]);
        assert!(m.check(&url("https://api.example.com/docs/x")).is_none());
        assert!(m.check(&url("https://api.example.com/api")).is_some());
    }

    #[test]
    fn wildcard_with_path_prefix() {
        let m = DomainMatcher::new(&["*.vercel.com/docs".into()]);
        assert!(m.check(&url("https://docs.vercel.com/docs/x")).is_none());
        assert!(m.check(&url("https://docs.vercel.com/api")).is_some());
    }

    #[test]
    fn case_insensitive_host() {
        let m = DomainMatcher::new(&["Example.Com".into()]);
        assert!(m.check(&url("https://example.com/page")).is_none());
        assert!(m.check(&url("https://EXAMPLE.COM/page")).is_none());
    }

    #[test]
    fn rejects_unlisted_domain() {
        let m = DomainMatcher::new(&["docs.rs".into()]);
        let blocked = m.check(&url("https://evil.com/steal"));
        assert!(matches!(blocked, Some(WebFetchOutput::DomainNotAllowed(d)) if d == "evil.com"));
    }

    #[test]
    fn blocks_all_when_empty() {
        let m = DomainMatcher::new(&[]);
        assert!(m.check(&url("https://docs.python.org/3/")).is_some());
        assert!(m.check(&url("https://example.com/")).is_some());
    }

    #[test]
    fn www_prefix_stripped() {
        let m = DomainMatcher::new(&["react.dev".into()]);
        assert!(m.check(&url("https://www.react.dev/learn")).is_none());
    }

    #[test]
    fn trailing_dot_stripped() {
        let m = DomainMatcher::new(&["react.dev".into()]);
        assert!(m.check(&url("https://react.dev./learn")).is_none());
    }

    // ── Path-scoped entries ──────────────────────────────────────────────

    #[test]
    fn path_scoped_allows_matching_path() {
        let m = DomainMatcher::new(&["vercel.com/docs".into()]);
        assert!(m.check(&url("https://vercel.com/docs")).is_none());
        assert!(m.check(&url("https://vercel.com/docs/foo")).is_none());
        assert!(
            m.check(&url("https://vercel.com/docs/platform/edge-functions"))
                .is_none()
        );
    }

    #[test]
    fn path_scoped_blocks_non_matching_path() {
        let m = DomainMatcher::new(&["vercel.com/docs".into()]);
        let blocked = m.check(&url("https://vercel.com/api"));
        assert!(matches!(blocked, Some(WebFetchOutput::DomainNotAllowed(h)) if h == "vercel.com"));
        assert!(m.check(&url("https://vercel.com/")).is_some());
    }

    #[test]
    fn path_scoped_blocks_wrong_host() {
        let m = DomainMatcher::new(&["vercel.com/docs".into()]);
        let blocked = m.check(&url("https://netlify.com/docs"));
        assert!(matches!(blocked, Some(WebFetchOutput::DomainNotAllowed(h)) if h == "netlify.com"));
    }

    #[test]
    fn path_scoped_rejects_sibling_prefix() {
        let m = DomainMatcher::new(&["vercel.com/docs".into()]);

        // "/docs-internal" shares "/docs" prefix but is a sibling, not a child.
        let blocked = m.check(&url("https://vercel.com/docs-internal"));
        assert!(
            matches!(blocked, Some(WebFetchOutput::DomainNotAllowed(h)) if h == "vercel.com"),
            "expected /docs-internal to be blocked"
        );

        // "/documentation" — also a sibling.
        let blocked2 = m.check(&url("https://vercel.com/documentation"));
        assert!(
            matches!(blocked2, Some(WebFetchOutput::DomainNotAllowed(h)) if h == "vercel.com"),
            "expected /documentation to be blocked"
        );

        // Actual child path is still allowed.
        assert!(
            m.check(&url("https://vercel.com/docs/guide")).is_none(),
            "expected /docs/guide to be allowed"
        );
    }

    #[test]
    fn path_scoped_case_insensitive() {
        let m = DomainMatcher::new(&["Vercel.COM/Docs".into()]);
        assert!(m.check(&url("https://vercel.com/docs")).is_none());
        assert!(m.check(&url("https://VERCEL.COM/DOCS/foo")).is_none());
    }

    #[test]
    fn root_path_entry_allows_any_path() {
        // "example.com/" should behave like host-only.
        let m = DomainMatcher::new(&["example.com/".into()]);
        assert!(m.check(&url("https://example.com/")).is_none());
        assert!(m.check(&url("https://example.com/anything")).is_none());
    }

    // ── Multiple path prefixes per host ──────────────────────────────────

    #[test]
    fn multiple_path_prefixes_per_host() {
        let m = DomainMatcher::new(&["github.com/org-a".into(), "github.com/org-b".into()]);
        assert!(
            m.check(&url("https://github.com/org-a/project-one"))
                .is_none()
        );
        assert!(
            m.check(&url("https://github.com/org-b/project-two"))
                .is_none()
        );
        assert!(
            m.check(&url("https://github.com/evil-org/malware"))
                .is_some()
        );
    }

    #[test]
    fn host_only_overrides_path_prefixes() {
        // If both "github.com" (host-only) and "github.com/docs" exist,
        // host-only wins — any path is allowed.
        let m = DomainMatcher::new(&["github.com/docs".into(), "github.com".into()]);
        assert!(m.check(&url("https://github.com/anything")).is_none());
    }

    // ── Model URL variants ───────────────────────────────────────────────

    #[test]
    fn model_url_variants() {
        let m = DomainMatcher::new(&[
            "docs.python.org".into(),
            "developer.mozilla.org".into(),
            "react.dev".into(),
            "api.example.com".into(),
        ]);

        // Should match.
        assert!(m.check(&url("https://react.dev")).is_none());
        assert!(
            m.check(&url("https://docs.python.org/3/library/asyncio.html"))
                .is_none()
        );
        assert!(
            m.check(&url(
                "https://developer.mozilla.org/en-US/docs/Web/API/fetch?v=2#syntax"
            ))
            .is_none()
        );
        assert!(m.check(&url("https://react.dev/")).is_none());
        assert!(m.check(&url("https://React.Dev/learn")).is_none());
        assert!(m.check(&url("https://api.example.com/v1/users")).is_none());
        assert!(m.check(&url("https://react.dev:443/reference")).is_none());
        assert!(m.check(&url("https://www.react.dev/learn")).is_none());

        // Should NOT match.
        assert!(m.check(&url("https://example.com/page")).is_some());
        assert!(m.check(&url("https://evil.example.com/")).is_some());
        assert!(m.check(&url("https://react-dev.com/")).is_some());
        assert!(
            m.check(&url("https://stackoverflow.com/questions/123"))
                .is_some()
        );
        assert!(m.check(&url("https://93.184.216.34/page")).is_some());
    }

    // ── domain_from_url ─────────────────────────────────────────────────

    #[test]
    fn domain_from_url_extracts_and_normalizes() {
        assert_eq!(
            domain_from_url("https://docs.python.org/3/library/asyncio.html"),
            Some("docs.python.org".to_string())
        );
        assert_eq!(
            domain_from_url("https://www.React.Dev/learn"),
            Some("react.dev".to_string())
        );
    }

    #[test]
    fn domain_from_url_returns_none_for_garbage() {
        assert_eq!(domain_from_url("not a url"), None);
        assert_eq!(domain_from_url(""), None);
    }
}
