//! Teams guest-join choreography.
//!
//! The bot joins as an ordinary anonymous participant. Teams' default policy
//! detects external bots, labels them, and holds them in the lobby until an
//! organizer admits them. We **observe and report** that; nothing here tries to
//! look like a human, defeat detection, or answer a verification challenge.

use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use xai_grok_cdp::{Browser, LaunchOptions, Page};

use crate::audio::{self, AudioServer};
use crate::error::{BotError, Result};
use crate::selectors::{SELECTORS_ENV, Selectors};
use crate::state::{BotState, ChatMessage, TapEvent};

/// Binding the page uses to push events to Turbo.
const BINDING: &str = "__turboEvent";

/// Default display name. Deliberately self-identifying: participants should be
/// able to tell at a glance that a notetaker is present.
pub const DEFAULT_DISPLAY_NAME: &str = "Turbo (Notetaker)";

/// How long to wait for the page to reach the pre-join screen.
const PREJOIN_TIMEOUT: Duration = Duration::from_secs(60);

/// How long to wait, after clicking Join, for lobby or admission.
const JOIN_SETTLE_TIMEOUT: Duration = Duration::from_secs(45);

/// Poll interval for Rust-side waits.
const POLL: Duration = Duration::from_millis(500);

/// How the bot should join.
#[derive(Debug, Clone)]
pub struct BotConfig {
    /// Teams join URL, passcode included.
    pub join_url: String,
    /// Name other participants see.
    pub display_name: String,
    /// Throwaway browser profile directory.
    pub profile_dir: PathBuf,
    /// STT sample rate; the page's audio graph runs natively at this rate.
    pub sample_rate: u32,
    /// Samples per PCM frame pushed to Turbo (320 = 20 ms at 16 kHz).
    pub frame_samples: usize,
    /// Page-side poll interval for state, chat, and roster.
    pub poll_ms: u64,
    /// How long the notetaker may sit in the lobby before the state goes
    /// `Failed`. Reported, not auto-recovered — see [`TeamsBot::join`].
    pub lobby_timeout: Duration,
    /// Run the browser without a visible window.
    pub headless: bool,
    /// DOM selector table.
    pub selectors: Selectors,
}

impl BotConfig {
    /// Config with Turbo's defaults for a 16 kHz STT pipeline.
    pub fn new(join_url: impl Into<String>, profile_dir: impl Into<PathBuf>) -> Self {
        Self {
            join_url: join_url.into(),
            display_name: DEFAULT_DISPLAY_NAME.to_string(),
            profile_dir: profile_dir.into(),
            sample_rate: 16_000,
            frame_samples: 320,
            poll_ms: 1_000,
            lobby_timeout: Duration::from_secs(300),
            headless: true,
            selectors: Selectors::default(),
        }
    }
}

/// Config handed to the page as `__TURBO_CFG`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PageConfig<'a> {
    binding_name: &'a str,
    audio_url: &'a str,
    sample_rate: u32,
    frame_samples: usize,
    poll_ms: u64,
    max_buffered_bytes: u32,
    selectors: &'a Selectors,
}

/// WebSocket backlog past which the page drops PCM instead of queueing it.
///
/// ~2 s of 16 kHz mono i16. Live meeting audio that has queued longer than
/// that is stale by the time it reaches STT, so shedding beats buffering.
const MAX_BUFFERED_BYTES: u32 = 64_000;

/// Build the document-start script: config, then the tap.
pub fn build_init_script(cfg: &BotConfig, audio_url: &str) -> String {
    let page_cfg = PageConfig {
        binding_name: BINDING,
        audio_url,
        sample_rate: cfg.sample_rate,
        frame_samples: cfg.frame_samples,
        poll_ms: cfg.poll_ms,
        max_buffered_bytes: MAX_BUFFERED_BYTES,
        selectors: &cfg.selectors,
    };
    let json = serde_json::to_string(&page_cfg).unwrap_or_else(|_| "{}".to_string());
    format!(
        "globalThis.__TURBO_CFG = {json};\n{tap}",
        tap = include_str!("tap.js")
    )
}

/// A joined (or joining) Teams notetaker.
pub struct TeamsBot {
    page: Page,
    state_rx: watch::Receiver<BotState>,
    audio: AudioServer,
    _browser: Browser,
    _pump: JoinHandle<()>,
    _watchdog: JoinHandle<()>,
}

impl std::fmt::Debug for TeamsBot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TeamsBot")
            .field("state", &*self.state_rx.borrow())
            .field("audio_frames", &self.audio.frames())
            .finish_non_exhaustive()
    }
}

impl TeamsBot {
    /// Launch a browser, join as a guest, and stream audio and chat.
    ///
    /// Returns once the notetaker is in the lobby or admitted. A meeting that
    /// refuses guests, demands verification, or denies the bot returns `Err`
    /// so the caller can fall back to local capture.
    ///
    /// Sitting in the lobby past `lobby_timeout` moves the state to
    /// [`BotState::Failed`] but does **not** silently start recording the
    /// operator's speakers instead — beginning a different kind of capture
    /// without saying so would be a privacy surprise.
    pub async fn join(
        cfg: BotConfig,
        pcm_tx: mpsc::Sender<Vec<u8>>,
        chat_tx: mpsc::Sender<ChatMessage>,
    ) -> Result<Self> {
        if xai_grok_cdp::find_browser().is_none() {
            return Err(BotError::NoBrowser(
                "no Microsoft Edge or Chrome found to run the notetaker".to_string(),
            ));
        }

        let audio = audio::start(pcm_tx).await?;
        let (state_tx, state_rx) = watch::channel(BotState::Launching);

        let mut opts = LaunchOptions::new(&cfg.profile_dir);
        if !cfg.headless {
            opts = opts.windowed();
        }
        let browser = Browser::launch(&opts).await?;
        let page = browser.new_page().await?;
        page.expose_binding(BINDING).await?;
        page.add_init_script(&build_init_script(&cfg, audio.url()))
            .await?;

        let mut binding = page.binding_stream(BINDING);
        let pump_state = state_tx.clone();
        let pump = tokio::spawn(async move {
            while let Some(payload) = binding.next().await {
                let Some(event) = TapEvent::parse(&payload) else {
                    continue;
                };
                match event {
                    TapEvent::State { state } => {
                        if let Some(s) = BotState::from_page(&state) {
                            pump_state.send_if_modified(|cur| {
                                if *cur == s {
                                    false
                                } else {
                                    *cur = s;
                                    true
                                }
                            });
                        }
                    }
                    TapEvent::Chat { from, text } => {
                        // Untrusted: forwarded verbatim as data.
                        if chat_tx.send(ChatMessage { from, text }).await.is_err() {
                            break;
                        }
                    }
                    TapEvent::Roster { names } => {
                        tracing::debug!(count = names.len(), "meeting roster");
                    }
                    TapEvent::Audio { state } => {
                        tracing::debug!(state = %state, "meeting audio tap");
                    }
                    TapEvent::Error { step, message } => {
                        tracing::warn!(step = %step, error = %message, "meeting tap error");
                    }
                }
            }
        });

        page.navigate(&cfg.join_url).await?;

        let bot_result = drive_join(&page, &cfg, &state_tx).await;
        if let Err(e) = bot_result {
            pump.abort();
            audio.shutdown();
            browser.close().await;
            return Err(e);
        }

        let watchdog_state = state_tx.clone();
        let lobby_timeout = cfg.lobby_timeout;
        let watchdog = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + lobby_timeout;
            loop {
                tokio::time::sleep(POLL).await;
                let current = watchdog_state.borrow().clone();
                if current == BotState::Admitted || current.is_terminal() {
                    return;
                }
                // Unconditional: any state that is neither admitted nor
                // terminal still has to stop watching eventually, or this
                // task outlives the meeting.
                if tokio::time::Instant::now() >= deadline {
                    if current == BotState::Lobby {
                        let _ = watchdog_state.send(BotState::Failed(format!(
                            "nobody admitted the notetaker within {}s",
                            lobby_timeout.as_secs()
                        )));
                    }
                    return;
                }
            }
        });

        Ok(Self {
            page,
            state_rx,
            audio,
            _browser: browser,
            _pump: pump,
            _watchdog: watchdog,
        })
    }

    /// Current join state.
    pub fn state(&self) -> BotState {
        self.state_rx.borrow().clone()
    }

    /// Watch join-state transitions.
    pub fn state_watch(&self) -> watch::Receiver<BotState> {
        self.state_rx.clone()
    }

    /// PCM frames accepted so far. A flat counter means the tap stalled.
    pub fn audio_frames(&self) -> u64 {
        self.audio.frames()
    }

    /// PCM frames shed because STT fell behind. Non-zero means the transcript
    /// has gaps, which the operator should be able to see.
    pub fn audio_dropped(&self) -> u64 {
        self.audio.dropped()
    }

    /// Block until the notetaker is admitted, or the deadline passes.
    pub async fn wait_for_admitted(&self, timeout: Duration) -> Result<()> {
        let mut rx = self.state_rx.clone();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let current = rx.borrow_and_update().clone();
            match &current {
                BotState::Admitted => return Ok(()),
                BotState::Denied => return Err(BotError::Denied),
                BotState::Captcha => return Err(BotError::VerificationRequired),
                BotState::SignInRequired => return Err(BotError::SignInRequired),
                _ => {}
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(BotError::LobbyTimeout {
                    secs: timeout.as_secs(),
                });
            }
            if tokio::time::timeout(remaining, rx.changed()).await.is_err() {
                return Err(BotError::LobbyTimeout {
                    secs: timeout.as_secs(),
                });
            }
        }
    }

    /// Post a line into meeting chat as the notetaker's own guest identity.
    ///
    /// This is why the bot removes the `GROK_GRAPH_TOKEN` dependency: the
    /// message comes from "Turbo (Notetaker)", not from the operator.
    pub async fn post_chat(&self, text: &str) -> Result<()> {
        let literal = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
        let posted = self
            .page
            .evaluate(&format!("window.__turbo.postChat({literal})"))
            .await?;
        if posted == serde_json::Value::Bool(true) {
            Ok(())
        } else {
            Err(BotError::Selector {
                step: "chat_input",
                env: SELECTORS_ENV,
            })
        }
    }

    /// Current roster as Teams renders it.
    pub async fn participants(&self) -> Result<Vec<String>> {
        let value = self.page.evaluate("window.__turbo.participants()").await?;
        Ok(value
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Leave the meeting and shut the browser down.
    pub async fn leave(self) {
        let _ = self.page.close().await;
        self.audio.shutdown();
        self._pump.abort();
        self._watchdog.abort();
        self._browser.close().await;
    }
}

/// Walk the pre-join screens and click Join.
async fn drive_join(
    page: &Page,
    cfg: &BotConfig,
    state_tx: &watch::Sender<BotState>,
) -> Result<()> {
    let _ = state_tx.send(BotState::Loading);

    // The tap defines `__turbo` at document start; wait for the document.
    if !page
        .wait_for_expression("typeof window.__turbo === 'object'", PREJOIN_TIMEOUT, POLL)
        .await?
    {
        return Err(BotError::JoinTimeout {
            secs: PREJOIN_TIMEOUT.as_secs(),
        });
    }

    // Optional app-download interstitial.
    let _ = page.evaluate("window.__turbo.continueInBrowser()").await;

    // Reaching a *known* screen. Terminal refusals short-circuit here so the
    // caller falls back instead of waiting out the timeout.
    let reached = page
        .wait_for_expression(
            "['prejoin','lobby','admitted','denied','captcha','sign-in-required']\
             .includes(window.__turbo.state())",
            PREJOIN_TIMEOUT,
            POLL,
        )
        .await?;
    if !reached {
        return Err(BotError::JoinTimeout {
            secs: PREJOIN_TIMEOUT.as_secs(),
        });
    }
    check_terminal(page, state_tx).await?;

    let state = read_state(page).await;
    if state == BotState::Prejoin {
        let name = serde_json::to_string(&cfg.display_name)
            .unwrap_or_else(|_| "\"Turbo (Notetaker)\"".to_string());
        if page.evaluate(&format!("window.__turbo.setName({name})")).await?
            != serde_json::Value::Bool(true)
        {
            return Err(BotError::Selector {
                step: "name_input",
                env: SELECTORS_ENV,
            });
        }
        // A notetaker joins muted and dark. Do this before Join, not after.
        let _ = page.evaluate("window.__turbo.muteDevices()").await;

        if page.evaluate("window.__turbo.clickJoin()").await? != serde_json::Value::Bool(true) {
            return Err(BotError::Selector {
                step: "join_button",
                env: SELECTORS_ENV,
            });
        }
    }

    // Settle into lobby or the call.
    let settled = page
        .wait_for_expression(
            "['lobby','admitted','denied','captcha','sign-in-required']\
             .includes(window.__turbo.state())",
            JOIN_SETTLE_TIMEOUT,
            POLL,
        )
        .await?;
    check_terminal(page, state_tx).await?;
    if !settled {
        return Err(BotError::JoinTimeout {
            secs: JOIN_SETTLE_TIMEOUT.as_secs(),
        });
    }

    let final_state = read_state(page).await;
    let _ = state_tx.send(final_state);
    Ok(())
}

/// Read the page's own view of the join state.
async fn read_state(page: &Page) -> BotState {
    match page.evaluate("window.__turbo.state()").await {
        Ok(serde_json::Value::String(s)) => BotState::from_page(&s).unwrap_or(BotState::Loading),
        _ => BotState::Loading,
    }
}

/// Turn a refusal into the matching error, so callers fall back promptly.
async fn check_terminal(page: &Page, state_tx: &watch::Sender<BotState>) -> Result<()> {
    let state = read_state(page).await;
    let err = match state {
        BotState::Denied => BotError::Denied,
        BotState::Captcha => BotError::VerificationRequired,
        BotState::SignInRequired => BotError::SignInRequired,
        _ => return Ok(()),
    };
    let _ = state_tx.send(state);
    Err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> BotConfig {
        BotConfig::new("https://teams.microsoft.com/l/meetup-join/abc", "/tmp/p")
    }

    #[test]
    fn defaults_match_the_stt_pipeline() {
        let c = cfg();
        assert_eq!(c.sample_rate, 16_000, "STT expects 16 kHz mono i16");
        assert_eq!(c.frame_samples, 320, "20 ms at 16 kHz");
        assert!(c.headless);
    }

    #[test]
    fn display_name_is_self_identifying() {
        assert!(
            DEFAULT_DISPLAY_NAME.to_lowercase().contains("notetaker"),
            "participants must be able to tell a recorder joined"
        );
        assert_eq!(cfg().display_name, DEFAULT_DISPLAY_NAME);
    }

    #[test]
    fn init_script_embeds_config_then_tap() {
        let script = build_init_script(&cfg(), "ws://127.0.0.1:5/tok");
        assert!(script.starts_with("globalThis.__TURBO_CFG = {"));
        assert!(script.contains("\"audioUrl\":\"ws://127.0.0.1:5/tok\""));
        assert!(script.contains("\"bindingName\":\"__turboEvent\""));
        assert!(script.contains("\"sampleRate\":16000"));
        assert!(script.contains("\"frameSamples\":320"));
        // The page reads this to decide when to shed audio; a serde rename
        // would silently remove the ceiling and restore unbounded buffering.
        assert!(script.contains("\"maxBufferedBytes\":"), "{script}");
        assert!(include_str!("tap.js").contains("CFG.maxBufferedBytes"));
        // Selector table must reach the page.
        assert!(script.contains("\"joinButton\""));
        // And the tap itself must be appended.
        assert!(script.contains("__turboTapInstalled"));
        assert!(script.contains("registerProcessor('turbo-tap'"));
    }

    /// The config line is JS source handed to `addScriptToEvaluateOnNewDocument`,
    /// never HTML, so `</script>` is inert. What *would* matter is a selector
    /// escaping its JSON string literal and becoming executable, so that is
    /// what this pins.
    #[test]
    fn hostile_selector_override_cannot_escape_its_string_literal() {
        let mut c = cfg();
        let hostile = r#"a"]}; evil(); //"#;
        c.selectors.join_button = vec![hostile.to_string()];
        let script = build_init_script(&c, "ws://x/y");

        let json = script
            .trim_start_matches("globalThis.__TURBO_CFG = ")
            .split(";\n")
            .next()
            .expect("config line");
        let parsed: serde_json::Value =
            serde_json::from_str(json).expect("config line must stay valid JSON");

        // The payload survives as inert data, and the raw form never appears.
        assert_eq!(parsed["selectors"]["joinButton"][0], hostile);
        assert!(
            !script.contains(&format!("\"{hostile}\"")),
            "quote must be escaped, not emitted raw"
        );
        assert!(script.contains(r#"a\"]}; evil(); //"#));
    }

    #[test]
    fn tap_never_calls_a_captcha_solver() {
        let tap = include_str!("tap.js");
        for banned in ["solveCaptcha", "recaptcha", "hcaptcha", "anti-captcha"] {
            assert!(
                !tap.to_lowercase().contains(&banned.to_lowercase()),
                "tap must not attempt `{banned}`"
            );
        }
    }

    #[test]
    fn tap_reports_captcha_rather_than_engaging() {
        let tap = include_str!("tap.js");
        assert!(tap.contains("'captcha'"), "captcha must be a reported state");
    }

    /// Tracks arrive together when several people join at once; the graph must
    /// be a memoized promise so racers wait for it rather than sailing past a
    /// still-undefined mixer and being dropped for the meeting.
    #[test]
    fn tap_builds_the_audio_graph_behind_a_promise() {
        let tap = include_str!("tap.js");
        assert!(tap.contains("graphReady"), "graph must be memoized");
        assert!(
            !tap.contains("started = true"),
            "a boolean latch reintroduces the dropped-track race"
        );
        assert!(
            tap.contains("attached.delete(track)"),
            "a failed graph build must let a later track retry"
        );
    }

    /// The worklet reads one channel, so the mixer has to downmix explicitly.
    #[test]
    fn tap_forces_a_mono_downmix() {
        let tap = include_str!("tap.js");
        assert!(tap.contains("channelCountMode = 'explicit'"));
        assert!(tap.contains("mixer.channelCount = 1"));
    }

    #[test]
    fn tap_installs_a_silent_outbound_track() {
        let tap = include_str!("tap.js");
        assert!(tap.contains("silentOutboundTrack"));
        assert!(
            tap.contains("gain.gain.value = 0"),
            "outbound audio must be silent, not the Chromium fake-device beep"
        );
        assert!(
            tap.contains("__turboSpeechDestination"),
            "v2 TTS needs a documented injection point"
        );
    }

    #[test]
    fn page_config_serializes_camel_case() {
        let sels = Selectors::default();
        let pc = PageConfig {
            binding_name: BINDING,
            audio_url: "ws://x",
            sample_rate: 16_000,
            frame_samples: 320,
            poll_ms: 1_000,
            max_buffered_bytes: MAX_BUFFERED_BYTES,
            selectors: &sels,
        };
        let json = serde_json::to_string(&pc).unwrap();
        assert!(json.contains("\"bindingName\""));
        assert!(json.contains("\"pollMs\""));
        assert!(!json.contains("\"binding_name\""));
    }
}
