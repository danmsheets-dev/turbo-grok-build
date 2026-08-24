//! Poll Graph chat + local inbox.jsonl for `Turbo:` questions.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use xai_grok_meetings::{MeetingStore, extract_turbo_question};

use crate::notification::types::ToolNotificationHandle;

use super::auto_ask;
use super::graph;

pub async fn run_watch(
    store: MeetingStore,
    join_url: String,
    stop: Arc<AtomicBool>,
    notification: Option<ToolNotificationHandle>,
) {
    let mut seen: HashSet<String> = HashSet::new();
    while !stop.load(Ordering::Relaxed) {
        drain_inbox(&store, &mut seen, notification.as_ref());
        if let Some(token) = graph::graph_token() {
            drain_graph(&store, &join_url, &token, &mut seen, notification.as_ref()).await;
        }
        tokio::time::sleep(Duration::from_secs(4)).await;
    }
}

fn drain_inbox(
    store: &MeetingStore,
    seen: &mut HashSet<String>,
    notification: Option<&ToolNotificationHandle>,
) {
    let path = store.inbox_path();
    let Ok(f) = fs::File::open(path) else {
        return;
    };
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (from, text) = if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            let from = v
                .get("from")
                .and_then(|x| x.as_str())
                .unwrap_or("chat")
                .to_string();
            let text = v
                .get("text")
                .or_else(|| v.get("question"))
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            (from, text)
        } else {
            ("inbox".into(), line.to_string())
        };
        consider(store, seen, &from, &text, notification);
    }
}

async fn drain_graph(
    store: &MeetingStore,
    join_url: &str,
    token: &str,
    seen: &mut HashSet<String>,
    notification: Option<&ToolNotificationHandle>,
) {
    let Ok(chat_id) = graph::chat_id_for_join_url(token, join_url).await else {
        return;
    };
    let Ok(msgs) = graph::recent_messages(token, &chat_id).await else {
        return;
    };
    for (id, from, text) in msgs {
        if !seen.insert(format!("graph:{id}")) {
            continue;
        }
        consider(store, seen, &from, &text, notification);
    }
}

fn consider(
    store: &MeetingStore,
    seen: &mut HashSet<String>,
    from: &str,
    text: &str,
    notification: Option<&ToolNotificationHandle>,
) {
    let Some(q) = extract_turbo_question(text) else {
        return;
    };
    let key = format!("{from}|{q}");
    if !seen.insert(key) {
        return;
    }
    let _ = store.enqueue_question(from, &q);
    if auto_ask::emit_auto_ask(notification, from, &q) {
        let _ = store.mark_question_answered(from, &q);
    }
}

/// Append a line to `inbox.jsonl`.
///
/// This is the ingress seam every chat source shares: the joined notetaker
/// writes scraped Teams chat here, and `drain_inbox` turns it into `Turbo:`
/// questions. Also used for tests and manual paste.
pub fn append_inbox(store: &MeetingStore, from: &str, text: &str) -> std::io::Result<()> {
    let rec = serde_json::json!({ "from": from, "text": text });
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(store.inbox_path())?;
    use std::io::Write;
    serde_json::to_writer(&mut f, &rec).map_err(std::io::Error::other)?;
    f.write_all(b"\n")?;
    Ok(())
}
