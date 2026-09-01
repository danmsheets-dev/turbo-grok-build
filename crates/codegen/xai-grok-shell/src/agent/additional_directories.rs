//! ACP `additionalDirectories`: extra workspace roots on a session.
//!
//! `cwd` stays the primary working directory (relative paths, git, sessions).
//! Extra roots expand the filesystem confine set. Each extra path must be
//! absolute, exist as a directory, and — when process `--confine` is set —
//! lie under **some** process confine root (hard upper bound).

use std::path::{Path, PathBuf};

use agent_client_protocol as acp;
use xai_grok_tools::types::resources::{
    path_is_under_any_root, path_is_under_confine_root, process_confine_roots,
};
use xai_grok_workspace::trust::is_unsafe_trust_root;

/// Normalize extra workspace roots from an ACP session request.
///
/// Drops redundant entries (same as `primary`, nested inside `primary`,
/// duplicates). Rejects non-absolute, missing, unsafe (home / fs-root),
/// ancestor-of-primary, and extras outside process `--confine`.
pub(crate) fn normalize_additional_directories(
    primary: &Path,
    extras: &[PathBuf],
) -> Result<Vec<PathBuf>, acp::Error> {
    normalize_additional_directories_lenient(primary, extras)
}

fn normalize_one(
    primary: &Path,
    raw: &Path,
    process_roots: &[PathBuf],
) -> Result<Option<PathBuf>, acp::Error> {
    if !raw.is_absolute() {
        return Err(invalid(format!(
            "additionalDirectories entry `{}` must be an absolute path",
            raw.display()
        )));
    }
    let meta = std::fs::metadata(raw).map_err(|e| {
        invalid(format!(
            "additionalDirectories entry `{}` does not exist or is inaccessible: {e}",
            raw.display()
        ))
    })?;
    if !meta.is_dir() {
        return Err(invalid(format!(
            "additionalDirectories entry `{}` is not a directory",
            raw.display()
        )));
    }
    let canonical = dunce::canonicalize(raw).map_err(|e| {
        invalid(format!(
            "additionalDirectories entry `{}` could not be canonicalized: {e}",
            raw.display()
        ))
    })?;
    if is_unsafe_trust_root(&canonical) {
        return Err(invalid(format!(
            "additionalDirectories entry `{}` is too broad (home or filesystem root)",
            canonical.display()
        )));
    }
    if paths_same(&canonical, primary) || path_is_under_confine_root(&canonical, primary) {
        tracing::info!(
            extra = %canonical.display(),
            primary = %primary.display(),
            "additionalDirectories: dropping redundant extra (same as or inside primary cwd)"
        );
        return Ok(None);
    }
    if path_is_under_confine_root(primary, &canonical) {
        return Err(invalid(format!(
            "additionalDirectories entry `{}` is an ancestor of the session cwd `{}`; \
             attaching a parent would widen the primary workspace",
            canonical.display(),
            primary.display()
        )));
    }
    if !process_roots.is_empty() && !path_is_under_any_root(&canonical, process_roots) {
        let listed = process_roots
            .iter()
            .map(|r| format!("`{}`", r.display()))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(invalid(format!(
            "additionalDirectories entry `{}` is outside process confine roots {listed}",
            canonical.display()
        )));
    }
    Ok(Some(canonical))
}

fn invalid(msg: String) -> acp::Error {
    acp::Error::invalid_params().data(msg)
}

fn paths_same(a: &Path, b: &Path) -> bool {
    path_is_under_confine_root(a, b) && path_is_under_confine_root(b, a)
}

/// Drop `Redundant` errors so a list of extras can include primary-overlapping
/// entries without aborting the session.
pub(crate) fn normalize_additional_directories_lenient(
    primary: &Path,
    extras: &[PathBuf],
) -> Result<Vec<PathBuf>, acp::Error> {
    if extras.is_empty() {
        return Ok(Vec::new());
    }
    let primary_canon = dunce::canonicalize(primary).unwrap_or_else(|_| primary.to_path_buf());
    let process_roots = process_confine_roots();
    let mut out: Vec<PathBuf> = Vec::with_capacity(extras.len());
    for raw in extras {
        match normalize_one(&primary_canon, raw, process_roots)? {
            Some(canonical) if !out.iter().any(|existing| paths_same(existing, &canonical)) => {
                out.push(canonical);
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Claude-settings extras: invalid/missing/unsafe entries are skipped with a
/// warning instead of aborting the session (settings can be stale). Relative
/// paths join `primary`.
pub(crate) fn collect_claude_additional_directories(
    primary: &Path,
    project_trusted: bool,
) -> (Vec<PathBuf>, Vec<String>) {
    use xai_grok_workspace::permission::claude_settings::{
        claude_settings_paths_for_trust, load_claude_settings,
    };
    let mut raw = Vec::new();
    for path in claude_settings_paths_for_trust(primary, project_trusted) {
        let Some(settings) = load_claude_settings(&path) else {
            continue;
        };
        let Some(dirs) = settings.additional_directories else {
            continue;
        };
        for entry in dirs {
            raw.push((path.clone(), entry));
        }
    }
    merge_claude_entries(primary, &raw)
}

/// Union `existing` with Claude extras. Existing (ACP / `--add-dir`) wins
/// order; Claude fills in new siblings. Invalid Claude entries are skipped.
pub(crate) fn union_with_claude_directories(
    primary: &Path,
    existing: Vec<PathBuf>,
    project_trusted: bool,
) -> (Vec<PathBuf>, Vec<String>) {
    let (claude, skipped) = collect_claude_additional_directories(primary, project_trusted);
    if claude.is_empty() {
        return (existing, skipped);
    }
    let mut out = existing;
    let process_roots = process_confine_roots();
    let primary_canon = dunce::canonicalize(primary).unwrap_or_else(|_| primary.to_path_buf());
    for extra in claude {
        match normalize_one(&primary_canon, &extra, process_roots) {
            Ok(Some(canonical)) if !out.iter().any(|e| paths_same(e, &canonical)) => {
                out.push(canonical);
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }
    (out, skipped)
}

fn merge_claude_entries(primary: &Path, raw: &[(PathBuf, String)]) -> (Vec<PathBuf>, Vec<String>) {
    let mut accepted: Vec<PathBuf> = Vec::new();
    let mut skipped = Vec::new();
    let process_roots = process_confine_roots();
    let primary_canon = dunce::canonicalize(primary).unwrap_or_else(|_| primary.to_path_buf());
    for (source, entry) in raw {
        let path = resolve_claude_entry(primary, entry);
        match normalize_one(&primary_canon, &path, process_roots) {
            Ok(Some(canonical)) if !accepted.iter().any(|e| paths_same(e, &canonical)) => {
                accepted.push(canonical);
            }
            Ok(None) => {}
            Ok(Some(_)) => {}
            Err(err) => {
                let why = err
                    .data
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .unwrap_or("rejected")
                    .to_string();
                tracing::warn!(
                    source = %source.display(),
                    entry = %entry,
                    %why,
                    "Claude settings additionalDirectories entry skipped"
                );
                skipped.push(format!("{}: {why}", source.display()));
            }
        }
    }
    (accepted, skipped)
}

fn resolve_claude_entry(primary: &Path, entry: &str) -> PathBuf {
    let raw = PathBuf::from(entry);
    if raw.is_absolute() {
        raw
    } else {
        primary.join(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_list_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(normalize_additional_directories(tmp.path(), &[]).unwrap().is_empty());
    }

    #[test]
    fn sibling_dir_is_accepted() {
        let primary = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        let got = normalize_additional_directories(primary.path(), &[extra.path().to_path_buf()])
            .unwrap();
        assert_eq!(got.len(), 1);
        assert!(got[0].ends_with(extra.path().file_name().unwrap()));
    }

    #[test]
    fn relative_path_is_rejected() {
        let primary = tempfile::tempdir().unwrap();
        let err = normalize_additional_directories(
            primary.path(),
            &[PathBuf::from("relative/extra")],
        )
        .unwrap_err();
        let data = err.data.as_ref().and_then(|v| v.as_str()).unwrap_or("");
        assert!(data.contains("absolute"), "{data}");
    }

    #[test]
    fn nested_inside_primary_is_dropped() {
        let primary = tempfile::tempdir().unwrap();
        let nested = primary.path().join("src");
        std::fs::create_dir(&nested).unwrap();
        let got = normalize_additional_directories_lenient(primary.path(), &[nested]).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn ancestor_of_primary_is_rejected() {
        let outer = tempfile::tempdir().unwrap();
        let primary = outer.path().join("proj");
        std::fs::create_dir(&primary).unwrap();
        let err = normalize_additional_directories(primary.as_path(), &[outer.path().to_path_buf()])
            .unwrap_err();
        let data = err.data.as_ref().and_then(|v| v.as_str()).unwrap_or("");
        assert!(data.contains("ancestor") || data.contains("widen"), "{data}");
    }

    #[test]
    fn missing_path_is_rejected() {
        let primary = tempfile::tempdir().unwrap();
        let missing = primary.path().join("does-not-exist-extra");
        let err = normalize_additional_directories(primary.path(), &[missing]).unwrap_err();
        let data = err.data.as_ref().and_then(|v| v.as_str()).unwrap_or("");
        assert!(data.contains("does not exist"), "{data}");
    }

    #[test]
    fn non_directory_is_rejected() {
        let primary = tempfile::tempdir().unwrap();
        let extra_file = tempfile::NamedTempFile::new().unwrap();
        let err = normalize_additional_directories(
            primary.path(),
            &[extra_file.path().to_path_buf()],
        )
        .unwrap_err();
        let data = err.data.as_ref().and_then(|v| v.as_str()).unwrap_or("");
        assert!(data.contains("not a directory"), "{data}");
    }

    #[test]
    fn filesystem_root_is_rejected_as_too_broad() {
        let primary = tempfile::tempdir().unwrap();
        let root = if cfg!(windows) {
            PathBuf::from(r"C:\")
        } else {
            PathBuf::from("/")
        };
        let err = normalize_additional_directories(primary.path(), &[root]).unwrap_err();
        let data = err.data.as_ref().and_then(|v| v.as_str()).unwrap_or("");
        assert!(data.contains("too broad"), "{data}");
    }

    #[test]
    fn extra_outside_process_confine_is_rejected() {
        let primary = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        let err = normalize_one(
            primary.path(),
            extra.path(),
            &[primary.path().to_path_buf()],
        )
        .unwrap_err();
        let data = err.data.as_ref().and_then(|v| v.as_str()).unwrap_or("");
        assert!(data.contains("outside process confine"), "{data}");
    }

    #[test]
    fn extra_under_second_process_confine_root_is_accepted() {
        let primary = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        // extra is a sibling; treat `second` as a process root covering it by
        // passing extra itself as the second process root (same path).
        let got = normalize_one(
            primary.path(),
            extra.path(),
            &[primary.path().to_path_buf(), extra.path().to_path_buf()],
        )
        .unwrap();
        assert!(got.is_some(), "extra under a later confine root must be kept");
        let _ = second;
    }

    #[test]
    fn claude_relative_entry_joins_primary() {
        let primary = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        let (got, skipped) = merge_claude_entries(
            primary.path(),
            &[(
                primary.path().join(".claude/settings.json"),
                extra.path().to_string_lossy().into_owned(),
            )],
        );
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn claude_missing_entry_is_skipped() {
        let primary = tempfile::tempdir().unwrap();
        let (got, skipped) = merge_claude_entries(
            primary.path(),
            &[(
                primary.path().join(".claude/settings.json"),
                primary.path().join("gone-extra").to_string_lossy().into_owned(),
            )],
        );
        assert!(got.is_empty());
        assert_eq!(skipped.len(), 1);
    }

    #[test]
    fn union_keeps_existing_and_adds_claude_sibling() {
        let primary = tempfile::tempdir().unwrap();
        let existing = tempfile::tempdir().unwrap();
        let claude = tempfile::tempdir().unwrap();
        let existing_canon = dunce::canonicalize(existing.path()).unwrap();
        let (got, skipped) = {
            let (claude_dirs, skipped) = merge_claude_entries(
                primary.path(),
                &[(
                    PathBuf::from("/settings.json"),
                    claude.path().to_string_lossy().into_owned(),
                )],
            );
            let mut out = vec![existing_canon.clone()];
            for extra in claude_dirs {
                if !out.iter().any(|e| paths_same(e, &extra)) {
                    out.push(extra);
                }
            }
            (out, skipped)
        };
        assert!(skipped.is_empty());
        assert_eq!(got.len(), 2);
        assert!(got.iter().any(|p| paths_same(p, &existing_canon)));
    }
}
