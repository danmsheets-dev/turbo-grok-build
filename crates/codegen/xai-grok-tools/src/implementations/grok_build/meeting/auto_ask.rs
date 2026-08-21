//! Fire a session turn when a coworker asks `Turbo:` in chat or audio.

use crate::notification::types::{MeetingQuestion, ToolNotificationHandle};
use xai_grok_meetings::ask_instruction;

pub const AUTO_ASK_ENV: &str = "GROK_MEETING_AUTO_ASK";

pub fn auto_ask_enabled() -> bool {
    match std::env::var(AUTO_ASK_ENV) {
        Ok(s) => !matches!(s.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"),
        Err(_) => true,
    }
}

pub fn meeting_qa_task_id(from: &str, question: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    from.hash(&mut h);
    question.hash(&mut h);
    format!("meeting-qa-{:x}", h.finish())
}

/// Queue a research+reply turn. No-op if auto-ask is off or there is no sink.
pub fn emit_auto_ask(
    handle: Option<&ToolNotificationHandle>,
    from: &str,
    question: &str,
) -> bool {
    if !auto_ask_enabled() {
        return false;
    }
    let Some(h) = handle else {
        return false;
    };
    let from: String = from.chars().take(80).collect();
    let tagged = format!("(from {from}) {question}");
    h.send_meeting_question(MeetingQuestion {
        from: from.to_string(),
        question: question.to_string(),
        prompt: ask_instruction(Some(&tagged)),
        task_id: meeting_qa_task_id(&from, question),
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_id_is_stable() {
        let a = meeting_qa_task_id("alice", "status?");
        let b = meeting_qa_task_id("alice", "status?");
        let c = meeting_qa_task_id("bob", "status?");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("meeting-qa-"));
    }

    #[test]
    fn emit_sends_when_enabled() {
        if !auto_ask_enabled() {
            return;
        }
        let (h, mut rx) = ToolNotificationHandle::channel();
        assert!(emit_auto_ask(Some(&h), "alice", "How is the website?"));
        let n = rx.try_recv().expect("notification");
        match n {
            crate::notification::types::ToolNotification::MeetingQuestion(q) => {
                assert_eq!(q.from, "alice");
                assert!(q.prompt.contains("meeting_ask"));
                assert!(q.prompt.contains("website"));
                assert!(!q.task_id.is_empty());
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(!emit_auto_ask(None, "alice", "x"));
    }
}
