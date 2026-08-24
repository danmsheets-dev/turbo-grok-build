//! Work-folder meeting summary: filename, title, and document header.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, Utc};

use crate::store::CaptureSource;
use crate::url::MeetingPlatform;

/// Subfolder of the launch workspace where dated summaries are written.
pub const WORKSPACE_MEETINGS_DIR: &str = "Meetings";

pub fn default_meeting_title(platform: MeetingPlatform) -> String {
    format!("{} meeting", platform.label())
}

/// Local calendar date for the meeting (`YYYY-MM-DD`).
pub fn local_date_stamp(utc: DateTime<Utc>) -> String {
    utc.with_timezone(&Local).format("%Y-%m-%d").to_string()
}

/// Strip characters that cannot appear in a Windows filename.
pub fn sanitize_meeting_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if matches!(
            c,
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '\n' | '\r' | '\t'
        ) {
            out.push(' ');
        } else if c.is_control() {
            continue;
        } else {
            out.push(c);
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if trimmed.is_empty() {
        "Meeting".into()
    } else {
        trimmed.chars().take(80).collect()
    }
}

pub fn summary_filename(date: &str, name: &str) -> String {
    format!("{} - {}.md", date, sanitize_meeting_name(name))
}

pub fn workspace_meetings_dir(workspace: &Path) -> PathBuf {
    workspace.join(WORKSPACE_MEETINGS_DIR)
}

/// First unused path in `dir` for `filename` (`stem-2.md`, `stem-3.md`, …).
pub fn unique_summary_path(dir: &Path, filename: &str) -> PathBuf {
    let primary = dir.join(filename);
    if !primary.exists() {
        return primary;
    }
    let stem = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Meeting");
    for i in 2..1000 {
        let candidate = dir.join(format!("{stem}-{i}.md"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem}-overflow.md"))
}

pub fn extract_title_from_markdown(md: &str) -> Option<String> {
    for line in md.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(rest) = t.strip_prefix('#') {
            let title = rest.trim_start_matches('#').trim();
            if !title.is_empty() {
                return Some(title.to_string());
            }
        }
        break;
    }
    None
}

fn strip_leading_heading(md: &str) -> &str {
    let bytes = md.as_bytes();
    let mut i = 0;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\n' || bytes[i] == b'\r') {
        i += 1;
    }
    let rest = &md[i..];
    if rest.starts_with('#') {
        if let Some(nl) = rest.find('\n') {
            rest[nl + 1..].trim_start_matches(['\r', '\n'])
        } else {
            ""
        }
    } else {
        rest
    }
}

/// Canonical work summary: title + date at the top, then the recap body.
///
/// `capture` is recorded because this file outlives the session. A recap
/// transcribed from one PC's speakers reads exactly like one transcribed from
/// inside the meeting, and months later nobody can tell which they are holding.
pub fn compose_summary_markdown(
    title: &str,
    date: &str,
    platform: &str,
    capture: CaptureSource,
    body: &str,
) -> String {
    let body = strip_leading_heading(body).trim();
    let mut out = format!(
        "# {title}\n\n- Date: {date}\n- Meeting: {title}\n- Platform: {platform}\n- Source: {}\n",
        capture.describe()
    );
    if !body.is_empty() {
        out.push('\n');
        out.push_str(body);
        if !body.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// True when `dest` is a single file directly under `meetings_dir` (no `..`).
pub fn recap_dest_is_safe(meetings_dir: &Path, dest: &Path) -> bool {
    if dest.components().any(|c| matches!(c, std::path::Component::ParentDir))
        || meetings_dir
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return false;
    }
    let Some(name) = dest.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return false;
    }
    dest.parent() == Some(meetings_dir)
}

pub fn write_workspace_summary(path: &Path, markdown: &str) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "recap path has no parent",
        ));
    };
    if parent
        .symlink_metadata()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing to write recap through a symlinked Meetings folder",
        ));
    }
    fs::create_dir_all(parent)?;
    fs::write(path, markdown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    /// The recap outlives the session, so it has to say where the audio came
    /// from. A loopback transcript and a real meeting transcript are otherwise
    /// indistinguishable once the meeting folder is gone.
    #[test]
    fn recap_records_where_the_audio_came_from() {
        let bot = compose_summary_markdown("M", "2026-08-21", "Teams", CaptureSource::MeetingBot, "");
        assert!(
            bot.contains(CaptureSource::MeetingBot.describe()),
            "{bot}"
        );
        let local = compose_summary_markdown("M", "2026-08-21", "Teams", CaptureSource::Loopback, "");
        assert!(local.contains(CaptureSource::Loopback.describe()), "{local}");
        assert_ne!(bot, local, "the two must not render identically");
    }

    #[test]
    fn sanitizes_and_names_file() {
        assert_eq!(
            summary_filename("2026-08-21", "Website standup: Q3/Q4?"),
            "2026-08-21 - Website standup Q3 Q4.md"
        );
        assert_eq!(sanitize_meeting_name("   "), "Meeting");
        assert_eq!(sanitize_meeting_name("a/b\\c:d"), "a b c d");
    }

    #[test]
    fn extracts_title_and_composes_header() {
        let md = compose_summary_markdown(
            "Website standup",
            "2026-08-21",
            "Teams",
            CaptureSource::MeetingBot,
            "# Website standup\n\n## Summary\n- Ship the nav\n",
        );
        assert!(md.starts_with("# Website standup\n"));
        assert!(md.contains("- Date: 2026-08-21"));
        assert!(md.contains("- Meeting: Website standup"));
        assert!(md.contains("## Summary"));
        assert_eq!(
            extract_title_from_markdown("# Weekly planning\n\nHello"),
            Some("Weekly planning".into())
        );
    }

    #[test]
    fn unique_path_increments() {
        let root = env::temp_dir().join("turbo-meeting-summary-unique");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let name = "2026-08-21 - Website.md";
        fs::write(root.join(name), "a").unwrap();
        let p = unique_summary_path(&root, name);
        assert_eq!(p.file_name().unwrap(), "2026-08-21 - Website-2.md");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn recap_dest_rejects_parent_dir() {
        let dir = PathBuf::from(r"C:\work\Meetings");
        assert!(recap_dest_is_safe(
            &dir,
            &dir.join("2026-08-21 - Website.md")
        ));
        assert!(!recap_dest_is_safe(
            &dir,
            &dir.join("..").join("outside.md")
        ));
        assert!(!recap_dest_is_safe(&dir, Path::new(r"C:\other\x.md")));
    }
}
