//! `{workspace}/Schedules/YYYY-MM-DD - <title>.md` jail (same rules as Meetings/).

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, Utc};

/// Subfolder of the launch workspace where schedule briefings are written.
pub const WORKSPACE_SCHEDULES_DIR: &str = "Schedules";

pub fn workspace_schedules_dir(workspace: &Path) -> PathBuf {
    workspace.join(WORKSPACE_SCHEDULES_DIR)
}

pub fn local_date_stamp(utc: DateTime<Utc>) -> String {
    utc.with_timezone(&Local).format("%Y-%m-%d").to_string()
}

/// Strip characters that cannot appear in a Windows filename.
pub fn sanitize_schedule_title(name: &str) -> String {
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
        "schedule".into()
    } else {
        trimmed.chars().take(80).collect()
    }
}

pub fn schedule_filename(date: &str, title: &str) -> String {
    format!("{} - {}.md", date, sanitize_schedule_title(title))
}

/// First unused path in `dir` for `filename` (`stem-2.md`, `stem-3.md`, …).
pub fn unique_schedule_path(dir: &Path, filename: &str) -> PathBuf {
    let primary = dir.join(filename);
    if !primary.exists() {
        return primary;
    }
    let stem = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("schedule");
    for i in 2..1000 {
        let candidate = dir.join(format!("{stem}-{i}.md"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem}-overflow.md"))
}

/// True when `dest` is a single file directly under `schedules_dir` (no `..`).
pub fn schedule_dest_is_safe(schedules_dir: &Path, dest: &Path) -> bool {
    if dest
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
        || schedules_dir
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
    dest.parent() == Some(schedules_dir)
}

pub fn write_workspace_schedule(path: &Path, markdown: &str) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "schedule path has no parent",
        ));
    };
    if parent
        .symlink_metadata()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing to write through a symlinked Schedules folder",
        ));
    }
    fs::create_dir_all(parent)?;
    fs::write(path, markdown)
}

/// Jail + write a briefing under `{workspace}/Schedules/YYYY-MM-DD - <title>.md`.
pub fn write_schedule_briefing(
    workspace: &Path,
    title: &str,
    markdown: &str,
    now: DateTime<Utc>,
) -> std::io::Result<PathBuf> {
    let dir = workspace_schedules_dir(workspace);
    let dest = unique_schedule_path(&dir, &schedule_filename(&local_date_stamp(now), title));
    if !schedule_dest_is_safe(&dir, &dest) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing to write outside the workspace Schedules folder",
        ));
    }
    write_workspace_schedule(&dest, markdown)?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn sanitizes_and_names_file() {
        assert_eq!(
            schedule_filename("2026-08-23", "CI status: Q3/Q4?"),
            "2026-08-23 - CI status Q3 Q4.md"
        );
        assert_eq!(sanitize_schedule_title("   "), "schedule");
        assert_eq!(sanitize_schedule_title("a/b\\c:d"), "a b c d");
    }

    #[test]
    fn schedule_dest_rejects_parent_dir() {
        let dir = PathBuf::from(r"C:\work\Schedules");
        assert!(schedule_dest_is_safe(
            &dir,
            &dir.join("2026-08-23 - briefing.md")
        ));
        assert!(!schedule_dest_is_safe(
            &dir,
            &dir.join("..").join("outside.md")
        ));
        assert!(!schedule_dest_is_safe(&dir, Path::new(r"C:\other\x.md")));
        assert!(!schedule_dest_is_safe(
            &dir,
            &dir.join("nested").join("x.md")
        ));
    }

    #[test]
    fn write_briefing_rejects_dotdot_title() {
        let root = env::temp_dir().join("turbo-schedule-dotdot");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let dir = workspace_schedules_dir(&root);
        let sneaky = dir.join("..").join("escaped.md");
        assert!(!schedule_dest_is_safe(&dir, &sneaky));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn write_briefing_round_trip() {
        let root = env::temp_dir().join("turbo-schedule-write");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let now = DateTime::parse_from_rfc3339("2026-08-23T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let path = write_schedule_briefing(&root, "daily search", "# hi\n", now).unwrap();
        assert!(schedule_dest_is_safe(
            &workspace_schedules_dir(&root),
            &path
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "# hi\n");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn write_rejects_symlinked_schedules_folder() {
        let root = env::temp_dir().join("turbo-schedule-symlink");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let real = root.join("real");
        fs::create_dir_all(&real).unwrap();
        let link = root.join(WORKSPACE_SCHEDULES_DIR);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real, &link).unwrap();
        }
        #[cfg(windows)]
        {
            if std::os::windows::fs::symlink_dir(&real, &link).is_err() {
                let _ = fs::remove_dir_all(&root);
                return;
            }
        }
        let dest = link.join("2026-08-23 - x.md");
        let err = write_workspace_schedule(&dest, "nope").unwrap_err();
        assert!(
            err.to_string().contains("symlink"),
            "expected symlink refusal, got {err}"
        );
        let _ = fs::remove_dir_all(&root);
    }
}
