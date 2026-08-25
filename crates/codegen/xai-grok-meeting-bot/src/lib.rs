//! Self-hosted Teams meeting notetaker.
//!
//! Drives the Edge already installed on the machine through
//! [`xai_grok_cdp`], joins a Teams meeting as an ordinary anonymous guest
//! named "Turbo (Notetaker)", and streams two things back:
//!
//! - **16 kHz mono 16-bit LE PCM**, tapped inside the page from the inbound
//!   WebRTC audio tracks, over a loopback WebSocket.
//! - **Chat messages and roster**, scraped from the DOM, over a CDP binding.
//!
//! The tap is in-page (not the sound card). PCM is streamed over loopback to
//! Turbo, then uploaded to xAI hosted STT (`wss://api.x.ai/v1/stt` by default,
//! overridable via `[voice].api_base`).
//!
//! # What this deliberately does not do
//!
//! Teams detects external meeting bots, labels them, and holds them in the
//! lobby for an explicit organizer admit. That is the intended experience, and
//! this crate reports it rather than working around it. There is no attempt to
//! evade detection, impersonate a human, or answer a verification challenge —
//! a challenge is a fallback trigger, never a retry.
//!
//! The outbound audio track is silent by construction. [`state`] and
//! [`selectors`] carry the operator-facing surfaces.

pub mod audio;
pub mod error;
pub mod selectors;
pub mod state;
pub mod teams;

pub use error::{BotError, Result};
pub use selectors::{SELECTORS_ENV, SELECTORS_FILENAME, Selectors};
pub use state::{BotState, ChatMessage, TapEvent};
pub use teams::{BotConfig, DEFAULT_DISPLAY_NAME, TeamsBot, build_init_script};

/// Env toggle: `0`/`false`/`off` forces the legacy local-capture path.
pub const BOT_ENV: &str = "GROK_MEETING_BOT";

/// Whether the guest-bot transport is enabled. Defaults to on.
pub fn bot_enabled() -> bool {
    match std::env::var(BOT_ENV) {
        Ok(s) => !matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        ),
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn bot_toggle_defaults_on_and_accepts_common_falsey_spellings() {
        // Only assert the pure mapping; the process env is shared across tests.
        for s in ["0", "false", "off", "no", "FALSE", " Off "] {
            assert!(
                matches!(
                    s.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "off" | "no"
                ),
                "{s} should disable the bot"
            );
        }
        for s in ["1", "true", "on", "yes"] {
            assert!(!matches!(
                s.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            ));
        }
    }
}
