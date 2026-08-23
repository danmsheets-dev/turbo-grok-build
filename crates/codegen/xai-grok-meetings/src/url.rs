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
pub fn first_https_url(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 8 <= bytes.len() {
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
        i += 1;
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
