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
}
