//! Teams DOM selectors, in one table.
//!
//! Teams web ships UI changes on Microsoft's schedule, not ours. Every selector
//! is a *candidate list* tried in order, and the whole table can be overridden
//! from disk so an operator can repair a broken join without waiting for a
//! Turbo release. This is the single file to edit when the DOM moves.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Env var pointing at a selector-override JSON file.
pub const SELECTORS_ENV: &str = "GROK_MEETING_SELECTORS";

/// Filename looked up under `$GROK_HOME` when the env var is unset.
pub const SELECTORS_FILENAME: &str = "teams-selectors.json";

fn v(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

macro_rules! selector_table {
    ($( $(#[$m:meta])* $field:ident : $default:expr ),+ $(,)?) => {
        /// Candidate selectors and text probes for the Teams web client.
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(default, rename_all = "camelCase")]
        pub struct Selectors {
            $( $(#[$m])* pub $field: Vec<String>, )+
        }

        impl Default for Selectors {
            fn default() -> Self {
                Self { $( $field: v(&$default), )+ }
            }
        }

        impl Selectors {
            /// Field names paired with their candidate lists.
            pub fn entries(&self) -> Vec<(&'static str, &[String])> {
                vec![ $( (stringify!($field), self.$field.as_slice()), )+ ]
            }
        }
    };
}

selector_table! {
    /// The "Continue on this browser" interstitial Teams shows before pre-join.
    continue_in_browser: [
        "[data-tid='joinOnWeb']",
        "button[data-tid='continue-on-web']",
        "a[data-tid='joinOnWeb']",
        "button[aria-label*='Continue on this browser' i]",
    ],
    /// Guest display-name box on the pre-join screen.
    name_input: [
        "input[data-tid='prejoin-display-name-input']",
        "input[data-tid='prejoin-display-name']",
        "input[data-tid='prejoin-name-input']",
        "input[data-tid='preJoinDisplayName']",
        "input[data-tid='displayname']",
        "[data-tid='prejoin-display-name-section'] input",
        "[data-tid='prejoin-display-name'] input",
        "input[id*='displayName' i]",
        "input[name='displayName']",
        "input[name='username']",
        "input[autocomplete='name']",
        "input[aria-label*='name' i]",
        "input[aria-label*='Your name' i]",
        "input[placeholder*='name' i]",
        "input[placeholder*='Type your name' i]",
        "input[placeholder*='Enter your name' i]",
        "#displayName",
        "#username",
        "#prejoin-display-name",
        "#premeeting-name-input",
    ],
    /// The button that submits the pre-join screen.
    join_button: [
        "button[data-tid='prejoin-join-button']",
        "button[data-tid='joinBtn']",
        "button[data-tid='prejoin-join']",
        "button[data-tid='join-now']",
        "button[aria-label*='Join now' i]",
        "button[aria-label*='Join meeting' i]",
        "button[title*='Join now' i]",
        "button[title*='Join' i]",
    ],
    /// Microphone toggle on the pre-join screen.
    mic_toggle: [
        "[data-tid='toggle-mute']",
        "button[aria-label*='microphone' i]",
        "#microphone-button",
    ],
    /// Camera toggle on the pre-join screen.
    camera_toggle: [
        "[data-tid='toggle-video']",
        "button[aria-label*='camera' i]",
        "#video-button",
    ],
    /// Present once we are in the call proper.
    call_controls: [
        "[data-tid='call-controls']",
        "[data-tid='hangup-button']",
        "#hangup-button",
        "[data-tid='toggle-chat']",
    ],
    /// Explicit lobby element, when Teams renders one.
    lobby_indicator: [
        "[data-tid='lobby-screen']",
        "[data-tid='waiting-in-lobby']",
    ],
    /// Roster entries.
    participant: [
        "[data-tid='participant-item']",
        "[data-tid='roster-participant']",
        "li[role='listitem'][data-tid*='participant']",
    ],
    /// One chat message container.
    chat_message: [
        "[data-tid='chat-pane-message']",
        "[data-tid='message-pane-list-item']",
        "div[role='listitem'][data-mid]",
    ],
    /// Author within a chat message.
    chat_author: [
        "[data-tid='message-author-name']",
        "[data-tid='messageAuthorName']",
        ".ui-chat__message__author",
    ],
    /// Body within a chat message.
    chat_body: [
        "[data-tid='message-body-content']",
        "[data-tid='messageBodyContent']",
        ".ui-chat__message__content",
    ],
    /// Chat composer.
    chat_input: [
        "[data-tid='ckeditor']",
        "div[contenteditable='true'][role='textbox']",
        "[data-tid='newMessageCommands'] div[contenteditable='true']",
    ],
    /// Chat send button.
    chat_send: [
        "button[data-tid='newMessageCommands-send']",
        "button[data-tid='sendMessageCommands-send']",
        "button[aria-label*='Send' i]",
    ],
    /// Body-text probes meaning "waiting in the lobby".
    lobby_text: [
        "someone in the meeting should let you in",
        "waiting for someone to let you in",
        "you're in the lobby",
        "when the meeting starts, we'll let people know you're waiting",
    ],
    /// Body-text probes meaning "the organizer refused us".
    denied_text: [
        "you were removed from the meeting",
        "didn't let you in",
        "did not let you in",
        "sorry, you were denied",
    ],
    /// Body-text probes for a verification challenge.
    ///
    /// Turbo never solves these. Detecting one is how we fall back honestly.
    captcha_text: [
        "verify you're a human",
        "verify you are a human",
        "enter the characters you see",
        "security check",
    ],
    /// Body-text probes meaning anonymous join is refused.
    sign_in_required_text: [
        "sign in to join",
        "only people in the organization can join",
        "you need to sign in",
    ],
}

impl Selectors {
    /// Load overrides from `path`, falling back to defaults when absent.
    ///
    /// A malformed file is an error rather than a silent fallback: a typo that
    /// silently reverted to defaults would be diagnosed as "Teams changed".
    pub fn load_from(path: &Path) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Resolve the override path: `GROK_MEETING_SELECTORS`, else
    /// `<grok_home>/teams-selectors.json`, else none.
    pub fn override_path(grok_home: Option<&Path>) -> Option<PathBuf> {
        if let Ok(p) = std::env::var(SELECTORS_ENV) {
            let p = p.trim();
            if !p.is_empty() {
                return Some(PathBuf::from(p));
            }
        }
        let candidate = grok_home?.join(SELECTORS_FILENAME);
        candidate.is_file().then_some(candidate)
    }

    /// Defaults, with any override file applied on top.
    pub fn resolve(grok_home: Option<&Path>) -> Self {
        match Self::override_path(grok_home) {
            Some(path) => match Self::load_from(&path) {
                Ok(s) => {
                    tracing::info!(path = %path.display(), "loaded Teams selector overrides");
                    s
                }
                Err(e) => {
                    tracing::warn!(error = %e, "selector override unusable; using defaults");
                    Self::default()
                }
            },
            None => Self::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_selector_group_has_candidates() {
        for (name, list) in Selectors::default().entries() {
            assert!(!list.is_empty(), "`{name}` has no candidates");
            for sel in list {
                assert!(!sel.trim().is_empty(), "`{name}` has a blank candidate");
            }
        }
    }

    #[test]
    fn table_covers_every_step_the_join_needs() {
        let s = Selectors::default();
        let names: Vec<&str> = s.entries().into_iter().map(|(n, _)| n).collect();
        for required in [
            "continue_in_browser",
            "name_input",
            "join_button",
            "mic_toggle",
            "camera_toggle",
            "call_controls",
            "lobby_text",
            "denied_text",
            "captcha_text",
            "sign_in_required_text",
            "chat_message",
            "chat_input",
            "chat_send",
            "participant",
        ] {
            assert!(names.contains(&required), "missing `{required}`");
        }
    }

    #[test]
    fn serializes_camel_case_for_the_page() {
        let json = serde_json::to_string(&Selectors::default()).unwrap();
        assert!(json.contains("\"nameInput\""), "{json}");
        assert!(json.contains("\"lobbyText\""));
        assert!(!json.contains("\"name_input\""));
    }

    #[test]
    fn partial_override_keeps_other_defaults() {
        let s: Selectors =
            serde_json::from_str(r##"{"joinButton":["#only-this"]}"##).unwrap();
        assert_eq!(s.join_button, vec!["#only-this".to_string()]);
        assert_eq!(s.name_input, Selectors::default().name_input);
    }

    #[test]
    fn malformed_override_is_an_error_not_a_silent_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(Selectors::load_from(&path).is_err());
    }

    #[test]
    fn round_trips_through_json() {
        let s = Selectors::default();
        let text = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Selectors>(&text).unwrap(), s);
    }

    #[test]
    fn override_path_absent_without_home_or_env() {
        // Env may be set by the operator; only assert the no-home branch.
        if std::env::var(SELECTORS_ENV).is_err() {
            assert!(Selectors::override_path(None).is_none());
        }
    }
}
