//! Meeting notetaker tools (`meeting_join` / `stop` / `status` /
//! `transcript` / `notes`).
//!
//! Teams: try a guest named "Turbo (Notetaker)" in the lobby; fall back to
//! WASAPI loopback + mic on this machine when the guest cannot join. Other
//! platforms use local capture. Stream to Grok STT, persist transcript +
//! notes, auto-answer `Turbo:` questions, and write a work-only recap on stop.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use xai_grok_meetings::{
    CaptureSource, MeetingMeta, MeetingStatus, MeetingStore, NotetakerOutcome, briefing,
    compose_summary_markdown, default_meeting_title, extract_title_from_markdown, local_date_stamp,
    meeting_dir, new_meeting_id, parse_meeting_url, read_current_id, read_knowledge_dir,
    redact_join_secrets, summary_filename, unique_summary_path, workspace_meetings_dir,
    write_current, write_knowledge_dir, write_workspace_summary,
};
use xai_grok_voice::auth::{SharedVoiceAuth, StaticVoiceAuth, VoiceAuthProvider};
use xai_grok_voice::config::VoiceConfig;
use crate::notification::types::ToolNotificationHandle;
use crate::types::SharedApiKeyProvider;
use crate::types::output::ToolOutput;

mod ask;
mod auto_ask;
mod graph;
mod knowledge;
mod notes;
mod open;
mod pipeline;
mod reply;
mod status;
mod stop;
mod transcript;
mod transport;
mod watch;

pub mod join;

pub use ask::{MEETING_ASK_TOOL_NAME, MeetingAskInput, MeetingAskTool};
pub use auto_ask::MEETING_QA_TASK_PREFIX;
pub use join::{MEETING_JOIN_TOOL_NAME, MeetingJoinInput, MeetingJoinTool};
pub use knowledge::{MEETING_KNOWLEDGE_TOOL_NAME, MeetingKnowledgeInput, MeetingKnowledgeTool};
pub use notes::{MEETING_NOTES_TOOL_NAME, MeetingNotesInput, MeetingNotesTool};
pub use reply::{MEETING_REPLY_TOOL_NAME, MeetingReplyInput, MeetingReplyTool};
pub use status::{MEETING_STATUS_TOOL_NAME, MeetingStatusInput, MeetingStatusTool};
pub use stop::{MEETING_STOP_TOOL_NAME, MeetingStopInput, MeetingStopTool};
pub use transcript::{
    MEETING_TRANSCRIPT_TOOL_NAME, MeetingTranscriptInput, MeetingTranscriptTool,
};

pub use xai_grok_meetings::{
    MEETING_COMMAND_NAME, ask_instruction, detect_join_request, first_https_url,
    is_joinable_platform, join_instruction, knowledge_instruction, notes_instruction,
    reply_instruction, split_join_args, status_instruction, stop_instruction,
    transcript_instruction, usage_message,
};

struct KeyVoiceAuth(SharedApiKeyProvider);

impl std::fmt::Debug for KeyVoiceAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("KeyVoiceAuth").field(&"<redacted>").finish()
    }
}

impl VoiceAuthProvider for KeyVoiceAuth {
    fn bearer(&self) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>> {
        let p = Arc::clone(&self.0);
        Box::pin(async move { p.current_api_key_async().await })
    }
}

struct LiveMeeting {
    store: MeetingStore,
    capture_source: CaptureSource,
    stop: Arc<AtomicBool>,
    _task: JoinHandle<()>,
    _watch: JoinHandle<()>,
    capture: Option<xai_grok_voice::audio::CaptureHandle>,
    /// The joined notetaker, when the bot transport won.
    bot: Option<Arc<xai_grok_meeting_bot::TeamsBot>>,
    /// Chat-scrape forwarder feeding `inbox.jsonl`.
    ingress: Option<JoinHandle<()>>,
}

impl LiveMeeting {
    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Persist Stopped before aborting STT so an in-flight append_segment
        // cannot rewrite status=recording.
        let _ = self.store.mark_stopped();
        if let Some(cap) = self.capture.take() {
            cap.stop();
        }
        if let Some(ingress) = self.ingress.take() {
            ingress.abort();
        }
        // Dropping the bot drops its `Browser`, whose enrolled process group
        // reaps the whole Chromium tree. A notetaker must never outlive the
        // meeting it was recording.
        self.bot.take();
        self._task.abort();
        self._watch.abort();
    }
}

impl Drop for LiveMeeting {
    fn drop(&mut self) {
        self.shutdown();
    }
}

static LIVE: OnceLock<Mutex<HashMap<String, LiveMeeting>>> = OnceLock::new();

fn live_map() -> &'static Mutex<HashMap<String, LiveMeeting>> {
    LIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_live() -> std::sync::MutexGuard<'static, HashMap<String, LiveMeeting>> {
    live_map().lock().unwrap_or_else(|e| e.into_inner())
}

fn drain_live_meetings() {
    let meetings: Vec<LiveMeeting> = {
        let mut map = lock_live();
        map.drain().map(|(_, live)| live).collect()
    };
    drop(meetings);
}

extern "C" fn meeting_atexit() {
    drain_live_meetings();
}

unsafe extern "C" {
    fn atexit(cb: extern "C" fn()) -> i32;
}

fn ensure_meeting_atexit() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: once per process; handler only drains the live map.
        let _ = unsafe { atexit(meeting_atexit) };
    });
}

static JOINING: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();

fn joining_set() -> &'static Mutex<std::collections::HashSet<String>> {
    JOINING.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

/// Reserves a session for the duration of a join.
///
/// Joining a notetaker takes seconds (browser launch, lobby wait), so the
/// "already recording" check and the `LIVE` insert are far apart. Without a
/// reservation two concurrent `meeting_join` calls in one session would both
/// pass the check and launch two browsers into the same meeting.
struct JoinReservation(String);

impl JoinReservation {
    fn acquire(session_id: &str) -> Option<Self> {
        let mut set = joining_set().lock().unwrap_or_else(|e| e.into_inner());
        set.insert(session_id.to_string())
            .then(|| Self(session_id.to_string()))
    }
}

impl Drop for JoinReservation {
    fn drop(&mut self) {
        let mut set = joining_set().lock().unwrap_or_else(|e| e.into_inner());
        set.remove(&self.0);
    }
}

fn stop_live_session(session_id: &str) -> Option<MeetingStore> {
    if session_id.trim().is_empty() {
        return None;
    }
    let live = lock_live().remove(session_id)?;
    let store = live.store.clone();
    drop(live);
    Some(store)
}

struct MeetingHandleInner {
    session_id: String,
    session_folder: Option<PathBuf>,
    api_key_provider: Option<SharedApiKeyProvider>,
    notification: Option<ToolNotificationHandle>,
}

/// Session resource so tools can start/stop the notetaker.
#[derive(Clone)]
pub struct MeetingHandle {
    inner: Arc<MeetingHandleInner>,
}

impl MeetingHandle {
    pub fn new(
        session_id: impl Into<String>,
        session_folder: Option<PathBuf>,
        api_key_provider: Option<SharedApiKeyProvider>,
        notification: Option<ToolNotificationHandle>,
    ) -> Self {
        Self {
            inner: Arc::new(MeetingHandleInner {
                session_id: session_id.into(),
                session_folder,
                api_key_provider,
                notification,
            }),
        }
    }

    pub fn unbound() -> Self {
        Self {
            inner: Arc::new(MeetingHandleInner {
                session_id: String::new(),
                session_folder: None,
                api_key_provider: None,
                notification: None,
            }),
        }
    }

    fn folder(&self) -> Result<&PathBuf, xai_tool_runtime::ToolError> {
        self.inner.session_folder.as_ref().ok_or_else(|| {
            xai_tool_runtime::ToolError::custom(
                "meeting_no_session",
                "meeting_* tools need a session folder",
            )
        })
    }

    fn voice_auth(&self) -> Result<SharedVoiceAuth, xai_tool_runtime::ToolError> {
        if let Some(p) = &self.inner.api_key_provider {
            return Ok(Arc::new(KeyVoiceAuth(Arc::clone(p))));
        }
        StaticVoiceAuth::shared(std::env::var("XAI_API_KEY").unwrap_or_default()).ok_or_else(|| {
            xai_tool_runtime::ToolError::custom(
                "meeting_auth",
                "not signed in — run `turbo login` or set XAI_API_KEY for Grok STT",
            )
        })
    }

    pub async fn join(
        &self,
        url: &str,
        title: Option<&str>,
    ) -> Result<String, xai_tool_runtime::ToolError> {
        if self.inner.session_id.trim().is_empty() {
            return Err(xai_tool_runtime::ToolError::custom(
                "meeting_no_session",
                "meeting_* tools require a pager session id",
            ));
        }
        let parsed = parse_meeting_url(url).map_err(|e| {
            xai_tool_runtime::ToolError::custom("meeting_bad_url", e.to_string())
        })?;
        let folder = self.folder()?.clone();

        {
            let map = lock_live();
            if map.contains_key(&self.inner.session_id) {
                return Err(xai_tool_runtime::ToolError::custom(
                    "meeting_busy",
                    "a meeting is already being recorded in this session — /meeting stop first",
                ));
            }
        }
        // Held until this call returns, so a second join cannot slip in during
        // the browser launch and lobby wait.
        let _reservation = JoinReservation::acquire(&self.inner.session_id).ok_or_else(|| {
            xai_tool_runtime::ToolError::custom(
                "meeting_busy",
                "a meeting join is already in progress in this session",
            )
        })?;

        // NB: the join link is *not* opened here. Whether to hand it to the OS
        // depends on which transport wins, which is not known until below.
        let id = new_meeting_id(parsed.platform);
        let intended = pipeline::choose_capture_source();
        let store = MeetingStore::create(&folder, &id, &parsed, intended).map_err(|e| {
            xai_tool_runtime::ToolError::custom("meeting_store", e.to_string())
        })?;
        write_current(&folder, &id)
            .map_err(|e| xai_tool_runtime::ToolError::custom("meeting_store", e.to_string()))?;
        if let Some(k) = read_knowledge_dir(&folder) {
            let _ = store.set_knowledge_dir(&k);
        }

        let mut title = title
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if title.is_none() {
            if let Some(token) = graph::graph_token() {
                if let Ok(subject) =
                    graph::meeting_subject_for_join_url(&token, &parsed.raw).await
                {
                    title = Some(subject);
                }
            }
        }
        if let Some(ref t) = title {
            let _ = store.set_title(t);
        }

        let stop = Arc::new(AtomicBool::new(false));
        let mut bot: Option<Arc<xai_grok_meeting_bot::TeamsBot>> = None;
        let mut ingress: Option<JoinHandle<()>> = None;
        let mut fallback_note: Option<String> = None;
        // Every path below must leave this describing reality. Local capture
        // produces a healthy-looking transcript whether or not a guest joined,
        // so "nobody is in the lobby" has to be recorded, not inferred.
        let mut outcome = NotetakerOutcome::NotAttempted {
            why: "audio capture is disabled".to_string(),
        };

        let (task, capture, source) = if intended == CaptureSource::None {
            (tokio::spawn(async {}), None, CaptureSource::None)
        } else {
            // Prefer a joined notetaker: it hears the meeting rather than this
            // machine, and keeps working when the operator leaves.
            let mut chosen = None;
            match transport::bot_candidate(parsed.platform) {
                Ok(()) => {
                    let auth = self.voice_auth()?;
                    let config = VoiceConfig::default();
                    let (pcm_tx, pcm_rx) = mpsc::channel::<Vec<u8>>(64);
                    match transport::try_join_bot(&parsed, &store, config.sample_rate, pcm_tx)
                        .await
                    {
                        Ok((joined, chat_ingress)) => {
                            let stt_store = store.clone();
                            let stt_stop = stop.clone();
                            let stt_notif = self.inner.notification.clone();
                            let stt = tokio::spawn(async move {
                                pipeline::run_stt_loop(
                                    stt_store, auth, config, pcm_rx, stt_stop, stt_notif,
                                )
                                .await;
                            });
                            bot = Some(joined);
                            ingress = Some(chat_ingress);
                            outcome = NotetakerOutcome::Joined;
                            chosen = Some((stt, None, CaptureSource::MeetingBot));
                        }
                        Err(reason) => {
                            outcome = reason.outcome();
                            fallback_note = Some(reason.line());
                        }
                    }
                }
                Err(reason) => {
                    outcome = reason.outcome();
                    fallback_note = Some(reason.line());
                }
            }
            match chosen {
                Some(via_bot) => via_bot,
                None => {
                    let (task, cap, actual) =
                        self.spawn_capture(store.clone(), stop.clone(), intended)?;
                    if actual != intended {
                        let _ = store.set_capture_source(actual);
                    }
                    (task, Some(cap), actual)
                }
            }
        };
        if source != intended {
            let _ = store.set_capture_source(source);
        }
        // On disk, because `meeting_status` and `meeting_reply` both re-read the
        // store and must still tell the truth after a restart.
        let _ = store.set_notetaker_outcome(outcome.clone());

        // Only the local-capture paths need the operator in the meeting, so
        // only they get the link handed to the OS. Doing this unconditionally
        // is what put a File Explorer window on screen during a bot join.
        let shell_open = if open::should_shell_open(source) {
            Some(open::open_meeting_url(&parsed.raw).await)
        } else {
            None
        };
        let join_url = parsed.raw.clone();
        let watch_store = store.clone();
        let watch_stop = stop.clone();
        let watch_notif = self.inner.notification.clone();
        let watch = tokio::spawn(async move {
            watch::run_watch(watch_store, join_url, watch_stop, watch_notif).await;
        });

        ensure_meeting_atexit();
        let bot_state = bot.as_ref().map(|b| b.state());
        lock_live().insert(
            self.inner.session_id.clone(),
            LiveMeeting {
                store: store.clone(),
                capture_source: source,
                stop,
                _task: task,
                _watch: watch,
                capture,
                bot,
                ingress,
            },
        );

        Ok(JoinSummary {
            platform: parsed.platform,
            kind: parsed.kind,
            id: &id,
            title: title.as_deref(),
            redacted_url: &redact_join_secrets(&parsed.raw),
            transcript: &store.transcript_path().display().to_string(),
            source,
            outcome: &outcome,
            bot_state: bot_state.as_ref(),
            shell_open,
            fallback_note: fallback_note.as_deref(),
            loopback_only: pipeline::capture_pref_from_env()
                == pipeline::CapturePref::LoopbackOnly,
        }
        .render()
        .join("\n"))
    }

    fn spawn_capture(
        &self,
        store: MeetingStore,
        stop: Arc<AtomicBool>,
        intended: CaptureSource,
    ) -> Result<
        (
            JoinHandle<()>,
            xai_grok_voice::audio::CaptureHandle,
            CaptureSource,
        ),
        xai_tool_runtime::ToolError,
    > {
        let auth = self.voice_auth()?;
        let config = VoiceConfig::default();
        let (pcm_tx, pcm_rx) = mpsc::channel::<Vec<u8>>(64);
        let prefer_loopback = intended == CaptureSource::Loopback;
        let include_mic = intended == CaptureSource::Microphone
            || pipeline::capture_pref_from_env() != pipeline::CapturePref::LoopbackOnly;
        let (capture, report) = xai_grok_voice::audio::spawn_meeting_pcm_capture(
            config.sample_rate,
            pcm_tx,
            prefer_loopback,
            include_mic,
        )
        .map_err(|e| {
            xai_tool_runtime::ToolError::custom(
                "meeting_capture",
                format!("could not open audio capture: {e}"),
            )
        })?;
        let actual = if report.used_loopback {
            CaptureSource::Loopback
        } else {
            CaptureSource::Microphone
        };
        if !report.device_labels.is_empty() {
            tracing::info!(
                devices = %report.device_labels.join(", "),
                loopback = report.used_loopback,
                mic = report.mic,
                "meeting capture devices"
            );
        }

        let stt_notif = self.inner.notification.clone();
        let task = tokio::spawn(async move {
            pipeline::run_stt_loop(store, auth, config, pcm_rx, stop, stt_notif).await;
        });
        Ok((task, capture, actual))
    }

    pub fn stop(&self) -> Result<String, xai_tool_runtime::ToolError> {
        let Some(store) = stop_live_session(&self.inner.session_id) else {
            return Err(xai_tool_runtime::ToolError::custom(
                "meeting_idle",
                "no active meeting recording in this session",
            ));
        };
        let meta = store.read_meta().map_err(|e| {
            xai_tool_runtime::ToolError::custom("meeting_store", e.to_string())
        })?;
        // Keep current.txt so /meeting notes can still write the work-folder summary.
        // The notetaker line is what stops a stop-after-failed-join from being
        // shape-identical to a stop after a real one.
        Ok(format!(
            "Notetaker stopped.\nid: {}\nname: {}\nsegments: {}\ncapture: {:?}\nnotetaker: {}\ntranscript: {}\nNext: write a work-only recap with meeting_notes (saved as Meetings/YYYY-MM-DD - <name>.md in the work folder).",
            meta.id,
            meta.title.as_deref().unwrap_or("(untitled)"),
            meta.final_segments,
            meta.capture_source,
            format_notetaker_line(&meta),
            store.transcript_path().display(),
        ))
    }

    pub fn status_text(&self) -> Result<String, xai_tool_runtime::ToolError> {
        if let Some(live) = lock_live().get(&self.inner.session_id) {
            let meta = live.store.read_meta().map_err(|e| {
                xai_tool_runtime::ToolError::custom("meeting_store", e.to_string())
            })?;
            let mut text = format_meta(&meta, Some(live.capture_source), &live.store);
            if let Some(bot) = &live.bot {
                text.push_str(&format!(
                    "\n{}\nnotetaker_audio_frames: {}\nnotetaker_audio_dropped: {}",
                    transport::bot_status_line(&bot.state()),
                    bot.audio_frames(),
                    bot.audio_dropped()
                ));
            }
            return Ok(text);
        }
        let folder = self.folder()?;
        if let Some(id) = read_current_id(folder).ok().flatten() {
            let (store, meta) = MeetingStore::open(meeting_dir(folder, &id)).map_err(|e| {
                xai_tool_runtime::ToolError::custom("meeting_store", e.to_string())
            })?;
            return Ok(format_meta(&meta, None, &store));
        }
        Ok("No meeting notetaker is running in this session.".into())
    }

    pub fn transcript_text(&self) -> Result<String, xai_tool_runtime::ToolError> {
        let store = self.active_or_current_store()?;
        let text = store.transcript_text().map_err(|e| {
            xai_tool_runtime::ToolError::custom("meeting_store", e.to_string())
        })?;
        if text.trim().is_empty() {
            Ok("(transcript empty so far)".into())
        } else {
            Ok(text)
        }
    }

    pub fn attach_knowledge(&self, path: &str) -> Result<String, xai_tool_runtime::ToolError> {
        let folder = self.folder()?.clone();
        let dir = PathBuf::from(path);
        if dir.as_os_str().is_empty() {
            return Err(xai_tool_runtime::ToolError::custom(
                "meeting_knowledge",
                "path is empty",
            ));
        }
        let dir = if dir.is_absolute() {
            dir
        } else {
            std::env::current_dir()
                .map_err(|e| xai_tool_runtime::ToolError::custom("meeting_knowledge", e.to_string()))?
                .join(dir)
        };
        if !dir.is_dir() {
            return Err(xai_tool_runtime::ToolError::custom(
                "meeting_knowledge",
                format!("path is not a directory: {}", dir.display()),
            ));
        }
        write_knowledge_dir(&folder, &dir).map_err(|e| {
            xai_tool_runtime::ToolError::custom("meeting_knowledge", e.to_string())
        })?;
        if let Ok(store) = self.active_or_current_store() {
            let _ = store.set_knowledge_dir(&dir);
        }
        Ok(format!(
            "Optional extra notes path recorded:\n{}\nTurbo already researches the launch workspace with full tools (including MCP). This path is extra context only — no folder or projects.md was created.\nRun /meeting ask <question> (or with no args to drain a pending Turbo: question).",
            dir.display()
        ))
    }

    pub fn ask(
        &self,
        question: Option<&str>,
        workspace: Option<&std::path::Path>,
    ) -> Result<String, xai_tool_runtime::ToolError> {
        let folder = self.folder()?;
        let store = self.active_or_current_store().ok();
        let q = match question.map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) => s.to_string(),
            None => {
                let Some(ref store) = store else {
                    return Err(xai_tool_runtime::ToolError::custom(
                        "meeting_ask",
                        "no question and no meeting — pass question= or /meeting join first",
                    ));
                };
                match store.take_next_question().map_err(|e| {
                    xai_tool_runtime::ToolError::custom("meeting_store", e.to_string())
                })? {
                    Some((from, q)) => format!("(from {from}) {q}"),
                    None => {
                        return Err(xai_tool_runtime::ToolError::custom(
                            "meeting_ask",
                            "no pending Turbo: questions — pass question= or wait for chat/inbox",
                        ));
                    }
                }
            }
        };
        let extra = store
            .as_ref()
            .and_then(|s| s.read_meta().ok())
            .and_then(|m| m.knowledge_dir.map(PathBuf::from))
            .or_else(|| read_knowledge_dir(folder));
        let cwd = workspace
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok());
        Ok(briefing(&q, cwd.as_deref(), extra.as_deref(), store.as_ref()))
    }

    pub async fn reply(&self, answer: &str) -> Result<String, xai_tool_runtime::ToolError> {
        let mut text = answer.trim().to_string();
        if !text.to_ascii_lowercase().starts_with("[turbo]") {
            text = format!("[Turbo] {text}");
        }
        let store = self.active_or_current_store()?;
        store.write_last_reply(&text).map_err(|e| {
            xai_tool_runtime::ToolError::custom("meeting_store", e.to_string())
        })?;
        let mut lines = vec![format!("Saved {}", store.last_reply_path().display())];

        // Prefer the notetaker's own guest identity. This is what lets chat
        // Q&A work with no Graph token and without answering as the operator.
        // The guard is scoped so it is never held across an await.
        let bot = {
            lock_live()
                .get(&self.inner.session_id)
                .and_then(|live| live.bot.clone())
        };
        let mut posted = false;
        if let Some(bot) = bot {
            let state = bot.state();
            if state == xai_grok_meeting_bot::BotState::Admitted {
                match bot.post_chat(&text).await {
                    Ok(()) => {
                        lines.push("Posted to meeting chat as Turbo (Notetaker).".into());
                        posted = true;
                    }
                    Err(e) => lines.push(format!("Notetaker chat post failed: {e}")),
                }
            } else {
                lines.push(format!("Notetaker is {} — did not post.", state.label()));
            }
        }

        if !posted {
            if let Some(token) = graph::graph_token() {
                let url = store.read_meta().ok().map(|m| m.url).unwrap_or_default();
                match graph::chat_id_for_join_url(&token, &url).await {
                    Ok(chat_id) => match graph::post_chat(&token, &chat_id, &text).await {
                        Ok(()) => lines.push("Posted to Teams meeting chat as you.".into()),
                        Err(e) => lines.push(format!("Graph post failed: {e}")),
                    },
                    Err(e) => lines.push(format!("Graph chat id failed: {e}")),
                }
            } else {
                lines.push(
                    "No notetaker in the meeting and GROK_GRAPH_TOKEN not set — paste the [Turbo] line into Teams chat yourself."
                        .into(),
                );
            }
        }
        lines.push(text);
        Ok(lines.join("\n"))
    }

    pub fn write_notes(
        &self,
        markdown: &str,
        workspace: Option<&std::path::Path>,
    ) -> Result<String, xai_tool_runtime::ToolError> {
        let store = self.active_or_current_store()?;
        let mut meta = store.read_meta().map_err(|e| {
            xai_tool_runtime::ToolError::custom("meeting_store", e.to_string())
        })?;
        let title = extract_title_from_markdown(markdown)
            .or_else(|| meta.title.clone())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| default_meeting_title(meta.platform));
        if meta.title.as_deref() != Some(title.as_str()) {
            let _ = store.set_title(&title);
            meta.title = Some(title.clone());
        }
        let date = local_date_stamp(meta.started_at);
        let doc = compose_summary_markdown(
            &title,
            &date,
            meta.platform.label(),
            meta.capture_source,
            markdown,
        );
        store.write_notes(&doc).map_err(|e| {
            xai_tool_runtime::ToolError::custom("meeting_store", e.to_string())
        })?;
        let mut lines = vec![format!("Session notes: {}", store.notes_path().display())];
        if let Some(ws) = workspace {
            let dir = workspace_meetings_dir(ws);
            let dest = if let Some(existing) = meta
                .workspace_summary_path
                .as_deref()
                .map(std::path::PathBuf::from)
            {
                if xai_grok_meetings::recap_dest_is_safe(&dir, &existing) {
                    existing
                } else {
                    unique_summary_path(&dir, &summary_filename(&date, &title))
                }
            } else {
                unique_summary_path(&dir, &summary_filename(&date, &title))
            };
            if !xai_grok_meetings::recap_dest_is_safe(&dir, &dest) {
                return Err(xai_tool_runtime::ToolError::custom(
                    "meeting_summary",
                    "refusing to write recap outside the workspace Meetings folder",
                ));
            }
            write_workspace_summary(&dest, &doc).map_err(|e| {
                xai_tool_runtime::ToolError::custom("meeting_summary", e.to_string())
            })?;
            let _ = store.set_workspace_summary_path(&dest);
            lines.push(format!("Work folder summary: {}", dest.display()));
        } else {
            lines.push(
                "Work folder unavailable — summary saved next to the transcript only.".into(),
            );
        }
        Ok(lines.join("\n"))
    }

    fn active_or_current_store(&self) -> Result<MeetingStore, xai_tool_runtime::ToolError> {
        if let Some(live) = lock_live().get(&self.inner.session_id) {
            return Ok(live.store.clone());
        }
        let folder = self.folder()?;
        let id = read_current_id(folder).ok().flatten().ok_or_else(|| {
            xai_tool_runtime::ToolError::custom(
                "meeting_idle",
                "no meeting transcript in this session — /meeting join <url> first",
            )
        })?;
        MeetingStore::open(meeting_dir(folder, &id))
            .map(|(s, _)| s)
            .map_err(|e| xai_tool_runtime::ToolError::custom("meeting_store", e.to_string()))
    }
}

fn graph_status_label() -> &'static str {
    if graph::graph_token().is_some() {
        "configured"
    } else {
        "missing"
    }
}

fn format_status_line(meta: &MeetingMeta, live: bool) -> String {
    if live {
        "recording".into()
    } else if meta.status == MeetingStatus::Recording {
        "stale recording — capture is not running; meeting_notes can still recap".into()
    } else {
        format!("{:?}", meta.status).to_ascii_lowercase()
    }
}

/// Everything `meeting_join` reports back, in one place.
///
/// Extracted from [`MeetingHandle::join`] so the honesty contract is testable:
/// under `cfg!(test)` `choose_capture_source` returns `None`, so the bot path
/// is unreachable in-process and asserting on a real `join()` could never cover
/// a *failed guest join* — precisely the case the field incident was about.
struct JoinSummary<'a> {
    platform: xai_grok_meetings::MeetingPlatform,
    kind: xai_grok_meetings::MeetingKind,
    id: &'a str,
    title: Option<&'a str>,
    /// Already passed through `redact_join_secrets`.
    redacted_url: &'a str,
    transcript: &'a str,
    source: CaptureSource,
    outcome: &'a NotetakerOutcome,
    bot_state: Option<&'a xai_grok_meeting_bot::BotState>,
    /// `None` when the link was deliberately not handed to the OS.
    shell_open: Option<bool>,
    fallback_note: Option<&'a str>,
    loopback_only: bool,
}

impl JoinSummary<'_> {
    fn render(&self) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let guest_failed = matches!(self.outcome, NotetakerOutcome::Failed { .. });
        if guest_failed {
            // First line, not seventh of eight. The operator asked for a guest
            // in the lobby; a transcript of their own speakers is a different
            // feature and must not read as the one they asked for.
            lines.push(self.outcome.headline());
        }
        lines.extend([
            if guest_failed {
                format!("Local recording started ({})", self.platform.label())
            } else {
                format!("Notetaker started ({})", self.platform.label())
            },
            format!("id: {}", self.id),
            format!(
                "name: {}",
                self.title.unwrap_or("(will use Graph subject or recap title)")
            ),
            format!("url: {}", self.redacted_url),
            format!("capture: {:?} — {}", self.source, self.source.describe()),
            format!("transcript: {}", self.transcript),
        ]);
        if self.source != CaptureSource::None {
            lines.push(
                "stt: captured audio is uploaded to xAI hosted STT \
                 (default wss://api.x.ai/v1/stt; override [voice].api_base)"
                    .into(),
            );
        }
        if self.kind == xai_grok_meetings::MeetingKind::Webinar {
            lines.push("note: webinar links often block attendee chat; v1 is notes-only.".into());
        }
        if self.shell_open == Some(false) {
            lines.push("could not auto-open the link — paste it in Teams/Zoom yourself.".into());
        }
        if let Some(state) = self.bot_state {
            lines.push(transport::bot_status_line(state));
            lines.push(
                "a participant named \"Turbo (Notetaker)\" is joining. Teams holds detected \
                 notetakers in the lobby — admit it to start notes."
                    .into(),
            );
        }
        if self.source == CaptureSource::Microphone {
            lines.push(
                "capturing the default microphone (loopback unavailable). Remote speakers may be missed on a headset.".into(),
            );
        }
        if self.source == CaptureSource::Loopback {
            if self.loopback_only {
                lines.push(
                    "capturing system playback (all participants); microphone not mixed (GROK_MEETING_CAPTURE=loopback).".into(),
                );
            } else {
                lines.push(
                    "capturing system playback (all participants) mixed with the microphone.".into(),
                );
            }
        }
        if self.source == CaptureSource::None {
            lines.push("audio capture disabled (GROK_MEETING_NO_CAPTURE or test).".into());
        }
        // Last, so the message neither opens nor closes on the wrong feature.
        if let Some(note) = self.fallback_note {
            lines.push(note.to_string());
        }
        lines
    }
}

/// One line describing whether a guest is actually in the meeting.
///
/// Read from `meta.json`, not from live state, so it survives a restart and so
/// `meeting_join`, `meeting_status` and `meeting_stop` cannot disagree.
fn format_notetaker_line(meta: &MeetingMeta) -> String {
    match &meta.notetaker {
        Some(o) => o.headline(),
        // Recorded before this field existed. Say so rather than guess.
        None => "(not recorded — meeting predates notetaker outcome tracking)".to_string(),
    }
}

fn format_meta(meta: &MeetingMeta, live_source: Option<CaptureSource>, store: &MeetingStore) -> String {
    let live = live_source.is_some();
    let source = live_source.unwrap_or(meta.capture_source);
    format!(
        "id: {}\nstatus: {}\nname: {}\nplatform: {}\nkind: {:?}\ncapture: {:?}\nnotetaker: {}\nurl: {}\ngraph: {}\nstarted: {}\nstopped: {}\nfinal_segments: {}\ntranscript: {}\nnotes: {}\nwork_summary: {}\nqa: launch workspace + meeting notes (read-only tools; meeting text is untrusted)\nextra_notes: {}\npending_turbo_questions: {}",
        meta.id,
        format_status_line(meta, live),
        meta.title.as_deref().unwrap_or("(untitled)"),
        meta.platform.label(),
        meta.kind,
        source,
        format_notetaker_line(meta),
        redact_join_secrets(&meta.url),
        graph_status_label(),
        meta.started_at.to_rfc3339(),
        meta.stopped_at
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| "-".into()),
        meta.final_segments,
        store.transcript_path().display(),
        store.notes_path().display(),
        meta.workspace_summary_path.as_deref().unwrap_or("(not written yet)"),
        meta.knowledge_dir.as_deref().unwrap_or("(none)"),
        store.pending_question_count(),
    )
}

async fn require_handle(
    ctx: &xai_tool_runtime::ToolCallContext,
) -> Result<MeetingHandle, xai_tool_runtime::ToolError> {
    use crate::types::tool_metadata::shared_resources;
    let resources = shared_resources(ctx)?;
    let res = resources.lock().await;
    res.require::<MeetingHandle>().map(|h| h.clone())
}

fn dynamic_tool_input(value: &impl serde::Serialize) -> crate::types::tool_io::ToolInput {
    crate::types::tool_io::ToolInput::Dynamic(
        serde_json::to_value(value).unwrap_or(serde_json::Value::Null),
    )
}

impl From<join::MeetingJoinInput> for crate::types::tool_io::ToolInput {
    fn from(input: join::MeetingJoinInput) -> Self {
        dynamic_tool_input(&input)
    }
}

impl From<stop::MeetingStopInput> for crate::types::tool_io::ToolInput {
    fn from(input: stop::MeetingStopInput) -> Self {
        dynamic_tool_input(&input)
    }
}

impl From<status::MeetingStatusInput> for crate::types::tool_io::ToolInput {
    fn from(input: status::MeetingStatusInput) -> Self {
        dynamic_tool_input(&input)
    }
}

impl From<transcript::MeetingTranscriptInput> for crate::types::tool_io::ToolInput {
    fn from(input: transcript::MeetingTranscriptInput) -> Self {
        dynamic_tool_input(&input)
    }
}

impl From<notes::MeetingNotesInput> for crate::types::tool_io::ToolInput {
    fn from(input: notes::MeetingNotesInput) -> Self {
        dynamic_tool_input(&input)
    }
}

impl From<knowledge::MeetingKnowledgeInput> for crate::types::tool_io::ToolInput {
    fn from(input: knowledge::MeetingKnowledgeInput) -> Self {
        dynamic_tool_input(&input)
    }
}

impl From<ask::MeetingAskInput> for crate::types::tool_io::ToolInput {
    fn from(input: ask::MeetingAskInput) -> Self {
        dynamic_tool_input(&input)
    }
}

impl From<reply::MeetingReplyInput> for crate::types::tool_io::ToolInput {
    fn from(input: reply::MeetingReplyInput) -> Self {
        dynamic_tool_input(&input)
    }
}

fn text_output(text: String) -> ToolOutput {
    ToolOutput::Text(text.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use xai_grok_meetings::parse_meeting_url;

    /// Joining takes seconds; a second join must not launch a second browser
    /// into the same meeting while the first is still in the lobby.
    #[test]
    fn join_reservation_is_exclusive_per_session() {
        let session = unique("sess-reserve");
        let first = JoinReservation::acquire(&session).expect("first join reserves");
        assert!(
            JoinReservation::acquire(&session).is_none(),
            "a concurrent join in the same session must be refused"
        );
        // A different session is unaffected.
        let other = unique("sess-reserve-other");
        assert!(JoinReservation::acquire(&other).is_some());

        drop(first);
        assert!(
            JoinReservation::acquire(&session).is_some(),
            "the slot must free once the join returns"
        );
    }

    fn unique(tag: &str) -> String {
        let ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{tag}-{}-{ns}", std::process::id())
    }

    fn temp_session(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(unique(tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn secret_url() -> xai_grok_meetings::MeetingUrl {
        parse_meeting_url("https://teams.microsoft.com/meet/2907709513066?p=secret").unwrap()
    }

    fn insert_live(session_id: &str, store: MeetingStore) {
        lock_live().insert(
            session_id.to_string(),
            LiveMeeting {
                store,
                capture_source: CaptureSource::None,
                stop: Arc::new(AtomicBool::new(false)),
                _task: tokio::spawn(async {}),
                _watch: tokio::spawn(async {}),
                capture: None,
                bot: None,
                ingress: None,
            },
        );
    }

    fn summary<'a>(
        source: CaptureSource,
        outcome: &'a NotetakerOutcome,
        fallback_note: Option<&'a str>,
    ) -> JoinSummary<'a> {
        JoinSummary {
            platform: xai_grok_meetings::MeetingPlatform::Teams,
            kind: xai_grok_meetings::MeetingKind::Meeting,
            id: "teams-1",
            title: Some("Weekly standup"),
            redacted_url: "https://teams.microsoft.com/meet/1",
            transcript: "C:/x/transcript.jsonl",
            source,
            outcome,
            bot_state: None,
            shell_open: None,
            fallback_note,
            loopback_only: false,
        }
    }

    /// The field incident in one assertion. The operator asked for a guest in
    /// the lobby that answers questions in chat; they got a loopback recording
    /// whose report opened with "Notetaker started" and closed with reassurance
    /// about system playback. The one honest sentence was seventh of eight.
    #[test]
    fn failed_join_leads_with_no_guest_and_names_the_reason() {
        let outcome = NotetakerOutcome::Failed {
            stage: xai_grok_meetings::JoinFailureStage::LauncherHandoff,
            detail: "Teams app launcher".into(),
        };
        // Derived from the real reason, not hardcoded, so the two cannot drift.
        let reason = transport::FallbackReason::JoinFailed {
            stage: xai_grok_meetings::JoinFailureStage::LauncherHandoff,
            detail: "Teams app launcher".into(),
        };
        let note = reason.line();
        assert_eq!(reason.outcome(), outcome, "the two must describe one event");
        let lines = summary(CaptureSource::Loopback, &outcome, Some(&note)).render();

        let first = &lines[0];
        assert!(first.contains("NO GUEST IN THE MEETING"), "{first}");
        assert!(first.contains("Teams app launcher"), "must name why: {first}");
        assert!(
            first.to_lowercase().contains("q&a"),
            "must name the feature that is not running: {first}"
        );
        assert!(
            !lines[1].starts_with("Notetaker started"),
            "a failed guest join must not announce a notetaker: {}",
            lines[1]
        );
        // And it must not end on reassurance about the wrong feature.
        let last = lines.last().unwrap();
        assert!(
            last.contains("no participant joins the meeting"),
            "last line was {last:?}"
        );
        assert!(
            !last.contains("capturing system playback"),
            "the message must not close on the fallback sounding healthy"
        );
    }

    /// A real guest join keeps the old, reassuring shape.
    #[test]
    fn successful_join_does_not_shout_about_a_missing_guest() {
        let outcome = NotetakerOutcome::Joined;
        let lines = summary(CaptureSource::MeetingBot, &outcome, None).render();
        assert!(lines[0].starts_with("Notetaker started"), "{:?}", lines[0]);
        assert!(
            !lines.iter().any(|l| l.contains("NO GUEST")),
            "{lines:#?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("wss://api.x.ai/v1/stt")),
            "join output must name the remote STT destination: {lines:#?}"
        );
    }

    /// The auto-open line only appears when an open was actually attempted and
    /// failed — never when the bot deliberately kept the link to itself.
    #[test]
    fn the_auto_open_warning_only_speaks_when_an_open_was_tried() {
        let outcome = NotetakerOutcome::Joined;
        let warning = "could not auto-open";

        let mut bot = summary(CaptureSource::MeetingBot, &outcome, None);
        bot.shell_open = None;
        assert!(
            !bot.render().iter().any(|l| l.contains(warning)),
            "no open was attempted, so there is nothing to warn about"
        );

        let mut ok = summary(CaptureSource::Loopback, &outcome, None);
        ok.shell_open = Some(true);
        assert!(!ok.render().iter().any(|l| l.contains(warning)));

        let mut failed = summary(CaptureSource::Loopback, &outcome, None);
        failed.shell_open = Some(false);
        assert!(failed.render().iter().any(|l| l.contains(warning)));
    }

    /// Not-attempted is a different thing from attempted-and-failed: the
    /// operator disabling the bot, or joining a Zoom call, is not an incident.
    #[test]
    fn a_platform_without_a_bot_is_not_reported_as_a_failure() {
        let outcome = NotetakerOutcome::NotAttempted {
            why: "no joined notetaker for Zoom yet".into(),
        };
        let note = "no joined notetaker for Zoom yet — capturing this machine's audio instead; \
                    no participant joins the meeting.";
        let lines = summary(CaptureSource::Loopback, &outcome, Some(note)).render();
        assert!(lines[0].starts_with("Notetaker started"), "{:?}", lines[0]);
        assert!(!lines[0].contains("NO GUEST"), "{:?}", lines[0]);
        // The existing honesty line still lands, and still lands last.
        assert!(
            lines.last().unwrap().contains("no participant joins the meeting"),
            "{lines:#?}"
        );
    }

    /// `meta.json` written by an older build has no outcome. Say so rather
    /// than let the absence read as "a guest joined".
    #[test]
    fn a_meeting_without_a_recorded_outcome_says_so() {
        let line = format_notetaker_line(&meta_fixture(None));
        assert!(line.contains("not recorded"), "{line}");
        assert!(!line.contains("NO GUEST"), "{line}");

        let joined = format_notetaker_line(&meta_fixture(Some(NotetakerOutcome::Joined)));
        assert!(joined.contains("Notetaker"), "{joined}");
    }

    fn meta_fixture(notetaker: Option<NotetakerOutcome>) -> MeetingMeta {
        MeetingMeta {
            id: "teams-1".into(),
            url: "https://teams.microsoft.com/meet/1".into(),
            platform: xai_grok_meetings::MeetingPlatform::Teams,
            kind: xai_grok_meetings::MeetingKind::Meeting,
            capture_source: CaptureSource::Loopback,
            status: MeetingStatus::Recording,
            started_at: chrono::Utc::now(),
            stopped_at: None,
            final_segments: 0,
            knowledge_dir: None,
            title: None,
            workspace_summary_path: None,
            notetaker,
        }
    }

    #[test]
    fn join_tool_id() {
        assert_eq!(
            xai_tool_runtime::Tool::id(&MeetingJoinTool).as_str(),
            "meeting_join"
        );
        assert_eq!(
            crate::types::tool_metadata::ToolMetadata::kind(&MeetingJoinTool),
            crate::types::tool::ToolKind::Meeting
        );
    }

    #[tokio::test]
    async fn live_drop_marks_disk_stopped() {
        let root = temp_session("meet-drop");
        let store = MeetingStore::create(&root, "teams-drop-1", &secret_url(), CaptureSource::None)
            .unwrap();
        write_current(&root, "teams-drop-1").unwrap();
        let session = unique("sess-drop");
        insert_live(&session, store.clone());
        assert_eq!(store.read_meta().unwrap().status, MeetingStatus::Recording);
        let _ = stop_live_session(&session);
        let meta = store.read_meta().unwrap();
        assert_eq!(meta.status, MeetingStatus::Stopped);
        assert!(meta.stopped_at.is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn handle_drop_does_not_stop_live_recording() {
        let root = temp_session("meet-handle-drop");
        let store = MeetingStore::create(&root, "teams-keep-1", &secret_url(), CaptureSource::None)
            .unwrap();
        write_current(&root, "teams-keep-1").unwrap();
        let session = unique("sess-keep");
        insert_live(&session, store.clone());
        {
            let handle = MeetingHandle::new(session.clone(), Some(root.clone()), None, None);
            drop(handle);
        }
        assert_eq!(store.read_meta().unwrap().status, MeetingStatus::Recording);
        assert!(lock_live().contains_key(&session));
        let _ = stop_live_session(&session);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stale_disk_recording_is_not_live_and_redacts_passcode() {
        let root = temp_session("meet-stale");
        let store = MeetingStore::create(&root, "teams-stale-1", &secret_url(), CaptureSource::None)
            .unwrap();
        write_current(&root, "teams-stale-1").unwrap();
        let created = store.read_meta().unwrap();
        assert!(!created.url.contains("secret"), "{}", created.url);
        assert!(!created.url.contains("p="), "{}", created.url);
        let handle = MeetingHandle::new(unique("sess-stale"), Some(root.clone()), None, None);
        let text = handle.status_text().unwrap();
        assert!(
            text.contains("stale recording"),
            "disk leftover must not claim live Recording: {text}"
        );
        assert!(
            !text.to_ascii_lowercase().contains("status: recording\n"),
            "must not claim live recording: {text}"
        );
        assert!(!text.contains("secret"), "{text}");
        assert!(!text.contains("p=secret"), "{text}");
        assert!(
            text.contains("graph: configured") || text.contains("graph: missing"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
