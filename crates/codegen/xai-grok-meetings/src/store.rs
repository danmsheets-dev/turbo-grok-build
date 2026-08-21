//! On-disk transcript + notes for one meeting under the session folder.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::url::{MeetingKind, MeetingPlatform, MeetingUrl};

/// Where audio is coming from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSource {
    /// WASAPI loopback / system mix (closest to Fathom).
    Loopback,
    /// Default microphone (fallback when loopback is unavailable).
    Microphone,
    /// Tests / `GROK_MEETING_NO_CAPTURE=1`.
    None,
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
            url: url.raw.clone(),
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
            meta.final_segments = meta.final_segments.saturating_add(1);
            self.write_meta(&meta)?;
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
        let url = parse("https://teams.microsoft.com/l/meetup-join/x").unwrap();
        let store = MeetingStore::create(&root, "teams-1", &url, CaptureSource::Microphone).unwrap();
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
