//! Parse Zoom / Teams / Meet / Webex / webinar join URLs.

use serde::{Deserialize, Serialize};

/// Video-conference platform inferred from the join URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingPlatform {
    Teams,
    Zoom,
    GoogleMeet,
    Webex,
    Other,
}

impl MeetingPlatform {
    /// Short label for TUI / tool output.
    pub fn label(self) -> &'static str {
        match self {
            Self::Teams => "Teams",
            Self::Zoom => "Zoom",
            Self::GoogleMeet => "Google Meet",
            Self::Webex => "Webex",
            Self::Other => "Meeting",
        }
    }
}

/// Meeting vs webinar (webinars often block attendee chat).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingKind {
    Meeting,
    Webinar,
}

/// A user-supplied join URL after validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingUrl {
    /// Canonical https URL (trimmed).
    pub raw: String,
    pub platform: MeetingPlatform,
    pub kind: MeetingKind,
}

/// Why a join URL was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("meeting URL is empty")]
    Empty,
    #[error("not an https meeting URL: {0}")]
    NotHttp(String),
    #[error("http:// join URLs are not opened; use https: {0}")]
    NotHttps(String),
    #[error("refusing non-http(s) scheme: {0}")]
    BadScheme(String),
}

/// Parse a paste-in Zoom/Teams/Meet/Webex join URL.
pub fn parse(input: &str) -> Result<MeetingUrl, ParseError> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(ParseError::Empty);
    }
    if raw.bytes().any(|b| b < 0x20 || b == 0x7f || b == b'"') {
        return Err(ParseError::NotHttp(raw.to_string()));
    }
    let parsed = url::Url::parse(raw).map_err(|_| ParseError::NotHttp(raw.to_string()))?;
    match parsed.scheme() {
        "https" => {}
        "http" => return Err(ParseError::NotHttps(raw.to_string())),
        other => {
            let _ = other;
            return Err(ParseError::BadScheme(raw.to_string()));
        }
    }
    if parsed.host_str().is_none() {
        return Err(ParseError::NotHttp(raw.to_string()));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ParseError::NotHttp(raw.to_string()));
    }
    let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
    if !host_is_safe(&host) || !authority_matches(raw, &parsed) {
        return Err(ParseError::NotHttp(raw.to_string()));
    }
    let path = parsed.path().to_ascii_lowercase();
    let query = parsed.query().unwrap_or("").to_ascii_lowercase();
    let (platform, kind) = classify_host(&host, &path, &query);
    Ok(MeetingUrl {
        raw: parsed.to_string(),
        platform,
        kind,
    })
}

fn authority_matches(raw: &str, parsed: &url::Url) -> bool {
    let Some((_, rest)) = raw.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let expected = match parsed.port() {
        Some(p) => format!("{}:{p}", parsed.host_str().unwrap_or("")),
        None => parsed.host_str().unwrap_or("").to_string(),
    };
    authority.eq_ignore_ascii_case(&expected)
}

fn host_is_safe(host: &str) -> bool {
    if host.starts_with('[') && host.ends_with(']') {
        return host.len() > 2;
    }
    !host.is_empty()
        && host.contains('.')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

fn host_is(host: &str, exact: &str) -> bool {
    host == exact || host.ends_with(&format!(".{exact}"))
}

fn classify_host(host: &str, path: &str, query: &str) -> (MeetingPlatform, MeetingKind) {
    if host_is(host, "zoom.us") || host_is(host, "zoom.com") {
        let webinar = path.contains("/w/")
            || path.contains("/webinar")
            || query.contains("role=attendee")
            || path.contains("/rec/share");
        return (
            MeetingPlatform::Zoom,
            if webinar {
                MeetingKind::Webinar
            } else {
                MeetingKind::Meeting
            },
        );
    }
    if host_is(host, "teams.microsoft.com")
        || host_is(host, "teams.live.com")
        || host_is(host, "teams.office.com")
    {
        let webinar = path.contains("webinar")
            || path.contains("townhall")
            || query.contains("webinar")
            || query.contains("townhall");
        return (
            MeetingPlatform::Teams,
            if webinar {
                MeetingKind::Webinar
            } else {
                MeetingKind::Meeting
            },
        );
    }
    if host_is(host, "meet.google.com") {
        return (MeetingPlatform::GoogleMeet, MeetingKind::Meeting);
    }
    if host_is(host, "webex.com") {
        return (MeetingPlatform::Webex, MeetingKind::Meeting);
    }
    (MeetingPlatform::Other, MeetingKind::Meeting)
}

/// True when this is a real conferencing platform (not a random https link).
pub fn is_joinable_platform(platform: MeetingPlatform) -> bool {
    !matches!(platform, MeetingPlatform::Other)
}

/// First https URL token in `text` (stops at whitespace).
///
/// Walks *character* starts, not byte offsets: this runs on every prompt the
/// operator submits (`detect_join_request`), and slicing at a byte index inside
/// a multi-byte character panics — which `panic = "abort"` turns into a hard
/// process death for any paste containing a smart quote or an emoji.
pub fn first_https_url(text: &str) -> Option<&str> {
    for (i, _) in text.char_indices() {
        if i + 8 > text.len() {
            break;
        }
        let rest = &text[i..];
        let lower_prefix = rest.get(..8).unwrap_or("");
        if lower_prefix.eq_ignore_ascii_case("https://") {
            let end = rest
                .find(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == '"' || c == '\'')
                .unwrap_or(rest.len());
            let url = rest[..end].trim_end_matches(['.', ',', ';', ')', ']']);
            if url.len() > 8 {
                return Some(url);
            }
        }
    }
    None
}

/// Strip join-link secrets (`p`, `pwd`, `passcode`, `password`) from a URL.
///
/// Keeps host + path (and non-secret query) so Graph lookup can still match
/// on `JoinWebUrl` host/path. Never used as a substitute for not storing
/// the raw URL in memory during `meeting_join`.
pub fn redact_join_secrets(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url.trim()) else {
        return url.to_string();
    };
    const SECRETS: &[&str] = &["p", "pwd", "passcode", "password"];
    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(k, _)| !SECRETS.iter().any(|s| k.eq_ignore_ascii_case(s)))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if kept.is_empty() {
        parsed.set_query(None);
    } else {
        parsed.query_pairs_mut().clear();
        {
            let mut pairs = parsed.query_pairs_mut();
            for (k, v) in &kept {
                pairs.append_pair(k, v);
            }
        }
    }
    parsed.to_string()
}

/// Env kill-switch for [`teams_web_join_url`]. `0`/`false`/`off`/`no` disables
/// the rewrite, and the notetaker navigates exactly what the operator pasted.
pub const TEAMS_WEB_ENV: &str = "GROK_MEETING_TEAMS_WEB";

/// True unless the operator turned the web-join rewrite off.
pub fn teams_web_rewrite_enabled() -> bool {
    match std::env::var(TEAMS_WEB_ENV) {
        Ok(s) => !teams_web_env_disables(&s),
        Err(_) => true,
    }
}

/// Shared by [`teams_web_rewrite_enabled`] and its test, so the accepted
/// spellings cannot drift apart.
fn teams_web_env_disables(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "off" | "no"
    )
}

/// Query parameters that keep a Teams join on the anonymous **web** client.
///
/// Teams' own redirect to `/dl/launcher/launcher.html` carries
/// `msLaunch=true&directDl=true&suppressPrompt=true`, which fires the
/// `ms-teams:` protocol immediately and never renders "Continue on this
/// browser" - leaving the guest notetaker with no DOM to drive. Negating them
/// asks for the web client instead.
///
/// These names come from an observed redirect chain, not from documentation.
/// This is one layer of a layered defence, never the fix on its own.
const WEB_JOIN_PARAMS: &[(&str, &str)] = &[
    ("anon", "true"),
    ("msLaunch", "false"),
    ("directDl", "false"),
    ("enableMobilePage", "false"),
    ("suppressPrompt", "false"),
];

/// Rewrite a Teams join URL to ask for the anonymous web client.
///
/// **Query-only.** Scheme, host, port and path are preserved: `join_urls_match`
/// normalises to host+path and drops the query, so a path change would break
/// Graph subject lookup. Every original parameter is carried through, including
/// the `p` passcode, without which the link does not join at all.
///
/// Returns `None` for anything not recognised, so the caller navigates the
/// operator's URL unchanged. Never call this from [`parse`]: the raw URL is what
/// the meeting store, Graph, the watcher and the join summary all use.
pub fn teams_web_join_url(u: &MeetingUrl) -> Option<String> {
    if u.platform != MeetingPlatform::Teams || u.kind != MeetingKind::Meeting {
        return None;
    }
    let parsed = url::Url::parse(&u.raw).ok()?;
    // A hash-routed link (`/_#/l/meetup-join/...`) keeps the meeting id in the
    // fragment, which the server never sees, so a query parameter cannot steer
    // it. Leave it alone rather than pretend.
    if parsed.fragment().is_some() {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    let path = parsed.path();
    let recognised = if host_is(&host, "teams.microsoft.com") || host_is(&host, "teams.office.com") {
        path.starts_with("/l/meetup-join/") || is_short_meet_path(path)
    } else if host_is(&host, "teams.live.com") {
        // Consumer Teams, in its own arm so it can be disabled separately.
        is_short_meet_path(path)
    } else {
        false
    };
    if !recognised {
        return None;
    }

    let carried: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(k, _)| {
            !WEB_JOIN_PARAMS
                .iter()
                .any(|(name, _)| k.eq_ignore_ascii_case(name))
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    let mut out = parsed.clone();
    out.set_query(None);
    {
        let mut pairs = out.query_pairs_mut();
        // Operator parameters first, so the `p` passcode can never be crowded
        // out by a rewrite that grows later.
        for (k, v) in &carried {
            pairs.append_pair(k, v);
        }
        for (k, v) in WEB_JOIN_PARAMS {
            pairs.append_pair(k, v);
        }
    }
    Some(out.to_string())
}

/// `/meet/<id>` - the short join link Teams hands out today.
fn is_short_meet_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/meet/") else {
        return false;
    };
    let id = rest.trim_end_matches('/');
    !id.is_empty() && !id.contains('/')
}

fn has_join_intent(text: &str) -> bool {
    let l = text.to_ascii_lowercase();
    const PHRASES: &[&str] = &[
        "note taker",
        "take notes",
        "taking notes",
        "record this",
        "record the",
        "sit in",
        "drop in",
        "hop in",
        "listen in",
        "listen to",
        "meeting link",
        "link to test",
        "q&a",
        "q and a",
    ];
    if PHRASES.iter().any(|p| l.contains(p)) {
        return true;
    }
    // Word match so "enjoy" does not count as "join".
    const WORDS: &[&str] = &["join", "listen", "notetaker", "capture"];
    l.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| WORDS.contains(&w))
}

/// Detect a natural-language (or URL-only) request to join a meeting.
///
/// Returns `(url, optional_title)`. A bare Teams/Zoom/Meet/Webex https URL
/// counts. A longer message counts only with join/listen/notes intent so a
/// pasted ticket link does not start capture.
pub fn detect_join_request(text: &str) -> Option<(String, Option<String>)> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('/') {
        return None;
    }
    let url = first_https_url(trimmed)?;
    let parsed = parse(url).ok()?;
    if !is_joinable_platform(parsed.platform) {
        return None;
    }
    let remainder = trimmed.replacen(url, "", 1);
    let remainder = remainder.trim();
    let url_only = remainder.is_empty();
    if !url_only && !has_join_intent(trimmed) {
        return None;
    }
    let title = remainder
        .trim_matches(|c: char| c == ':' || c == '-' || c == ',')
        .trim();
    let title = if title.is_empty() || has_join_intent(title) && title.split_whitespace().count() <= 6
    {
        None
    } else if title.chars().count() > 80 {
        None
    } else {
        Some(title.to_string())
    };
    Some((parsed.raw, title))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_teams_meetup_join() {
        let u = parse("https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc").unwrap();
        assert_eq!(u.platform, MeetingPlatform::Teams);
        assert_eq!(u.kind, MeetingKind::Meeting);
    }

    #[test]
    fn parses_zoom_meeting() {
        let u = parse("https://us02web.zoom.us/j/123456789?pwd=secret").unwrap();
        assert_eq!(u.platform, MeetingPlatform::Zoom);
        assert_eq!(u.kind, MeetingKind::Meeting);
    }

    #[test]
    fn parses_zoom_webinar() {
        let u = parse("https://zoom.us/w/81111111111").unwrap();
        assert_eq!(u.kind, MeetingKind::Webinar);
    }

    #[test]
    fn parses_google_meet() {
        let u = parse("https://meet.google.com/abc-defg-hij").unwrap();
        assert_eq!(u.platform, MeetingPlatform::GoogleMeet);
    }

    #[test]
    fn rejects_empty_and_javascript() {
        assert!(matches!(parse("  "), Err(ParseError::Empty)));
        assert!(matches!(
            parse("javascript:alert(1)"),
            Err(ParseError::BadScheme(_))
        ));
        assert!(matches!(parse("msteams:join"), Err(ParseError::BadScheme(_))));
        assert!(matches!(
            parse("http://teams.microsoft.com/l/meetup-join/x"),
            Err(ParseError::NotHttps(_))
        ));
        assert!(parse("https://x.com&calc.exe").is_err());
        assert!(parse("https://example.com/\r\ncalc").is_err());
    }

    #[test]
    fn classifies_from_host_not_path() {
        let u = parse("https://evil.example/zoom.us/j/1").unwrap();
        assert_eq!(u.platform, MeetingPlatform::Other);
        let z = parse("https://us02web.zoom.us/j/1").unwrap();
        assert_eq!(z.platform, MeetingPlatform::Zoom);
    }

    #[test]
    fn parses_teams_short_meet_id() {
        let u = parse("https://teams.microsoft.com/meet/2907709513066?p=abc").unwrap();
        assert_eq!(u.platform, MeetingPlatform::Teams);
        assert_eq!(u.kind, MeetingKind::Meeting);
    }

    /// `first_https_url` walks byte indices. Slicing on one that lands inside a
    /// multi-byte character panics, and `panic = "abort"` in `[profile.dev]` and
    /// `[profile.release]` makes that a hard process death. This runs on every
    /// prompt submit via `detect_join_request`, so one smart quote is enough.
    #[test]
    fn non_ascii_text_does_not_panic_the_scanner() {
        // No URL anywhere; the crash needs only a multi-byte char past byte 8.
        assert_eq!(first_https_url("the operator said \u{201c}it broke\u{201d} again"), None);
        assert_eq!(first_https_url("\u{1f600}\u{1f600}\u{1f600}\u{1f600}"), None);
        assert_eq!(first_https_url("caf\u{e9} \u{2014} a long enough line"), None);
        // A real URL is still found when it follows multi-byte text.
        assert_eq!(
            first_https_url("notes \u{2014} https://teams.microsoft.com/meet/1?p=x here"),
            Some("https://teams.microsoft.com/meet/1?p=x")
        );
        // A multi-byte char is not a token terminator, so it stays in the match;
        // what matters here is that the token is not split mid-character.
        assert_eq!(
            first_https_url("\u{201c}https://example.com/a\u{201d}"),
            Some("https://example.com/a\u{201d}")
        );
    }

    #[test]
    fn detect_join_request_survives_a_non_ascii_paste() {
        // The pager calls this on every ordinary prompt submit.
        assert!(detect_join_request("here\u{2019}s the plan \u{2014} ship it").is_none());
    }

    #[test]
    fn detect_join_bare_teams_url() {
        let (url, title) =
            detect_join_request("https://teams.microsoft.com/meet/2907709513066?p=abc").unwrap();
        assert!(url.contains("2907709513066"));
        assert!(title.is_none());
    }

    #[test]
    fn detect_join_natural_language() {
        let (url, _) = detect_join_request(
            "Join this meeting and take notes: https://teams.microsoft.com/meet/1?p=x",
        )
        .unwrap();
        assert!(url.contains("teams.microsoft.com"));
        assert!(detect_join_request(
            "see the recording later at https://teams.microsoft.com/meet/1?p=x in the ticket"
        )
        .is_none());
        assert!(detect_join_request("/meeting join https://teams.microsoft.com/meet/1").is_none());
        assert!(
            detect_join_request(
                "enjoy this writeup https://teams.microsoft.com/meet/1?p=x in the ticket"
            )
            .is_none(),
            "substring 'join' inside 'enjoy' must not start capture"
        );
    }

    #[test]
    fn detect_join_operator_meeting_link_and_qa_phrasing() {
        let prompt = "Here is a meeting link to test with: https://teams.microsoft.com/meet/2907709513066?p=abc\nRun a full round of Q&A on Meetings.";
        let (url, _) = detect_join_request(prompt).expect("operator phrasing must join");
        assert!(url.contains("2907709513066"), "{url}");
        let qa = detect_join_request(
            "Run a full round of Q&A on Meetings https://teams.microsoft.com/meet/1?p=x",
        );
        assert!(qa.is_some(), "Q&A on Meetings + Teams URL must join");
        assert!(
            detect_join_request(
                "comment on meeting notes: https://teams.microsoft.com/meet/1?p=x"
            )
            .is_none(),
            "substring 'on meeting' in ticket text must not start capture"
        );
    }

    /// The rewrite must never leak into `parse`. `parse`'s output feeds the
    /// meeting store, Graph lookup, the watcher, the pager's injected
    /// instruction and the operator-facing join summary; only the notetaker's
    /// `Page.navigate` may ever see a rewritten URL.
    ///
    /// Fixture-specific by design: `parse` returns `Url::to_string()`, which
    /// normalises. Asserting equality on two known-normalised fixtures catches a
    /// rewrite creeping into `parse` without inviting a maintainer to relax a
    /// general invariant that was never true.
    #[test]
    fn parse_does_not_rewrite_teams_urls() {
        for raw in [
            "https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc",
            "https://teams.microsoft.com/meet/2907709513066?p=abc",
        ] {
            assert_eq!(parse(raw).unwrap().raw, raw, "parse must not rewrite {raw}");
        }
    }

    #[test]
    fn teams_web_rewrite_preserves_scheme_host_port_path_and_passcode() {
        let u = parse("https://teams.microsoft.com/meet/2907709513066?p=s3cret").unwrap();
        let out = teams_web_join_url(&u).expect("short meet link is rewritable");
        let before = url::Url::parse(&u.raw).unwrap();
        let after = url::Url::parse(&out).unwrap();
        assert_eq!(after.scheme(), before.scheme());
        assert_eq!(after.host_str(), before.host_str());
        assert_eq!(after.port(), before.port());
        assert_eq!(after.path(), before.path(), "path is load-bearing for Graph");
        // The passcode survives, or the link does not join at all.
        assert_eq!(
            after
                .query_pairs()
                .find(|(k, _)| k == "p")
                .map(|(_, v)| v.into_owned()),
            Some("s3cret".to_string())
        );
        for (k, v) in WEB_JOIN_PARAMS {
            assert!(
                after.query_pairs().any(|(ak, av)| ak == *k && av == *v),
                "missing {k}={v} in {out}"
            );
        }
        // meetup-join links rewrite too, keeping their escaped path intact.
        let m = parse("https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc?p=x").unwrap();
        let mo = teams_web_join_url(&m).expect("meetup-join is rewritable");
        assert!(mo.contains("/l/meetup-join/19%3ameeting_abc"), "{mo}");
        assert!(mo.contains("p=x"), "{mo}");
    }

    /// An operator-supplied `msLaunch=true` is exactly the bug, so it loses.
    /// Everything else the operator wrote is carried through untouched.
    #[test]
    fn teams_web_rewrite_overrides_launcher_params_and_keeps_the_rest() {
        let u =
            parse("https://teams.microsoft.com/meet/123?p=pw&msLaunch=true&directDl=true&keep=me")
                .unwrap();
        let out = teams_web_join_url(&u).unwrap();
        let after = url::Url::parse(&out).unwrap();
        let pairs: Vec<(String, String)> = after
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert!(pairs.contains(&("keep".into(), "me".into())), "{out}");
        assert!(pairs.contains(&("p".into(), "pw".into())), "{out}");
        assert_eq!(
            pairs.iter().filter(|(k, _)| k == "msLaunch").count(),
            1,
            "no duplicate msLaunch: {out}"
        );
        assert!(pairs.contains(&("msLaunch".into(), "false".into())), "{out}");
        assert!(pairs.contains(&("directDl".into(), "false".into())), "{out}");
    }

    #[test]
    fn teams_web_rewrite_returns_none_for_non_teams_and_unknown_shapes() {
        for raw in [
            "https://us02web.zoom.us/j/123456789?pwd=x",
            "https://meet.google.com/abc-defg-hij",
            "https://webex.com/meet/x",
            // Hash-routed: the meeting id lives in the fragment, which the
            // server never sees, so a query parameter cannot steer it.
            "https://teams.microsoft.com/_#/l/meetup-join/19%3ameeting_abc",
            // Webinars and town halls are a different join flow.
            "https://teams.microsoft.com/l/webinar/19%3ameeting_abc",
            // Shapes we do not recognise.
            "https://teams.microsoft.com/some/other/path",
            "https://teams.microsoft.com/meet/",
        ] {
            let u = parse(raw).unwrap();
            assert_eq!(teams_web_join_url(&u), None, "must not rewrite {raw}");
        }
    }

    /// `parse` is the security boundary and the rewrite sits behind it: a
    /// `MeetingUrl` cannot be built for a hostile authority, so the rewrite can
    /// never be reached with one.
    #[test]
    fn teams_web_rewrite_still_rejects_hostile_authorities() {
        for hostile in [
            "http://teams.microsoft.com/meet/1",
            "https://teams.microsoft.com&calc.exe/meet/1",
            "https://user:pw@teams.microsoft.com/meet/1",
            "msteams:join",
        ] {
            assert!(parse(hostile).is_err(), "parse must reject {hostile}");
        }
        // A lookalike host parses, but is not Teams, so it is never rewritten.
        let evil = parse("https://evil.example/teams.microsoft.com/meet/1").unwrap();
        assert_eq!(evil.platform, MeetingPlatform::Other);
        assert_eq!(teams_web_join_url(&evil), None);
    }

    #[test]
    fn teams_web_rewrite_is_kill_switchable() {
        assert_eq!(TEAMS_WEB_ENV, "GROK_MEETING_TEAMS_WEB");
        // The env var is process-global, so assert the parser, not the process.
        for off in ["0", "false", "OFF", "no", " 0 "] {
            assert!(teams_web_env_disables(off), "{off} must read as disabled");
        }
        for on in ["1", "true", "", "yes"] {
            assert!(!teams_web_env_disables(on), "{on} must leave it enabled");
        }
    }

    #[test]
    fn redact_join_secrets_strips_passcode_query() {
        let teams = redact_join_secrets("https://teams.microsoft.com/meet/2907709513066?p=secret");
        assert!(!teams.contains("secret"), "{teams}");
        assert!(!teams.contains("p="), "{teams}");
        assert!(teams.contains("teams.microsoft.com/meet/2907709513066"), "{teams}");
        let zoom = redact_join_secrets("https://us02web.zoom.us/j/123456789?pwd=secret&foo=1");
        assert!(!zoom.contains("secret"), "{zoom}");
        assert!(!zoom.to_ascii_lowercase().contains("pwd="), "{zoom}");
        assert!(zoom.contains("foo=1"), "{zoom}");
        let passcode = redact_join_secrets("https://meet.google.com/abc-defg-hij?passcode=s3cret&password=also");
        assert!(!passcode.contains("s3cret"), "{passcode}");
        assert!(!passcode.contains("also"), "{passcode}");
    }
}
