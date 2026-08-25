//! Fire a session turn when a coworker asks `Turbo:` in chat or audio.

use crate::notification::types::{MeetingQuestion, ToolNotificationHandle};
use xai_grok_meetings::ask_instruction;

pub const AUTO_ASK_ENV: &str = "GROK_MEETING_AUTO_ASK";

/// Prefix on both the injected task id and the resulting prompt id.
///
/// This is the tag that marks a turn as driven by untrusted meeting text.
/// The pager copies it onto the prompt id and the shell parses it back into
/// `PromptOrigin::MeetingQuestion`, which confines the turn to read-only
/// tools. Changing it in one place without the others silently removes that
/// confinement, so all three read this constant.
pub const MEETING_QA_TASK_PREFIX: &str = "meeting-qa-";

pub fn auto_ask_enabled() -> bool {
    match std::env::var(AUTO_ASK_ENV) {
        Ok(s) => !matches!(s.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"),
        // Off unless the operator opts in: meeting-guest text must not start a
        // workspace-reading turn by default.
        Err(_) => false,
    }
}

pub fn meeting_qa_task_id(from: &str, question: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    from.hash(&mut h);
    question.hash(&mut h);
    format!("{MEETING_QA_TASK_PREFIX}{:x}", h.finish())
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

    static AUTO_ASK_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct EnvGuard {
        prev: Option<String>,
    }

    impl EnvGuard {
        fn set(val: &str) -> Self {
            let prev = std::env::var(AUTO_ASK_ENV).ok();
            unsafe { std::env::set_var(AUTO_ASK_ENV, val) };
            Self { prev }
        }

        fn unset() -> Self {
            let prev = std::env::var(AUTO_ASK_ENV).ok();
            unsafe { std::env::remove_var(AUTO_ASK_ENV) };
            Self { prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(prev) => unsafe { std::env::set_var(AUTO_ASK_ENV, prev) },
                None => unsafe { std::env::remove_var(AUTO_ASK_ENV) },
            }
        }
    }

    #[test]
    fn auto_ask_default_is_false() {
        let _lock = AUTO_ASK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = EnvGuard::unset();
        assert!(!auto_ask_enabled());
    }

    #[test]
    fn emit_sends_when_enabled() {
        let _lock = AUTO_ASK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = EnvGuard::set("1");
        assert!(auto_ask_enabled());
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

    #[test]
    fn emit_is_noop_when_default_off() {
        let _lock = AUTO_ASK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = EnvGuard::unset();
        let (h, mut rx) = ToolNotificationHandle::channel();
        assert!(!emit_auto_ask(Some(&h), "alice", "How is the website?"));
        assert!(rx.try_recv().is_err());
    }
}
