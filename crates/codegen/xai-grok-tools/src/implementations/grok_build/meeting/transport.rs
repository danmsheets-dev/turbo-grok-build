//! Choosing between a joined notetaker and local capture.
//!
//! The bot is preferred for Teams. Anything that stops it — no browser, a
//! meeting that refuses guests, a verification challenge, stale selectors —
//! falls back to the existing local capture rather than losing the meeting.
//! The reason is always reported; a silent downgrade would leave the operator
//! waiting for a lobby admit that is never coming.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc;
use xai_grok_meeting_bot::{BotConfig, BotError, BotState, TeamsBot, bot_enabled};
use xai_grok_meetings::{
    JoinFailureStage, MeetingPlatform, MeetingStore, MeetingUrl, NotetakerOutcome,
    redact_join_secrets, teams_web_join_url, teams_web_rewrite_enabled,
};

use super::watch::append_inbox;

/// Where the notetaker browser profile lives, under the meeting's own folder.
pub fn bot_profile_dir(store: &MeetingStore) -> PathBuf {
    store.meta_path().with_file_name("bot-profile")
}

/// Why the bot path was not used. `None` means it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FallbackReason {
    /// `GROK_MEETING_BOT=0`.
    Disabled,
    /// The platform has no bot implementation yet.
    UnsupportedPlatform(MeetingPlatform),
    /// The bot tried and could not join.
    JoinFailed {
        /// Typed classification, durable in `meta.json`.
        stage: JoinFailureStage,
        /// Short operator-facing reason.
        detail: String,
    },
}

impl FallbackReason {
    /// Operator-facing line appended to the join output.
    pub fn line(&self) -> String {
        match self {
            Self::Disabled => {
                "notetaker bot disabled (GROK_MEETING_BOT=0) — capturing this machine's audio \
                 instead; no participant joins the meeting."
                    .into()
            }
            Self::UnsupportedPlatform(p) => format!(
                "no joined notetaker for {} yet — capturing this machine's audio instead; \
                 no participant joins the meeting.",
                p.label()
            ),
            Self::JoinFailed { detail, .. } => format!(
                "could not join as a notetaker ({detail}) — capturing this machine's audio \
                 instead; no participant joins the meeting."
            ),
        }
    }

    /// The durable outcome this reason implies.
    ///
    /// `meeting_join`, `meeting_status` and `meeting_stop` all render this one
    /// value, so they cannot disagree about whether a guest is in the meeting.
    pub fn outcome(&self) -> NotetakerOutcome {
        match self {
            Self::Disabled => NotetakerOutcome::NotAttempted {
                why: "GROK_MEETING_BOT=0".into(),
            },
            Self::UnsupportedPlatform(p) => NotetakerOutcome::NotAttempted {
                why: format!("no joined notetaker for {} yet", p.label()),
            },
            Self::JoinFailed { stage, detail } => NotetakerOutcome::Failed {
                stage: *stage,
                detail: detail.clone(),
            },
        }
    }
}

/// Classify a bot failure so `meta.json` records *which* step gave up.
///
/// "join timed out" was the same three words for a stale selector, a meeting
/// that never loaded, and Teams routing the notetaker at the desktop app --
/// which is why the field incident took four attempts to diagnose.
fn stage_of(e: &BotError) -> JoinFailureStage {
    match e {
        BotError::NoBrowser(_) => JoinFailureStage::NoBrowser,
        BotError::Cdp(_) => JoinFailureStage::Browser,
        BotError::Audio(_) => JoinFailureStage::Audio,
        BotError::Selector { .. } => JoinFailureStage::Selector,
        BotError::LobbyTimeout { .. } => JoinFailureStage::LobbyTimeout,
        BotError::Denied => JoinFailureStage::Denied,
        BotError::VerificationRequired => JoinFailureStage::Verification,
        BotError::SignInRequired => JoinFailureStage::SignInRequired,
        BotError::LauncherHandoff => JoinFailureStage::LauncherHandoff,
        BotError::JoinTimeout { .. } => JoinFailureStage::JoinTimeout,
    }
}

/// Should we attempt a joined notetaker for this platform?
pub fn bot_candidate(platform: MeetingPlatform) -> Result<(), FallbackReason> {
    if !bot_enabled() {
        return Err(FallbackReason::Disabled);
    }
    if platform != MeetingPlatform::Teams {
        return Err(FallbackReason::UnsupportedPlatform(platform));
    }
    Ok(())
}

/// The URL the notetaker actually navigates.
///
/// The *only* place a rewritten URL is produced. Everything else -- the meeting
/// store, Graph subject lookup, the watcher, the operator-facing summary --
/// keeps using `url.raw`.
pub fn navigate_url(url: &MeetingUrl) -> String {
    if !teams_web_rewrite_enabled() {
        return url.raw.clone();
    }
    match teams_web_join_url(url) {
        Some(rewritten) => {
            tracing::info!(
                from = %redact_join_secrets(&url.raw),
                to = %redact_join_secrets(&rewritten),
                "notetaker asking Teams for the anonymous web client"
            );
            rewritten
        }
        None => url.raw.clone(),
    }
}

/// Build the bot config for a meeting.
pub fn config_for(url: &MeetingUrl, store: &MeetingStore, sample_rate: u32) -> BotConfig {
    let mut cfg = BotConfig::new(navigate_url(url), bot_profile_dir(store));
    cfg.sample_rate = sample_rate;
    cfg.selectors = xai_grok_meeting_bot::Selectors::resolve(Some(&crate::util::grok_home()));
    if let Some(secs) = env_secs("GROK_MEETING_LOBBY_TIMEOUT") {
        cfg.lobby_timeout = std::time::Duration::from_secs(secs);
    }
    // Diagnosing a failed join is much easier with the window visible.
    if matches!(
        std::env::var("GROK_MEETING_BOT_WINDOW").as_deref(),
        Ok("1") | Ok("true")
    ) {
        cfg.headless = false;
    }
    cfg
}

fn env_secs(key: &str) -> Option<u64> {
    std::env::var(key).ok()?.trim().parse::<u64>().ok()
}

/// Forward scraped meeting chat into `inbox.jsonl`, which `drain_inbox` reads.
///
/// Chat text is untrusted participant input and is written verbatim as data;
/// interpretation happens later, behind the read-only meeting toolset.
pub fn spawn_chat_ingress(
    store: MeetingStore,
    mut chat_rx: mpsc::Receiver<xai_grok_meeting_bot::ChatMessage>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(msg) = chat_rx.recv().await {
            if let Err(e) = append_inbox(&store, &msg.from, &msg.text) {
                tracing::warn!(error = %e, "meeting chat ingress write failed");
            }
        }
    })
}

/// Try to join as a notetaker. `Err` carries the reason to report and fall back on.
pub async fn try_join_bot(
    url: &MeetingUrl,
    store: &MeetingStore,
    sample_rate: u32,
    pcm_tx: mpsc::Sender<Vec<u8>>,
) -> Result<(Arc<TeamsBot>, tokio::task::JoinHandle<()>), FallbackReason> {
    let cfg = config_for(url, store, sample_rate);
    let (chat_tx, chat_rx) = mpsc::channel(64);
    match TeamsBot::join(cfg, pcm_tx, chat_tx).await {
        Ok(bot) => {
            let ingress = spawn_chat_ingress(store.clone(), chat_rx);
            Ok((Arc::new(bot), ingress))
        }
        Err(e) => {
            tracing::warn!(error = %e, "notetaker guest join failed");
            Err(FallbackReason::JoinFailed {
                stage: stage_of(&e),
                detail: e.short(),
            })
        }
    }
}

/// The join-output line describing where a live bot currently stands.
pub fn bot_status_line(state: &BotState) -> String {
    format!("notetaker: {}", state.label())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_teams_platforms_fall_back_with_a_named_reason() {
        let r = bot_candidate(MeetingPlatform::Zoom).unwrap_err();
        assert_eq!(r, FallbackReason::UnsupportedPlatform(MeetingPlatform::Zoom));
        let line = r.line();
        assert!(line.contains("Zoom"), "{line}");
        assert!(
            line.contains("no participant joins"),
            "fallback must say the lobby will stay empty: {line}"
        );
    }

    #[test]
    fn every_fallback_reason_says_no_participant_joins() {
        // This is the honesty requirement from fr_01a03036: an operator must
        // never be left waiting to admit a bot that was never dispatched.
        let reasons = [
            FallbackReason::Disabled,
            FallbackReason::UnsupportedPlatform(MeetingPlatform::GoogleMeet),
            FallbackReason::JoinFailed {
                stage: JoinFailureStage::Denied,
                detail: "denied".into(),
            },
        ];
        for r in reasons {
            let line = r.line();
            assert!(line.contains("no participant joins the meeting"), "{line}");
            assert!(
                line.contains("this machine's audio"),
                "must name what is actually being recorded: {line}"
            );
        }
    }

    #[test]
    fn join_failure_reason_is_carried_through() {
        let r = FallbackReason::JoinFailed {
            stage: JoinFailureStage::Verification,
            detail: "verification required".into(),
        };
        assert!(r.line().contains("verification required"));
    }

    /// Every bot error must land on a distinct, durable stage; a catch-all
    /// would put us back to "join timed out" meaning four different things.
    #[test]
    fn every_bot_error_maps_to_a_stage() {
        let cases: Vec<(BotError, JoinFailureStage)> = vec![
            (BotError::NoBrowser("x".into()), JoinFailureStage::NoBrowser),
            (BotError::Audio("x".into()), JoinFailureStage::Audio),
            (
                BotError::Selector { step: "s", env: "E" },
                JoinFailureStage::Selector,
            ),
            (
                BotError::LobbyTimeout { secs: 1 },
                JoinFailureStage::LobbyTimeout,
            ),
            (BotError::Denied, JoinFailureStage::Denied),
            (
                BotError::VerificationRequired,
                JoinFailureStage::Verification,
            ),
            (BotError::SignInRequired, JoinFailureStage::SignInRequired),
            (
                BotError::LauncherHandoff,
                JoinFailureStage::LauncherHandoff,
            ),
            (
                BotError::JoinTimeout { secs: 1 },
                JoinFailureStage::JoinTimeout,
            ),
        ];
        for (err, want) in cases {
            assert_eq!(stage_of(&err), want, "{err:?}");
        }
        // The launcher hop is the whole point: it must not collapse into the
        // generic timeout it used to be reported as.
        assert_ne!(
            stage_of(&BotError::LauncherHandoff),
            stage_of(&BotError::JoinTimeout { secs: 1 })
        );
    }

    #[test]
    fn fallback_reasons_become_honest_outcomes() {
        assert!(!FallbackReason::Disabled.outcome().guest_present());
        let failed = FallbackReason::JoinFailed {
            stage: JoinFailureStage::LauncherHandoff,
            detail: "Teams app launcher".into(),
        }
        .outcome();
        assert!(!failed.guest_present());
        assert!(failed.headline().contains("NO GUEST IN THE MEETING"), "{failed:?}");
        assert!(failed.headline().contains("Teams app launcher"), "{failed:?}");
    }

    /// The rewrite happens once, here, and never touches `url.raw`.
    #[test]
    fn config_for_rewrites_only_the_navigate_url() {
        // The rewrite is kill-switchable process-wide, so only assert the
        // relationship that must hold either way: whatever the bot navigates,
        // the parsed URL it came from is untouched.
        let url = xai_grok_meetings::parse_meeting_url(
            "https://teams.microsoft.com/meet/2907709513066?p=s3cret",
        )
        .unwrap();
        let raw_before = url.raw.clone();
        let navigate = navigate_url(&url);
        assert_eq!(url.raw, raw_before, "navigate_url must not mutate the parse");
        assert!(
            navigate.starts_with("https://teams.microsoft.com/meet/2907709513066"),
            "{navigate}"
        );
        assert!(navigate.contains("s3cret"), "passcode must survive: {navigate}");
    }

    /// Non-Teams URLs must never acquire Teams query parameters.
    #[test]
    fn navigate_url_is_identity_for_other_platforms() {
        for raw in [
            "https://us02web.zoom.us/j/123456789?pwd=x",
            "https://meet.google.com/abc-defg-hij",
        ] {
            let url = xai_grok_meetings::parse_meeting_url(raw).unwrap();
            assert_eq!(navigate_url(&url), url.raw, "{raw}");
        }
    }

    #[test]
    fn bot_status_line_is_prefixed() {
        assert_eq!(
            bot_status_line(&BotState::Lobby),
            format!("notetaker: {}", BotState::Lobby.label())
        );
    }

    #[test]
    fn profile_dir_sits_beside_the_meeting_meta() {
        let root = std::env::temp_dir().join("turbo-transport-profile");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let url = xai_grok_meetings::parse_meeting_url(
            "https://teams.microsoft.com/l/meetup-join/x",
        )
        .unwrap();
        let store = MeetingStore::create(
            &root,
            "teams-profile-1",
            &url,
            xai_grok_meetings::CaptureSource::None,
        )
        .unwrap();
        let dir = bot_profile_dir(&store);
        assert_eq!(dir.file_name().unwrap(), "bot-profile");
        assert_eq!(dir.parent(), store.meta_path().parent());
        let _ = std::fs::remove_dir_all(&root);
    }
}
