//! On-disk transcript + notes for one meeting under the session folder.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::url::{MeetingKind, MeetingPlatform, MeetingUrl, redact_join_secrets};

/// Where audio is coming from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSource {
    /// WASAPI loopback / system mix (local capture of this machine).
    Loopback,
    /// Default microphone (fallback when loopback is unavailable).
    Microphone,
    /// A joined guest notetaker, tapped inside the meeting page.
    ///
    /// Unlike the loopback paths this does not depend on the operator being in
    /// the meeting, or even at the machine.
    MeetingBot,
    /// Tests / `GROK_MEETING_NO_CAPTURE=1`.
    None,
}

impl CaptureSource {
    /// One-line description for join output and `meeting_status`.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Loopback => "system playback (all participants)",
            Self::Microphone => "default microphone",
            Self::MeetingBot => "joined notetaker (meeting audio, no local devices)",
            Self::None => "disabled",
        }
    }

    /// True when audio comes from a participant Turbo joined, not this machine.
    ///
    /// The distinction matters for anything that reasons about the operator's
    /// devices: a bot capture is unaffected by muted speakers, an unplugged
    /// headset, or the operator leaving the meeting.
    pub fn is_bot(self) -> bool {
        matches!(self, Self::MeetingBot)
    }
}

#[cfg(test)]
mod capture_source_tests {
    use super::CaptureSource;

    #[test]
    fn only_the_bot_source_is_independent_of_local_devices() {
        assert!(CaptureSource::MeetingBot.is_bot());
        for local in [
            CaptureSource::Loopback,
            CaptureSource::Microphone,
            CaptureSource::None,
        ] {
            assert!(!local.is_bot(), "{local:?} records this machine");
        }
    }

    #[test]
    fn every_source_describes_itself_for_join_output() {
        for s in [
            CaptureSource::Loopback,
            CaptureSource::Microphone,
            CaptureSource::MeetingBot,
            CaptureSource::None,
        ] {
            assert!(!s.describe().is_empty(), "{s:?} needs a description");
        }
        assert!(
            CaptureSource::MeetingBot.describe().contains("no local devices"),
            "the bot line must say local devices are not involved"
        );
    }

    /// `meta.json` is durable; renaming a variant would orphan old meetings.
    #[test]
    fn capture_source_wire_names_are_stable() {
        let json = serde_json::to_string(&CaptureSource::MeetingBot).unwrap();
        assert_eq!(json, "\"meeting_bot\"");
        assert_eq!(
            serde_json::from_str::<CaptureSource>("\"loopback\"").unwrap(),
            CaptureSource::Loopback
        );
    }
}

/// Which step of the guest join failed.
///
/// Durable in `meta.json`, so the wire names are frozen: renaming a variant
/// orphans meetings recorded by an older build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinFailureStage {
    /// No Chromium-family browser to run the notetaker in.
    NoBrowser,
    /// Teams served the desktop-app launcher page instead of a web join screen.
    LauncherHandoff,
    /// A pre-join element could not be found; the Teams DOM moved.
    Selector,
    /// The page never reached any screen we recognise.
    JoinTimeout,
    /// We sat in the lobby and nobody admitted the notetaker.
    LobbyTimeout,
    /// The organizer refused or removed the notetaker.
    Denied,
    /// A human-verification challenge, which Turbo never answers.
    Verification,
    /// The meeting admits only signed-in participants.
    SignInRequired,
    /// The loopback audio sink for the notetaker could not be set up.
    Audio,
    /// Driving the browser failed.
    Browser,
}

impl JoinFailureStage {
    /// Short operator-facing label.
    pub fn label(self) -> &'static str {
        match self {
            Self::NoBrowser => "no browser",
            Self::LauncherHandoff => "Teams app launcher",
            Self::Selector => "Teams UI changed",
            Self::JoinTimeout => "join timed out",
            Self::LobbyTimeout => "not admitted",
            Self::Denied => "denied",
            Self::Verification => "verification required",
            Self::SignInRequired => "sign-in required",
            Self::Audio => "audio setup failed",
            Self::Browser => "browser error",
        }
    }

    /// Every stage, for exhaustiveness tests.
    pub const ALL: &'static [Self] = &[
        Self::NoBrowser,
        Self::LauncherHandoff,
        Self::Selector,
        Self::JoinTimeout,
        Self::LobbyTimeout,
        Self::Denied,
        Self::Verification,
        Self::SignInRequired,
        Self::Audio,
        Self::Browser,
    ];
}

/// What became of the guest notetaker for this meeting.
///
/// The operator asked for a participant in the lobby that can answer questions
/// in meeting chat. Local capture is a *different* feature that happens to
/// produce a transcript, so a meeting that fell back must never read as one
/// that succeeded. This is the single value `meeting_join`, `meeting_status`
/// and `meeting_stop` all render, so the three cannot disagree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum NotetakerOutcome {
    /// No guest was dispatched at all.
    NotAttempted {
        /// Why not, in operator language.
        why: String,
    },
    /// A guest reached the meeting: lobby or admitted.
    Joined,
    /// A guest was dispatched and could not get in.
    Failed {
        /// Typed classification, for anything that reasons about the failure.
        stage: JoinFailureStage,
        /// Short reason, already operator-facing.
        detail: String,
    },
}

impl NotetakerOutcome {
    /// True only when a participant named "Turbo (Notetaker)" is in the meeting.
    ///
    /// Anything else means the lobby is empty and notetaker chat Q&A is not
    /// running, however healthy the transcript looks.
    pub fn guest_present(&self) -> bool {
        matches!(self, Self::Joined)
    }

    /// The one line every meeting tool leads with.
    pub fn headline(&self) -> String {
        match self {
            Self::Joined => {
                "guest notetaker \"Turbo (Notetaker)\" is in the meeting".to_string()
            }
            Self::NotAttempted { why } => format!(
                "NO GUEST IN THE MEETING - no notetaker was dispatched ({why}). Nobody is in \
                 the lobby and chat Q&A through the notetaker is unavailable."
            ),
            Self::Failed { detail, .. } => format!(
                "NO GUEST IN THE MEETING - the notetaker could not join ({detail}). Nobody is \
                 in the lobby and chat Q&A through the notetaker is unavailable."
            ),
        }
    }
}

#[cfg(test)]
mod notetaker_outcome_tests {
    use super::{JoinFailureStage, NotetakerOutcome};

    /// `meta.json` is durable; renaming a variant would orphan old meetings.
    #[test]
    fn notetaker_outcome_wire_names_are_stable() {
        let joined = serde_json::to_string(&NotetakerOutcome::Joined).unwrap();
        assert_eq!(joined, "{\"state\":\"joined\"}");
        let failed = serde_json::to_string(&NotetakerOutcome::Failed {
            stage: JoinFailureStage::LauncherHandoff,
            detail: "Teams app launcher".into(),
        })
        .unwrap();
        assert!(failed.contains("\"state\":\"failed\""), "{failed}");
        assert!(failed.contains("\"stage\":\"launcher_handoff\""), "{failed}");
        let back: NotetakerOutcome =
            serde_json::from_str("{\"state\":\"not_attempted\",\"why\":\"disabled\"}").unwrap();
        assert_eq!(back, NotetakerOutcome::NotAttempted { why: "disabled".into() });
    }

    #[test]
    fn only_a_joined_guest_counts_as_present() {
        assert!(NotetakerOutcome::Joined.guest_present());
        assert!(!NotetakerOutcome::NotAttempted { why: "x".into() }.guest_present());
        for stage in JoinFailureStage::ALL {
            let o = NotetakerOutcome::Failed {
                stage: *stage,
                detail: stage.label().to_string(),
            };
            assert!(!o.guest_present(), "{stage:?} is not a guest in the meeting");
        }
    }

    /// The honesty contract: anything short of a joined guest must say so in
    /// words the operator cannot mistake for success.
    #[test]
    fn every_non_joined_outcome_headline_says_no_guest() {
        let mut outcomes = vec![NotetakerOutcome::NotAttempted { why: "GROK_MEETING_BOT=0".into() }];
        for stage in JoinFailureStage::ALL {
            outcomes.push(NotetakerOutcome::Failed {
                stage: *stage,
                detail: stage.label().to_string(),
            });
        }
        for o in &outcomes {
            let line = o.headline();
            assert!(line.contains("NO GUEST IN THE MEETING"), "{line}");
            assert!(line.contains("lobby"), "{line}");
            assert!(
                line.to_lowercase().contains("q&a"),
                "must name the feature that is not running: {line}"
            );
        }
        let ok = NotetakerOutcome::Joined.headline();
        assert!(!ok.contains("NO GUEST"), "{ok}");
        assert!(ok.contains("Notetaker"), "{ok}");
    }

    #[test]
    fn every_stage_has_a_short_label() {
        for stage in JoinFailureStage::ALL {
            let l = stage.label();
            assert!(!l.is_empty() && l.len() < 40, "{stage:?} -> {l:?}");
        }
    }
}

/// Live notetaker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingStatus {
    Recording,
    Stopped,
}

/// Durable snapshot written to `meta.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingMeta {
    pub id: String,
    pub url: String,
    pub platform: MeetingPlatform,
    pub kind: MeetingKind,
    pub capture_source: CaptureSource,
    pub status: MeetingStatus,
    pub started_at: DateTime<Utc>,
    pub stopped_at: Option<DateTime<Utc>>,
    pub final_segments: u32,
    /// Optional extra notes path (not required — Q&A uses the launch workspace).
    #[serde(default)]
    pub knowledge_dir: Option<String>,
    /// Human meeting name (join title, Graph subject, or recap H1).
    #[serde(default)]
    pub title: Option<String>,
    /// Dated summary written into the launch workspace (`Meetings/…`).
    #[serde(default)]
    pub workspace_summary_path: Option<String>,
    /// What became of the guest notetaker.
    ///
    /// `None` for meetings recorded before this field existed, and for the
    /// brief window between `create` and the join deciding an outcome.
    #[serde(default)]
    pub notetaker: Option<NotetakerOutcome>,
}

/// One STT segment (JSONL in `transcript.jsonl`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub at: DateTime<Utc>,
    pub text: String,
    pub is_final: bool,
}

/// Session-scoped meeting files: `{session}/meetings/{id}/`.
#[derive(Debug, Clone)]
pub struct MeetingStore {
    pub dir: PathBuf,
    pub id: String,
}

/// Meeting ids are a single path component (`teams-1710000000`).
pub fn is_safe_meeting_id(id: &str) -> bool {
    let id = id.trim();
    !id.is_empty()
        && id.len() <= 80
        && !id.contains("..")
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl MeetingStore {
    /// Create `{session_folder}/meetings/{id}/` and write initial meta.
    pub fn create(
        session_folder: &Path,
        id: impl Into<String>,
        url: &MeetingUrl,
        capture_source: CaptureSource,
    ) -> std::io::Result<Self> {
        let id = id.into();
        if !is_safe_meeting_id(&id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid meeting id",
            ));
        }
        let dir = session_folder.join("meetings").join(&id);
        fs::create_dir_all(&dir)?;
        let store = Self { dir, id: id.clone() };
        let meta = MeetingMeta {
            id,
            url: redact_join_secrets(&url.raw),
            platform: url.platform,
            kind: url.kind,
            capture_source,
            status: MeetingStatus::Recording,
            started_at: Utc::now(),
            stopped_at: None,
            final_segments: 0,
            knowledge_dir: None,
            title: None,
            workspace_summary_path: None,
            notetaker: None,
        };
        store.write_meta(&meta)?;
        Ok(store)
    }

    /// Open an existing meeting directory (must contain `meta.json`).
    pub fn open(dir: PathBuf) -> std::io::Result<(Self, MeetingMeta)> {
        let meta_path = dir.join("meta.json");
        let raw = fs::read_to_string(&meta_path)?;
        let meta: MeetingMeta = serde_json::from_str(&raw).map_err(std::io::Error::other)?;
        let id = meta.id.clone();
        if !is_safe_meeting_id(&id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid meeting id in meta.json",
            ));
        }
        Ok((Self { dir, id }, meta))
    }

    pub fn meta_path(&self) -> PathBuf {
        self.dir.join("meta.json")
    }

    pub fn transcript_path(&self) -> PathBuf {
        self.dir.join("transcript.jsonl")
    }

    pub fn notes_path(&self) -> PathBuf {
        self.dir.join("notes.md")
    }

    pub fn read_meta(&self) -> std::io::Result<MeetingMeta> {
        let raw = fs::read_to_string(self.meta_path())?;
        serde_json::from_str(&raw).map_err(std::io::Error::other)
    }

    pub fn write_meta(&self, meta: &MeetingMeta) -> std::io::Result<()> {
        let tmp = self.dir.join("meta.json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(meta).map_err(std::io::Error::other)?)?;
        fs::rename(tmp, self.meta_path())
    }

    /// Append a segment. Only `is_final` lines count toward `final_segments`.
    pub fn append_segment(&self, segment: &TranscriptSegment) -> std::io::Result<()> {
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.transcript_path())?;
        serde_json::to_writer(&mut f, segment).map_err(std::io::Error::other)?;
        f.write_all(b"\n")?;
        if segment.is_final {
            let mut meta = self.read_meta()?;
            let next = meta.final_segments.saturating_add(1);
            meta.final_segments = next;
            if meta.status == MeetingStatus::Stopped || meta.stopped_at.is_some() {
                meta.status = MeetingStatus::Stopped;
                self.write_meta(&meta)?;
            } else if let Ok(disk) = self.read_meta() {
                if disk.status == MeetingStatus::Stopped || disk.stopped_at.is_some() {
                    let mut stopped = disk;
                    stopped.final_segments = stopped.final_segments.max(next);
                    self.write_meta(&stopped)?;
                } else {
                    self.write_meta(&meta)?;
                }
            } else {
                self.write_meta(&meta)?;
            }
        }
        Ok(())
    }

    pub fn mark_stopped(&self) -> std::io::Result<MeetingMeta> {
        let mut meta = self.read_meta()?;
        meta.status = MeetingStatus::Stopped;
        meta.stopped_at = Some(Utc::now());
        self.write_meta(&meta)?;
        Ok(meta)
    }

    /// Concatenate final (and, if none, last partial) segment texts.
    pub fn transcript_text(&self) -> std::io::Result<String> {
        let path = self.transcript_path();
        if !path.exists() {
            return Ok(String::new());
        }
        let f = fs::File::open(path)?;
        let mut finals = Vec::new();
        let mut last_partial: Option<String> = None;
        for line in BufReader::new(f).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let seg: TranscriptSegment =
                serde_json::from_str(&line).map_err(std::io::Error::other)?;
            if seg.is_final {
                if !seg.text.trim().is_empty() {
                    finals.push(seg.text);
                }
                last_partial = None;
            } else {
                last_partial = Some(seg.text);
            }
        }
        if let Some(p) = last_partial.filter(|s| !s.trim().is_empty()) {
            finals.push(p);
        }
        Ok(finals.join("\n"))
    }

    pub fn write_notes(&self, markdown: &str) -> std::io::Result<()> {
        fs::write(self.notes_path(), markdown)
    }

    pub fn questions_path(&self) -> PathBuf {
        self.dir.join("questions.jsonl")
    }

    pub fn inbox_path(&self) -> PathBuf {
        self.dir.join("inbox.jsonl")
    }

    pub fn last_reply_path(&self) -> PathBuf {
        self.dir.join("last_reply.md")
    }

    pub fn set_knowledge_dir(&self, dir: &Path) -> std::io::Result<MeetingMeta> {
        let mut meta = self.read_meta()?;
        meta.knowledge_dir = Some(dir.to_string_lossy().into_owned());
        self.write_meta(&meta)?;
        Ok(meta)
    }

    pub fn set_capture_source(&self, source: CaptureSource) -> std::io::Result<MeetingMeta> {
        let mut meta = self.read_meta()?;
        meta.capture_source = source;
        self.write_meta(&meta)?;
        Ok(meta)
    }

    /// Record what became of the guest notetaker.
    ///
    /// Written to `meta.json` rather than held in memory because
    /// `meeting_status` and `meeting_reply` both re-read the store from disk,
    /// and must still tell the truth after a restart.
    pub fn set_notetaker_outcome(
        &self,
        outcome: NotetakerOutcome,
    ) -> std::io::Result<MeetingMeta> {
        let mut meta = self.read_meta()?;
        meta.notetaker = Some(outcome);
        self.write_meta(&meta)?;
        Ok(meta)
    }

    pub fn set_title(&self, title: &str) -> std::io::Result<MeetingMeta> {
        let mut meta = self.read_meta()?;
        meta.title = Some(title.trim().to_string());
        self.write_meta(&meta)?;
        Ok(meta)
    }

    pub fn set_workspace_summary_path(&self, path: &Path) -> std::io::Result<MeetingMeta> {
        let mut meta = self.read_meta()?;
        meta.workspace_summary_path = Some(path.to_string_lossy().into_owned());
        self.write_meta(&meta)?;
        Ok(meta)
    }

    pub fn enqueue_question(&self, from: &str, question: &str) -> std::io::Result<()> {
        let rec = serde_json::json!({
            "at": Utc::now().to_rfc3339(),
            "from": from,
            "question": question,
            "answered": false,
        });
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.questions_path())?;
        serde_json::to_writer(&mut f, &rec).map_err(std::io::Error::other)?;
        f.write_all(b"\n")?;
        Ok(())
    }

    /// Next unanswered question, marked answered on disk.
    pub fn take_next_question(&self) -> std::io::Result<Option<(String, String)>> {
        let path = self.questions_path();
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&path)?;
        let mut lines: Vec<String> = raw.lines().map(|l| l.to_string()).collect();
        let mut taken = None;
        for line in &mut lines {
            if line.trim().is_empty() {
                continue;
            }
            let mut v: serde_json::Value =
                serde_json::from_str(line).map_err(std::io::Error::other)?;
            if v.get("answered").and_then(|x| x.as_bool()) == Some(true) {
                continue;
            }
            let q = v
                .get("question")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let from = v
                .get("from")
                .and_then(|x| x.as_str())
                .unwrap_or("chat")
                .to_string();
            if q.is_empty() {
                continue;
            }
            v["answered"] = serde_json::Value::Bool(true);
            *line = v.to_string();
            taken = Some((from, q));
            break;
        }
        if taken.is_some() {
            fs::write(path, lines.join("\n") + "\n")?;
        }
        Ok(taken)
    }

    /// Mark the first unanswered matching question as answered.
    pub fn mark_question_answered(&self, from: &str, question: &str) -> std::io::Result<bool> {
        let path = self.questions_path();
        if !path.exists() {
            return Ok(false);
        }
        let raw = fs::read_to_string(&path)?;
        let mut lines: Vec<String> = raw.lines().map(|l| l.to_string()).collect();
        let mut found = false;
        for line in &mut lines {
            if line.trim().is_empty() {
                continue;
            }
            let mut v: serde_json::Value =
                serde_json::from_str(line).map_err(std::io::Error::other)?;
            if v.get("answered").and_then(|x| x.as_bool()) == Some(true) {
                continue;
            }
            let q = v.get("question").and_then(|x| x.as_str()).unwrap_or("");
            let f = v.get("from").and_then(|x| x.as_str()).unwrap_or("chat");
            if q == question && f == from {
                v["answered"] = serde_json::Value::Bool(true);
                *line = v.to_string();
                found = true;
                break;
            }
        }
        if found {
            fs::write(path, lines.join("\n") + "\n")?;
        }
        Ok(found)
    }

    pub fn pending_question_count(&self) -> u32 {
        let Ok(raw) = fs::read_to_string(self.questions_path()) else {
            return 0;
        };
        raw.lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|v| v.get("answered").and_then(|x| x.as_bool()) != Some(true))
            .count() as u32
    }

    pub fn write_last_reply(&self, markdown: &str) -> std::io::Result<()> {
        fs::write(self.last_reply_path(), markdown)
    }

    pub fn read_notes(&self) -> std::io::Result<Option<String>> {
        let path = self.notes_path();
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(fs::read_to_string(path)?))
    }
}

/// Pointer to the current recording for a session (`meetings/current.txt`).
pub fn current_pointer_path(session_folder: &Path) -> PathBuf {
    session_folder.join("meetings").join("current.txt")
}

pub fn write_current(session_folder: &Path, meeting_id: &str) -> std::io::Result<()> {
    if !is_safe_meeting_id(meeting_id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid meeting id",
        ));
    }
    let dir = session_folder.join("meetings");
    fs::create_dir_all(&dir)?;
    fs::write(current_pointer_path(session_folder), meeting_id)
}

pub fn clear_current(session_folder: &Path) -> std::io::Result<()> {
    let p = current_pointer_path(session_folder);
    if p.exists() {
        fs::remove_file(p)?;
    }
    Ok(())
}

pub fn read_current_id(session_folder: &Path) -> std::io::Result<Option<String>> {
    let p = current_pointer_path(session_folder);
    if !p.exists() {
        return Ok(None);
    }
    let id = fs::read_to_string(p)?;
    let id = id.trim();
    if id.is_empty() || !is_safe_meeting_id(id) {
        Ok(None)
    } else {
        Ok(Some(id.to_string()))
    }
}

pub fn meeting_dir(session_folder: &Path, id: &str) -> PathBuf {
    session_folder.join("meetings").join(id)
}

/// Stable-enough id: `{platform}-{unix}-{n}`.
pub fn new_meeting_id(platform: MeetingPlatform) -> String {
    let ts = Utc::now().timestamp();
    let slug = match platform {
        MeetingPlatform::Teams => "teams",
        MeetingPlatform::Zoom => "zoom",
        MeetingPlatform::GoogleMeet => "meet",
        MeetingPlatform::Webex => "webex",
        MeetingPlatform::Other => "meeting",
    };
    format!("{slug}-{ts}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::url::parse;
    use std::env;

    #[test]
    fn create_append_stop_roundtrip() {
        let root = env::temp_dir().join(format!("turbo-meeting-store-{}", Utc::now().timestamp_nanos_opt().unwrap_or(1)));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let url = parse("https://teams.microsoft.com/l/meetup-join/x?p=secret").unwrap();
        let store = MeetingStore::create(&root, "teams-1", &url, CaptureSource::Microphone).unwrap();
        let created = store.read_meta().unwrap();
        assert!(!created.url.contains("secret"), "{}", created.url);
        assert!(!created.url.contains("p="), "{}", created.url);
        store
            .append_segment(&TranscriptSegment {
                at: Utc::now(),
                text: "hello".into(),
                is_final: true,
            })
            .unwrap();
        store
            .append_segment(&TranscriptSegment {
                at: Utc::now(),
                text: "partial".into(),
                is_final: false,
            })
            .unwrap();
        assert_eq!(store.transcript_text().unwrap(), "hello\npartial");
        let meta = store.mark_stopped().unwrap();
        assert_eq!(meta.status, MeetingStatus::Stopped);
        assert_eq!(meta.final_segments, 1);
        store.write_notes("# Notes\n").unwrap();
        assert!(store.read_notes().unwrap().unwrap().contains("Notes"));
        write_current(&root, "teams-1").unwrap();
        assert_eq!(read_current_id(&root).unwrap().as_deref(), Some("teams-1"));
        store
            .append_segment(&TranscriptSegment {
                at: Utc::now(),
                text: "late final".into(),
                is_final: true,
            })
            .unwrap();
        let after = store.read_meta().unwrap();
        assert_eq!(after.status, MeetingStatus::Stopped);
        assert!(after.stopped_at.is_some());
        assert_eq!(after.final_segments, 2);
        store
            .enqueue_question("alice", "How is the new website project going")
            .unwrap();
        assert_eq!(store.pending_question_count(), 1);
        let (from, q) = store.take_next_question().unwrap().unwrap();
        assert_eq!(from, "alice");
        assert!(q.contains("website"));
        assert_eq!(store.pending_question_count(), 0);
        store
            .enqueue_question("bob", "Ship the nav?")
            .unwrap();
        assert_eq!(store.pending_question_count(), 1);
        assert!(store.mark_question_answered("bob", "Ship the nav?").unwrap());
        assert_eq!(store.pending_question_count(), 0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_traversal_ids() {
        let root = env::temp_dir().join("turbo-meeting-id-guard");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        assert!(write_current(&root, "..\\outside").is_err());
        fs::create_dir_all(root.join("meetings")).unwrap();
        fs::write(current_pointer_path(&root), "..\\outside").unwrap();
        assert_eq!(read_current_id(&root).unwrap(), None);
        assert!(!is_safe_meeting_id("a/b"));
        assert!(is_safe_meeting_id("teams-1"));
        let _ = fs::remove_dir_all(&root);
    }
}
