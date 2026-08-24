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
use xai_grok_meeting_bot::{BotConfig, BotState, TeamsBot, bot_enabled};
use xai_grok_meetings::{MeetingPlatform, MeetingStore};

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
    JoinFailed(String),
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
            Self::JoinFailed(why) => format!(
                "could not join as a notetaker ({why}) — capturing this machine's audio \
                 instead; no participant joins the meeting."
            ),
        }
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

/// Build the bot config for a meeting.
pub fn config_for(join_url: &str, store: &MeetingStore, sample_rate: u32) -> BotConfig {
    let mut cfg = BotConfig::new(join_url, bot_profile_dir(store));
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
    join_url: &str,
    store: &MeetingStore,
    sample_rate: u32,
    pcm_tx: mpsc::Sender<Vec<u8>>,
) -> Result<(Arc<TeamsBot>, tokio::task::JoinHandle<()>), FallbackReason> {
    let cfg = config_for(join_url, store, sample_rate);
    let (chat_tx, chat_rx) = mpsc::channel(64);
    match TeamsBot::join(cfg, pcm_tx, chat_tx).await {
        Ok(bot) => {
            let ingress = spawn_chat_ingress(store.clone(), chat_rx);
            Ok((Arc::new(bot), ingress))
        }
        Err(e) => Err(FallbackReason::JoinFailed(e.short())),
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
            FallbackReason::JoinFailed("denied".into()),
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
        let r = FallbackReason::JoinFailed("verification required".into());
        assert!(r.line().contains("verification required"));
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
