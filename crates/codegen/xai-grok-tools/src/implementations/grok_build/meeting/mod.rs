//! Fathom-style meeting notetaker tools (`meeting_join` / `stop` / `status` /
//! `transcript` / `notes`).
//!
//! v1: open the join URL, capture WASAPI loopback + mic on Windows (mic
//! fallback), stream to Grok STT, persist transcript + notes, auto-answer
//! `Turbo:` questions, and write a work-only recap on stop.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use xai_grok_meetings::{
    CaptureSource, MeetingMeta, MeetingStore, briefing, compose_summary_markdown,
    default_meeting_title, extract_title_from_markdown, local_date_stamp, meeting_dir,
    new_meeting_id, parse_meeting_url, read_current_id, read_knowledge_dir, summary_filename,
    unique_summary_path, workspace_meetings_dir, write_current, write_knowledge_dir,
    write_workspace_summary,
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
mod watch;

pub mod join;

pub use ask::{MEETING_ASK_TOOL_NAME, MeetingAskInput, MeetingAskTool};
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
}

static LIVE: OnceLock<Mutex<HashMap<String, LiveMeeting>>> = OnceLock::new();

fn live_map() -> &'static Mutex<HashMap<String, LiveMeeting>> {
    LIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_live() -> std::sync::MutexGuard<'static, HashMap<String, LiveMeeting>> {
    live_map().lock().unwrap_or_else(|e| e.into_inner())
}

/// Session resource so tools can start/stop the notetaker.
#[derive(Clone)]
pub struct MeetingHandle {
    session_id: String,
    session_folder: Option<PathBuf>,
    api_key_provider: Option<SharedApiKeyProvider>,
    notification: Option<ToolNotificationHandle>,
}

impl MeetingHandle {
    pub fn new(
        session_id: impl Into<String>,
        session_folder: Option<PathBuf>,
        api_key_provider: Option<SharedApiKeyProvider>,
        notification: Option<ToolNotificationHandle>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            session_folder,
            api_key_provider,
            notification,
        }
    }

    pub fn unbound() -> Self {
        Self {
            session_id: String::new(),
            session_folder: None,
            api_key_provider: None,
            notification: None,
        }
    }

    fn folder(&self) -> Result<&PathBuf, xai_tool_runtime::ToolError> {
        self.session_folder.as_ref().ok_or_else(|| {
            xai_tool_runtime::ToolError::custom(
                "meeting_no_session",
                "meeting_* tools need a session folder",
            )
        })
    }

    fn voice_auth(&self) -> Result<SharedVoiceAuth, xai_tool_runtime::ToolError> {
        if let Some(p) = &self.api_key_provider {
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
        if self.session_id.trim().is_empty() {
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
            if map.contains_key(&self.session_id) {
                return Err(xai_tool_runtime::ToolError::custom(
                    "meeting_busy",
                    "a meeting is already being recorded in this session — /meeting stop first",
                ));
            }
        }

        let opened = open::open_meeting_url(&parsed.raw);
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
        let (task, capture, source) = if intended == CaptureSource::None {
            (tokio::spawn(async {}), None, CaptureSource::None)
        } else {
            let (task, cap, actual) =
                self.spawn_capture(store.clone(), stop.clone(), intended)?;
            if actual != intended {
                let _ = store.set_capture_source(actual);
            }
            (task, Some(cap), actual)
        };
        let join_url = parsed.raw.clone();
        let watch_store = store.clone();
        let watch_stop = stop.clone();
        let watch_notif = self.notification.clone();
        let watch = tokio::spawn(async move {
            watch::run_watch(watch_store, join_url, watch_stop, watch_notif).await;
        });

        lock_live().insert(
            self.session_id.clone(),
            LiveMeeting {
                store: store.clone(),
                capture_source: source,
                stop,
                _task: task,
                _watch: watch,
                capture,
            },
        );

        let mut lines = vec![
            format!(
                "Notetaker started ({})",
                parsed.platform.label()
            ),
            format!("id: {id}"),
            format!(
                "name: {}",
                title.as_deref().unwrap_or("(will use Graph subject or recap title)")
            ),
            format!("url: {}", parsed.raw),
            format!("capture: {:?}", source),
            format!("transcript: {}", store.transcript_path().display()),
        ];
        if parsed.kind == xai_grok_meetings::MeetingKind::Webinar {
            lines.push(
                "note: webinar links often block attendee chat; v1 is notes-only.".into(),
            );
        }
        if !opened {
            lines.push(
                "could not auto-open the link — paste it in Teams/Zoom yourself.".into(),
            );
        }
        if source == CaptureSource::Microphone {
            lines.push(
                "capturing the default microphone (loopback unavailable). Remote speakers may be missed on a headset.".into(),
            );
        }
        if source == CaptureSource::Loopback {
            if pipeline::capture_pref_from_env() == pipeline::CapturePref::LoopbackOnly {
                lines.push(
                    "capturing system playback (all participants); microphone not mixed (GROK_MEETING_CAPTURE=loopback).".into(),
                );
            } else {
                lines.push(
                    "capturing system playback (all participants) mixed with the microphone.".into(),
                );
            }
        }
        if source == CaptureSource::None {
            lines.push("audio capture disabled (GROK_MEETING_NO_CAPTURE or test).".into());
        }
        Ok(lines.join("\n"))
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

        let stt_notif = self.notification.clone();
        let task = tokio::spawn(async move {
            pipeline::run_stt_loop(store, auth, config, pcm_rx, stop, stt_notif).await;
        });
        Ok((task, capture, actual))
    }

    pub fn stop(&self) -> Result<String, xai_tool_runtime::ToolError> {
        let live = lock_live().remove(&self.session_id);
        let Some(live) = live else {
            return Err(xai_tool_runtime::ToolError::custom(
                "meeting_idle",
                "no active meeting recording in this session",
            ));
        };
        live.stop.store(true, Ordering::Release);
        if let Some(cap) = live.capture {
            cap.stop();
        }
        live._task.abort();
        live._watch.abort();
        let meta = live.store.mark_stopped().map_err(|e| {
            xai_tool_runtime::ToolError::custom("meeting_store", e.to_string())
        })?;
        // Keep current.txt so /meeting notes can still write the work-folder summary.
        Ok(format!(
            "Notetaker stopped.\nid: {}\nname: {}\nsegments: {}\ntranscript: {}\nNext: write a work-only recap with meeting_notes (saved as Meetings/YYYY-MM-DD - <name>.md in the work folder).",
            meta.id,
            meta.title.as_deref().unwrap_or("(untitled)"),
            meta.final_segments,
            live.store.transcript_path().display(),
        ))
    }

    pub fn status_text(&self) -> Result<String, xai_tool_runtime::ToolError> {
        if let Some(live) = lock_live().get(&self.session_id) {
            let meta = live.store.read_meta().map_err(|e| {
                xai_tool_runtime::ToolError::custom("meeting_store", e.to_string())
            })?;
            return Ok(format_meta(&meta, Some(live.capture_source), &live.store));
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
                "GROK_GRAPH_TOKEN not set — paste the [Turbo] line into Teams chat, or set a delegated Graph token (Chat.ReadWrite)."
                    .into(),
            );
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
        let doc = compose_summary_markdown(&title, &date, meta.platform.label(), markdown);
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
        if let Some(live) = lock_live().get(&self.session_id) {
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

fn format_meta(meta: &MeetingMeta, live_source: Option<CaptureSource>, store: &MeetingStore) -> String {
    let source = live_source.unwrap_or(meta.capture_source);
    format!(
        "id: {}\nstatus: {:?}\nname: {}\nplatform: {}\nkind: {:?}\ncapture: {:?}\nurl: {}\nstarted: {}\nstopped: {}\nfinal_segments: {}\ntranscript: {}\nnotes: {}\nwork_summary: {}\nqa: launch workspace + meeting notes (full tools, including MCP)\nextra_notes: {}\npending_turbo_questions: {}",
        meta.id,
        meta.status,
        meta.title.as_deref().unwrap_or("(untitled)"),
        meta.platform.label(),
        meta.kind,
        source,
        meta.url,
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

    #[test]
    fn join_tool_id() {
        assert_eq!(
            xai_tool_runtime::Tool::id(&MeetingJoinTool).as_str(),
            "meeting_join"
        );
    }
}
