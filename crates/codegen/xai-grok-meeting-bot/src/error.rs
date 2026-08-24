//! Error type for the meeting bot.

/// Why a bot join could not proceed.
///
/// Every variant is a *fallback trigger*: the caller drops to local capture
/// rather than losing the meeting.
#[derive(Debug, thiserror::Error)]
pub enum BotError {
    /// No Chromium-family browser is installed.
    #[error("{0}")]
    NoBrowser(String),

    /// Driving the browser failed.
    #[error("browser: {0}")]
    Cdp(#[from] xai_grok_cdp::CdpError),

    /// The loopback audio sink could not be set up.
    #[error("audio: {0}")]
    Audio(String),

    /// A join step could not find its element.
    ///
    /// Names the step so a Teams UI change is diagnosable from one log line.
    #[error("Teams UI step `{step}` not found — selectors may be stale; override with {env}")]
    Selector {
        /// Which choreography step failed.
        step: &'static str,
        /// Env var pointing at a selector-override file.
        env: &'static str,
    },

    /// The organizer never admitted the bot.
    #[error("still waiting in the lobby after {secs}s — nobody admitted the notetaker")]
    LobbyTimeout {
        /// How long we waited.
        secs: u64,
    },

    /// The organizer refused the bot, or removed it.
    #[error("the meeting did not admit the notetaker")]
    Denied,

    /// A human-verification challenge was shown.
    ///
    /// Turbo does not solve these; this is always a fallback, never a retry.
    #[error("the meeting asked for human verification, which Turbo does not answer")]
    VerificationRequired,

    /// The meeting refuses anonymous participants.
    #[error("this meeting only admits signed-in users, so a guest notetaker cannot join")]
    SignInRequired,

    /// Teams served the desktop-app launcher instead of a web join screen.
    ///
    /// Distinct from [`Self::JoinTimeout`] because the remedy is different: the
    /// selectors are fine and the meeting is fine, but this URL is routing the
    /// notetaker at the installed desktop client instead of the web guest flow.
    #[error(
        "Teams served the desktop-app launcher page and never rendered a web join screen, \
         so the guest notetaker had nothing to drive"
    )]
    LauncherHandoff,

    /// The page never reached a state we recognize.
    #[error("the Teams page never reached a known state within {secs}s")]
    JoinTimeout {
        /// How long we waited.
        secs: u64,
    },
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, BotError>;

impl BotError {
    /// A short, operator-facing reason suitable for `meeting_status`.
    pub fn short(&self) -> String {
        match self {
            Self::NoBrowser(_) => "no browser".into(),
            Self::Cdp(_) => "browser error".into(),
            Self::Audio(_) => "audio setup failed".into(),
            Self::Selector { step, .. } => format!("Teams UI changed ({step})"),
            Self::LobbyTimeout { .. } => "not admitted".into(),
            Self::Denied => "denied".into(),
            Self::VerificationRequired => "verification required".into(),
            Self::SignInRequired => "sign-in required".into(),
            Self::LauncherHandoff => "Teams app launcher".into(),
            Self::JoinTimeout { .. } => "join timed out".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_error_names_the_step_and_the_escape_hatch() {
        let e = BotError::Selector {
            step: "join_button",
            env: crate::selectors::SELECTORS_ENV,
        };
        let text = e.to_string();
        assert!(text.contains("join_button"), "{text}");
        assert!(text.contains("GROK_MEETING_SELECTORS"), "{text}");
    }

    /// The launcher hop is a *routing* failure, not a stale-selector failure.
    /// Reporting it as a generic timeout is what made the field incident take
    /// four attempts to diagnose.
    #[test]
    fn launcher_handoff_is_distinguishable_from_a_plain_timeout() {
        let launcher = BotError::LauncherHandoff;
        assert_ne!(launcher.short(), BotError::JoinTimeout { secs: 60 }.short());
        assert!(launcher.to_string().contains("launcher"), "{launcher}");
        assert!(launcher.short().len() < 40, "{}", launcher.short());
    }

    #[test]
    fn verification_never_reads_as_retryable() {
        let text = BotError::VerificationRequired.to_string();
        assert!(text.contains("does not answer"), "{text}");
        assert_eq!(BotError::VerificationRequired.short(), "verification required");
    }

    #[test]
    fn every_variant_has_a_short_reason() {
        let all = [
            BotError::NoBrowser("x".into()),
            BotError::LauncherHandoff,
            BotError::Audio("x".into()),
            BotError::Selector { step: "s", env: "E" },
            BotError::LobbyTimeout { secs: 1 },
            BotError::Denied,
            BotError::VerificationRequired,
            BotError::SignInRequired,
            BotError::JoinTimeout { secs: 1 },
        ];
        for e in all {
            let s = e.short();
            assert!(!s.is_empty() && s.len() < 40, "{s:?}");
        }
    }
}
