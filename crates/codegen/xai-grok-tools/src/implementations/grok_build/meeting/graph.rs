//! Optional Microsoft Graph meeting-chat (delegated token = post as the operator).

use serde_json::Value;

pub const GRAPH_TOKEN_ENV: &str = "GROK_GRAPH_TOKEN";
const GRAPH: &str = "https://graph.microsoft.com/v1.0";

pub fn graph_token() -> Option<String> {
    std::env::var(GRAPH_TOKEN_ENV)
        .ok()
        .or_else(|| std::env::var("GROK_MEETING_GRAPH_TOKEN").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

async fn graph_get(token: &str, path_and_query: &str) -> Result<Value, String> {
    let url = format!("{GRAPH}{path_and_query}");
    let resp = reqwest::Client::new()
        .get(&url)
        .bearer_auth(token)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("Graph GET {status}: {}", truncate_graph_body(&text)));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

async fn first_online_meeting(token: &str, join_url: &str) -> Result<Value, String> {
    let literal = join_url.replace('\'', "''");
    let filter = format!("JoinWebUrl eq '{literal}'");
    let path = format!("/me/onlineMeetings?$filter={}", percent_encode_query(&filter));
    let v = graph_get(token, &path).await?;
    let arr = v
        .get("value")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    let matched = arr.iter().find(|m| {
        m.get("joinWebUrl")
            .and_then(|x| x.as_str())
            .is_some_and(|got| join_urls_match(join_url, got))
    });
    if let Some(m) = matched {
        return Ok(m.clone());
    }
    if arr.len() == 1 && arr[0].get("joinWebUrl").and_then(|x| x.as_str()).is_none() {
        return Ok(arr[0].clone());
    }
    Err(
        "Graph found no onlineMeeting for this join URL (need OnlineMeetings.Read on the token)"
            .to_string(),
    )
}

/// Resolve the meeting chat thread for a join URL (`chatInfo.threadId`).
pub async fn chat_id_for_join_url(token: &str, join_url: &str) -> Result<String, String> {
    let first = first_online_meeting(token, join_url).await?;
    first
        .pointer("/chatInfo/threadId")
        .and_then(|x| x.as_str())
        .map(str::to_string)
        .ok_or_else(|| "onlineMeeting has no chatInfo.threadId".to_string())
}

/// Calendar/Teams subject for a join URL, when Graph can see the onlineMeeting.
pub async fn meeting_subject_for_join_url(token: &str, join_url: &str) -> Result<String, String> {
    let first = first_online_meeting(token, join_url).await?;
    first
        .get("subject")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "onlineMeeting has no subject".to_string())
}

pub async fn recent_messages(token: &str, chat_id: &str) -> Result<Vec<(String, String, String)>, String> {
    let path = format!(
        "/chats/{}/messages?$top=25&$orderby=createdDateTime desc",
        percent_encode_path_segment(chat_id)
    );
    let v = graph_get(token, &path).await?;
    let mut out = Vec::new();
    if let Some(arr) = v.get("value").and_then(|x| x.as_array()) {
        for m in arr {
            let id = m.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let from = m
                .pointer("/from/user/displayName")
                .or_else(|| m.pointer("/from/application/displayName"))
                .and_then(|x| x.as_str())
                .unwrap_or("unknown")
                .to_string();
            let body = m
                .pointer("/body/content")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let plain = strip_simple_html(&body);
            if !plain.is_empty() {
                out.push((id, from, plain));
            }
        }
    }
    Ok(out)
}

pub async fn post_chat(token: &str, chat_id: &str, text: &str) -> Result<(), String> {
    let url = format!("{GRAPH}/chats/{}/messages", percent_encode_path_segment(chat_id));
    let body = serde_json::json!({
        "body": { "contentType": "text", "content": text }
    });
    let resp = reqwest::Client::new()
        .post(&url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        let t = resp.text().await.unwrap_or_default();
        return Err(format!("Graph POST {status}: {}", truncate_graph_body(&t)));
    }
    Ok(())
}

fn percent_encode_query(s: &str) -> String {
    percent_encode(s.as_bytes(), false)
}

fn percent_encode_path_segment(s: &str) -> String {
    percent_encode(s.as_bytes(), true)
}

fn percent_encode(bytes: &[u8], path_segment: bool) -> String {
    let mut out = String::new();
    for &b in bytes {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b':' | b'@' if path_segment => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn truncate_graph_body(text: &str) -> String {
    let t = text.trim();
    if t.chars().count() <= 180 {
        t.to_string()
    } else {
        format!("{}…", t.chars().take(180).collect::<String>())
    }
}

fn join_urls_match(expected: &str, got: &str) -> bool {
    fn norm(s: &str) -> Option<String> {
        let u = url::Url::parse(s).ok()?;
        let host = u.host_str()?.to_ascii_lowercase();
        let path = u.path().trim_end_matches('/').to_string();
        Some(format!("{host}{path}"))
    }
    match (norm(expected), norm(got)) {
        (Some(a), Some(b)) => a == b,
        _ => expected.trim() == got.trim(),
    }
}

fn strip_simple_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    html_unescape(out.trim())
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("<br>", "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_encode_stops_ampersand_breakout() {
        let filter = "JoinWebUrl eq 'https://zoom.us/j/1?pwd=x&$top=1'";
        let enc = percent_encode_query(filter);
        assert!(!enc.contains('&'), "{enc}");
        assert!(enc.contains("%26"));
        assert!(enc.contains("%27"));
    }

    #[test]
    fn join_url_match_ignores_trailing_slash() {
        assert!(join_urls_match(
            "https://teams.microsoft.com/l/meetup-join/abc",
            "https://teams.microsoft.com/l/meetup-join/abc/"
        ));
        assert!(!join_urls_match(
            "https://teams.microsoft.com/l/meetup-join/abc",
            "https://teams.microsoft.com/l/meetup-join/other"
        ));
    }

    #[test]
    fn truncates_graph_errors() {
        let long = "e".repeat(400);
        let t = truncate_graph_body(&long);
        assert!(t.chars().count() <= 181);
    }
}
