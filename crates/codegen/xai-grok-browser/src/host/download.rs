//! Session-scoped download broker (jail + listing).
//!
//! Page-initiated attachments never choose an arbitrary destination. The host
//! writes into `<session>/downloads` with sanitized names; listing refuses
//! symlinks and non-files. Kept outside `webview.rs` so the jail is testable
//! without WebView2.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::protocol::{DownloadInfo, DownloadsResult};

/// Partial + final paths for a page-initiated attachment under the session jail.
pub(crate) fn broker_attachment(
    session_folder: &Path,
    suggested_filename: Option<&str>,
    source_uri: Option<&str>,
) -> Result<(PathBuf, PathBuf), String> {
    let folder = prepare_download_folder(session_folder)?;
    let filename = suggested_filename
        .and_then(safe_download_filename)
        .or_else(|| {
            source_uri.and_then(|uri| {
                uri.split('?')
                    .next()
                    .and_then(|path| path.rsplit(['/', '\\']).next())
                    .and_then(safe_download_filename)
            })
        })
        .unwrap_or_else(|| "download.bin".to_owned());
    let final_destination = unique_download_path(&folder, &filename);
    let partial_name = format!(
        ".{}.part",
        final_destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("download.bin")
    );
    let partial_destination = unique_download_path(&folder, &partial_name);
    Ok((partial_destination, final_destination))
}

pub(crate) fn prepare_download_folder(session_folder: &Path) -> Result<PathBuf, String> {
    reject_symlink_components(session_folder)?;
    if !session_folder.is_dir() {
        return Err(format!(
            "session folder is not an existing directory: {}",
            session_folder.display()
        ));
    }
    let session_root = session_folder
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize session folder: {error}"))?;
    let folder = session_folder.join("downloads");
    if folder.exists() {
        let metadata = std::fs::symlink_metadata(&folder)
            .map_err(|error| format!("cannot inspect broker folder: {error}"))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("broker folder is not a real directory".to_owned());
        }
    } else {
        std::fs::create_dir(&folder)
            .map_err(|error| format!("cannot create broker folder: {error}"))?;
    }
    reject_symlink_components(&folder)?;
    let canonical_folder = folder
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize broker folder: {error}"))?;
    if !canonical_folder.starts_with(&session_root) {
        return Err("broker folder escapes the session folder".to_owned());
    }
    Ok(canonical_folder)
}

pub(crate) fn reject_symlink_components(path: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "path contains symlink component: {}",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot inspect path component: {error}")),
        }
    }
    Ok(())
}

pub(crate) fn recent_completed_download(
    session_folder: Option<&Path>,
    active_downloads: &HashSet<PathBuf>,
) -> Option<DownloadInfo> {
    let folder = session_folder?;
    let list = list_brokered_downloads(folder, active_downloads).ok()?;
    let now = std::time::SystemTime::now();
    list.downloads.into_iter().rev().find(|download| {
        if !download.completed {
            return false;
        }
        let Ok(meta) = std::fs::metadata(&download.path) else {
            return false;
        };
        let Ok(modified) = meta.modified() else {
            return false;
        };
        now.duration_since(modified)
            .map(|age| age.as_secs() < 15)
            .unwrap_or(false)
    })
}

pub(crate) fn list_brokered_downloads(
    session_folder: &Path,
    active_downloads: &HashSet<PathBuf>,
) -> Result<DownloadsResult, String> {
    reject_symlink_components(session_folder)?;
    let session_root = session_folder
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize session folder: {error}"))?;
    let folder = session_folder.join("downloads");
    let folder_metadata = match std::fs::symlink_metadata(&folder) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DownloadsResult::default());
        }
        Err(error) => {
            return Err(format!("cannot inspect broker folder: {error}"));
        }
    };
    if folder_metadata.file_type().is_symlink() || !folder_metadata.is_dir() {
        return Err("broker folder is not a real directory".to_owned());
    }
    let folder = folder
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize broker folder: {error}"))?;
    if !folder.starts_with(&session_root) {
        return Err("broker folder escapes the session folder".to_owned());
    }
    let entries = match std::fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(error) => {
            return Err(format!("cannot list brokered downloads: {error}"));
        }
    };
    let mut downloads = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot read brokered download: {error}"))?;
        // Do not follow links while exposing brokered files. A symlink in the
        // broker directory must not turn this read-only listing into a path
        // disclosure outside the session folder.
        let metadata = entry
            .path()
            .symlink_metadata()
            .map_err(|error| format!("cannot inspect brokered download: {error}"))?;
        if !metadata.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.ends_with(".part") {
            continue;
        }
        downloads.push(DownloadInfo {
            name: name.to_owned(),
            path: path.to_string_lossy().into_owned(),
            bytes: metadata.len(),
            completed: !active_downloads.contains(&path),
        });
    }
    downloads.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(DownloadsResult { downloads })
}

pub(crate) fn safe_download_filename(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty()
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '/' | '\\'))
    {
        return None;
    }
    let trimmed = name.trim_end_matches([' ', '.']);
    if trimmed.is_empty() {
        return None;
    }
    let stem = trimmed.split('.').next().unwrap_or_default();
    let upper_stem = stem.to_ascii_uppercase();
    let reserved_device = matches!(upper_stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper_stem.len() == 4
            && (upper_stem.starts_with("COM") || upper_stem.starts_with("LPT"))
            && upper_stem.as_bytes()[3].is_ascii_digit());
    if reserved_device {
        return None;
    }
    Some(trimmed.chars().take(180).collect())
}

pub(crate) fn unique_download_path(folder: &Path, filename: &str) -> PathBuf {
    let candidate = folder.join(filename);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("download");
    let extension = path.extension().and_then(|s| s.to_str());
    for index in 1..=10_000u32 {
        let name = match extension {
            Some(extension) => format!("{stem} ({index}).{extension}"),
            None => format!("{stem} ({index})"),
        };
        let candidate = folder.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    folder.join(format!(
        "download-{}-{}.bin",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_session(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "turbo-browser-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    #[test]
    fn attachment_is_brokered_and_listed() {
        let session = temp_session("attachment-e2e");
        let _ = std::fs::remove_dir_all(&session);
        std::fs::create_dir_all(&session).unwrap();

        let (partial, final_path) = broker_attachment(
            &session,
            Some("report.pdf"),
            Some("https://example.com/files/report.pdf"),
        )
        .unwrap();
        assert_eq!(
            final_path.file_name().and_then(|n| n.to_str()),
            Some("report.pdf")
        );
        assert!(
            partial
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".part")),
            "{partial:?}"
        );
        assert!(
            final_path.starts_with(session.join("downloads")) || {
                let canon = session.canonicalize().unwrap().join("downloads");
                final_path.starts_with(&canon)
            }
        );

        std::fs::write(&partial, b"%PDF-bytes").unwrap();
        std::fs::rename(&partial, &final_path).unwrap();

        let listed = list_brokered_downloads(&session, &HashSet::new()).unwrap();
        assert_eq!(listed.downloads.len(), 1);
        assert_eq!(listed.downloads[0].name, "report.pdf");
        assert_eq!(listed.downloads[0].bytes, 10);
        assert!(listed.downloads[0].completed);
        let _ = std::fs::remove_dir_all(session);
    }

    #[test]
    fn reserved_attachment_name_falls_back_to_download_bin() {
        let session = temp_session("reserved-name");
        let _ = std::fs::remove_dir_all(&session);
        std::fs::create_dir_all(&session).unwrap();
        let (_, final_path) = broker_attachment(&session, Some("CON"), None).unwrap();
        assert_eq!(
            final_path.file_name().and_then(|n| n.to_str()),
            Some("download.bin")
        );
        let (_, from_uri) =
            broker_attachment(&session, None, Some("https://example.com/LPT1.txt")).unwrap();
        assert_eq!(
            from_uri.file_name().and_then(|n| n.to_str()),
            Some("download.bin")
        );
        let _ = std::fs::remove_dir_all(session);
    }

    #[test]
    fn broker_folder_that_is_a_file_is_refused() {
        let session = temp_session("broker-file");
        let _ = std::fs::remove_dir_all(&session);
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(session.join("downloads"), b"not a dir").unwrap();
        let err = broker_attachment(&session, Some("a.pdf"), None).unwrap_err();
        assert!(err.contains("not a real directory"), "{err}");
        let err = list_brokered_downloads(&session, &HashSet::new()).unwrap_err();
        assert!(err.contains("not a real directory"), "{err}");
        let _ = std::fs::remove_dir_all(session);
    }

    #[test]
    fn download_listing_returns_sorted_regular_files_only() {
        let session = temp_session("download-list");
        let downloads = session.join("downloads");
        let _ = std::fs::remove_dir_all(&session);
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::write(downloads.join("zeta.txt"), b"123").unwrap();
        std::fs::write(downloads.join("alpha.pdf"), b"12").unwrap();
        std::fs::create_dir(downloads.join("nested")).unwrap();

        let canonical_downloads = downloads.canonicalize().unwrap();
        let result = list_brokered_downloads(&session, &HashSet::new()).unwrap();
        assert_eq!(
            result.downloads,
            vec![
                DownloadInfo {
                    name: "alpha.pdf".into(),
                    path: canonical_downloads
                        .join("alpha.pdf")
                        .to_string_lossy()
                        .into_owned(),
                    bytes: 2,
                    completed: true,
                },
                DownloadInfo {
                    name: "zeta.txt".into(),
                    path: canonical_downloads
                        .join("zeta.txt")
                        .to_string_lossy()
                        .into_owned(),
                    bytes: 3,
                    completed: true,
                },
            ]
        );
        let _ = std::fs::remove_dir_all(session);
    }

    #[test]
    fn active_downloads_are_not_reported_complete() {
        let session = temp_session("active-download");
        let downloads = session.join("downloads");
        let path = downloads.join("partial.bin.part");
        let _ = std::fs::remove_dir_all(&session);
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::write(&path, b"partial").unwrap();

        let active = HashSet::from([path.canonicalize().unwrap()]);
        let result = list_brokered_downloads(&session, &active).unwrap();
        assert!(result.downloads.is_empty());
        let _ = std::fs::remove_dir_all(session);
    }

    #[test]
    fn download_filename_rejects_traversal_and_devices() {
        for name in ["", ".", "..", "..\\secret.txt", "CON", "LPT1.txt"] {
            assert!(safe_download_filename(name).is_none(), "accepted {name:?}");
        }
        assert_eq!(
            safe_download_filename(" report.pdf. "),
            Some("report.pdf".to_owned())
        );
    }

    #[test]
    fn download_path_adds_collision_suffix() {
        let dir = temp_session("download-collision");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("report.pdf"), b"existing").unwrap();
        assert_eq!(
            unique_download_path(&dir, "report.pdf"),
            dir.join("report (1).pdf")
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
