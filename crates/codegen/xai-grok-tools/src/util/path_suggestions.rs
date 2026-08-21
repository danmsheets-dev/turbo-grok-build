//! Path-not-found enrichment hints for tool error messages.
//!
//! Enriches "does not exist" errors from `list_dir`, `read_file`,
//! `search_replace`, and `grep` with actionable hints.
//!
//! When a workspace tree index is already available (process cache or durable
//! store), also attaches atlas `resolve_path` candidates as "Did you mean?".

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::util::workspace_tree_cache;

/// Ceiling for the single blocking-thread filesystem probe.
const HINT_TIMEOUT: Duration = Duration::from_millis(100);
/// Max similar-name suggestions
const MAX_SIMILAR: usize = 3;
/// Max atlas resolve hits for miss recovery
const MAX_ATLAS: usize = 8;
/// Minimum atlas score to surface a "Did you mean?" candidate.
/// Slightly lower than resolve_path's comfort threshold so miss recovery
/// surfaces near-matches when the atlas is warm.
const MIN_ATLAS_SCORE: f32 = 0.45;
/// Reduces noise from single-character names that would match on too many entries
const MIN_LEAF_LEN: usize = 2;
/// Minimum stem length for reverse substring matching (query contains entry).
/// Prevents short stems from over-matching.
const MIN_REVERSE_STEM_LEN: usize = 4;

/// One ranked path from the workspace tree atlas.
#[derive(Debug, Clone)]
pub struct AtlasPathHint {
    pub rel_path: String,
    pub score: f32,
}

/// Enrichment hints for a path that was not found.
#[derive(Debug, Clone)]
pub struct PathNotFoundHint {
    /// A corrected path from "dropped repo folder" detection.
    pub suggestion: Option<PathBuf>,
    /// Up to [`MAX_SIMILAR`] entries from the parent directory whose names
    /// are case-insensitive substring matches of the missing leaf.
    pub similar: Vec<PathBuf>,
    /// Workspace-tree atlas candidates (relative paths), if index is ready.
    pub atlas: Vec<AtlasPathHint>,
    /// Always-present CWD note for model re-orientation.
    pub cwd_note: String,
}

impl fmt::Display for PathNotFoundHint {
    /// Formats as a suffix to append after `"Error: {path} does not exist."`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref s) = self.suggestion {
            write!(f, " Did you mean {}?", s.display())?;
        } else if !self.atlas.is_empty() {
            write!(f, "\nDid you mean?")?;
            for (i, hit) in self.atlas.iter().enumerate() {
                write!(
                    f,
                    "\n  {}. {}  (score {:.2})",
                    i + 1,
                    hit.rel_path,
                    hit.score
                )?;
            }
        } else if !self.similar.is_empty() {
            let names: Vec<&str> = self
                .similar
                .iter()
                .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
                .collect();
            write!(
                f,
                "\nSimilar entries in parent directory: {}",
                names.join(", ")
            )?;
        }

        write!(f, "\n{}", self.cwd_note)
    }
}

/// Build hints for a path-not-found error.
///
/// Returns [`PathNotFoundHint`].
///
/// `path` is the resolved (real) filesystem path that failed.
/// `display_cwd` is the model-facing working directory (for the CWD note).
#[tracing::instrument(name = "fs.path_not_found_hint", skip_all)]
/// Join `rel` onto a model-facing display path, staying in that path's namespace.
///
/// `display_cwd` belongs to the model's world, which can be POSIX even when the
/// host is Windows (sandbox / worktree remap). `PathBuf::join` would splice in
/// the host separator and hand the model `/home/user/project\src`, which it
/// cannot use verbatim.
fn join_in_display_namespace(display_cwd: &Path, rel: &Path) -> std::path::PathBuf {
    let base = display_cwd.to_string_lossy();
    if base.contains('/') && !base.contains('\\') {
        let rel = rel.to_string_lossy().replace('\\', "/");
        return std::path::PathBuf::from(format!("{}/{}", base.trim_end_matches('/'), rel));
    }
    display_cwd.join(rel)
}

pub async fn path_not_found_hint(path: &Path, cwd: &Path, display_cwd: &Path) -> PathNotFoundHint {
    let cwd_note = format!(
        "Note: your current working directory is {}",
        display_cwd.display()
    );

    // All filesystem probing runs in a single spawn_blocking.
    let path_owned = path.to_path_buf();
    let cwd_owned = cwd.to_path_buf();

    let result = tokio::time::timeout(
        HINT_TIMEOUT,
        tokio::task::spawn_blocking(move || collect_hints(&path_owned, &cwd_owned)),
    )
    .await;

    let (suggestion, similar, atlas) = match result {
        Ok(Ok(val)) => val,
        _ => (None, Vec::new(), Vec::new()),
    };

    // Remap resolved worktree path to display space so the model never
    // sees internal paths (e.g. /worktree/abc-123/...).
    let suggestion = suggestion.map(|corrected| {
        corrected
            .strip_prefix(cwd)
            .map(|rel| join_in_display_namespace(display_cwd, rel))
            .unwrap_or_else(|_| {
                tracing::warn!(
                    corrected = %corrected.display(),
                    cwd = %cwd.display(),
                    "corrected path not under cwd; falling back to corrected path"
                );
                corrected
            })
    });

    PathNotFoundHint {
        suggestion,
        similar,
        atlas,
        cwd_note,
    }
}

/// Format a path-not-found error message.
///
/// When `hints_enabled` is `false`, returns a bare error string.
/// When `true`, appends CWD note, "did you mean?" correction, or similar-name
/// suggestions via [`path_not_found_hint`].
///
/// `display_path` is the model-facing path (for the error message).
/// `resolved_path` is the real filesystem path (for hint lookups).
pub async fn format_not_found_error(
    display_path: &Path,
    resolved_path: &Path,
    cwd: &Path,
    display_cwd: &Path,
    hints_enabled: bool,
) -> String {
    let base = format!("Error: {} does not exist.", display_path.display());
    if !hints_enabled {
        return base;
    }
    let hint = path_not_found_hint(resolved_path, cwd, display_cwd).await;
    format!("{base}{hint}")
}

/// Returns `(suggestion, similar, atlas)` where `suggestion` is a corrected path
/// from "dropped repo folder" detection (raw, not yet remapped to display space),
/// `similar` is substring-matched sibling entries, and `atlas` is workspace-tree
/// resolve hits when an index is already available (never builds).
fn collect_hints(path: &Path, cwd: &Path) -> (Option<PathBuf>, Vec<PathBuf>, Vec<AtlasPathHint>) {
    if let Some(corrected) = try_suggest_under_cwd(path, cwd) {
        return (Some(corrected), Vec::new(), Vec::new());
    }
    let similar = find_similar_entries(path);
    let atlas = atlas_resolve_hints(path, cwd);
    (None, similar, atlas)
}

/// Best-effort atlas suggestions. Never builds an index (miss recovery must stay fast).
fn atlas_resolve_hints(path: &Path, cwd: &Path) -> Vec<AtlasPathHint> {
    let config = xai_workspace_tree::WorkspaceTreeConfig::from_env();
    if !config.enabled {
        return Vec::new();
    }
    // Prefer process cache (warm kickoff), then durable store. Never build.
    let Some(index) = workspace_tree_cache::try_get(cwd)
        .or_else(|| workspace_tree_cache::try_load_cached(cwd, &config))
    else {
        return Vec::new();
    };

    // Prefer relative path under cwd as the resolve name / hint.
    let rel = path
        .strip_prefix(cwd)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
    if rel.is_empty() {
        return Vec::new();
    }

    let leaf = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| rel.clone());
    let stem = Path::new(&leaf)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&leaf)
        .to_string();

    let mut hits: Vec<AtlasPathHint> = Vec::new();
    let mut push_hits = |name: &str, hint: Option<&str>| {
        let result = xai_workspace_tree::resolve_path(&index, name, hint, MAX_ATLAS);
        for h in result.hits {
            if h.score < MIN_ATLAS_SCORE {
                continue;
            }
            if hits.iter().any(|x| x.rel_path == h.rel_path) {
                continue;
            }
            hits.push(AtlasPathHint {
                rel_path: h.rel_path,
                score: h.score,
            });
        }
    };

    // 1) Leaf basename with full relative path as hint (best for wrong-folder guesses).
    push_hits(&leaf, Some(rel.as_str()));
    // 2) Stem without extension (e.g. ship_roster.gd.uid → ship_roster).
    if stem.len() >= MIN_LEAF_LEN && !stem.eq_ignore_ascii_case(&leaf) {
        push_hits(&stem, Some(rel.as_str()));
    }
    // 3) Full relative path as name (exact-path / substring recovery).
    if rel.contains('/') {
        push_hits(&rel, None);
    }
    // 4) Fallback: search when resolve is sparse.
    if hits.len() < 2 && leaf.len() >= MIN_LEAF_LEN {
        let search = xai_workspace_tree::search(&index, &stem, MAX_ATLAS);
        for h in search.hits {
            if hits.iter().any(|x| x.rel_path == h.rel_path) {
                continue;
            }
            // search scores are 0.5–1.0; keep same floor.
            if h.score < MIN_ATLAS_SCORE {
                continue;
            }
            hits.push(AtlasPathHint {
                rel_path: h.rel_path,
                score: h.score,
            });
        }
    }

    hits.sort_by(|a, b| match b.score.partial_cmp(&a.score) {
        Some(ord) => ord.then_with(|| a.rel_path.cmp(&b.rel_path)),
        None => a.rel_path.cmp(&b.rel_path),
    });
    hits.truncate(MAX_ATLAS);
    hits
}

/// Detect the "dropped repo folder" pattern.
///
/// If the model asks for `/parent/foo` but cwd is `/parent/repo`, check
/// whether `/parent/repo/foo` exists. Only fires when the requested path
/// is under cwd's parent but not already under cwd.
fn try_suggest_under_cwd(path: &Path, cwd: &Path) -> Option<PathBuf> {
    if !path.is_absolute() || path.starts_with(cwd) {
        return None;
    }

    let cwd_parent = cwd.parent()?;
    let rel_from_parent = path.strip_prefix(cwd_parent).ok()?;

    // Guard against existing paths outside of repo.
    if let Some(std::path::Component::Normal(first)) = rel_from_parent.components().next() {
        let sibling = cwd_parent.join(first);
        if sibling != cwd && sibling.exists() {
            return None;
        }
    }

    let candidate = cwd.join(rel_from_parent);
    candidate.exists().then_some(candidate)
}

/// Scan the parent directory for entries whose names are case-insensitive
/// substring matches of the missing leaf name.
fn find_similar_entries(path: &Path) -> Vec<PathBuf> {
    let parent = match path.parent() {
        Some(p) => p,
        None => return Vec::new(),
    };
    let base = match path.file_name().and_then(|n| n.to_str()) {
        Some(b) if b.len() >= MIN_LEAF_LEN => b.to_lowercase(),
        _ => return Vec::new(),
    };

    // Strip extension from the query leaf for stem-level comparison.
    let base_stem = Path::new(&base)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&base)
        .to_lowercase();

    let read_dir = match std::fs::read_dir(parent) {
        Ok(rd) => rd,
        Err(_) => return Vec::new(),
    };

    let mut matches = Vec::new();
    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name == base {
            continue;
        }

        let name_stem = Path::new(&name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&name)
            .to_lowercase();

        // Find file matches that are substrings or reverse substrings up to MIN_REVERSE_STEM_LEN
        let forward = name_stem.contains(&base_stem);
        let reverse =
            !forward && name_stem.len() >= MIN_REVERSE_STEM_LEN && base_stem.contains(&name_stem);
        if forward || reverse {
            matches.push(entry.path());
            if matches.len() >= MAX_SIMILAR {
                break;
            }
        }
    }
    matches
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // Unit tests here cover internal invariants (guards, caps, priority,
    // Display formatting). Broader integration fixtures live in
    // tests/path_suggestions_production.rs.

    // â”€â”€ CWD note â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    #[tokio::test]
    async fn cwd_note_always_present() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path();
        let missing = cwd.join("nonexistent");

        let hint = path_not_found_hint(&missing, cwd, cwd).await;

        assert!(hint.cwd_note.contains(&cwd.display().to_string()));
        assert!(hint.suggestion.is_none());
        assert!(hint.similar.is_empty());
    }

    // â”€â”€ "dropped repo folder" detection â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    #[tokio::test]
    async fn dropped_repo_folder_detected() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        let target = repo.join("src");
        std::fs::create_dir_all(&target).unwrap();

        let bad_path = tmp.path().join("src");
        let hint = path_not_found_hint(&bad_path, &repo, &repo).await;

        assert_eq!(hint.suggestion.as_deref(), Some(target.as_path()));
    }

    #[tokio::test]
    async fn dropped_repo_folder_not_triggered_for_path_under_cwd() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().to_path_buf();
        let path = cwd.join("some_missing_file.rs");

        let hint = path_not_found_hint(&path, &cwd, &cwd).await;

        assert!(hint.suggestion.is_none());
    }

    #[tokio::test]
    async fn dropped_repo_folder_not_triggered_for_existing_sibling() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        let repo_backup = tmp.path().join("repo_backup");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&repo_backup).unwrap();
        std::fs::create_dir_all(repo.join("repo_backup")).unwrap();
        std::fs::write(repo.join("repo_backup/config"), b"").unwrap();

        let bad_path = repo_backup.join("config");
        let hint = path_not_found_hint(&bad_path, &repo, &repo).await;

        assert!(
            hint.suggestion.is_none(),
            "should not suggest path under cwd when model targets an existing sibling"
        );
    }

    #[tokio::test]
    async fn suggestion_takes_priority_over_similar_scan() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::create_dir(tmp.path().join("src_old")).unwrap();

        let bad_path = tmp.path().join("src");
        let hint = path_not_found_hint(&bad_path, &repo, &repo).await;

        assert!(hint.suggestion.is_some());
        assert!(hint.similar.is_empty());
    }

    // â”€â”€ similar-name scan (internal invariants) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    #[tokio::test]
    async fn similar_name_multi_match() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("helpers.rs"), b"").unwrap();
        std::fs::write(tmp.path().join("helper_test.rs"), b"").unwrap();

        let missing = tmp.path().join("helper");
        let hint = path_not_found_hint(&missing, tmp.path(), tmp.path()).await;

        let names: Vec<String> = hint
            .similar
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        assert!(names.contains(&"helpers.rs".to_string()), "got: {names:?}");
        assert!(
            names.contains(&"helper_test.rs".to_string()),
            "got: {names:?}"
        );
    }

    #[tokio::test]
    async fn similar_name_cap_at_max() {
        let tmp = TempDir::new().unwrap();
        for i in 0..10 {
            std::fs::write(tmp.path().join(format!("test_{i}.rs")), b"").unwrap();
        }

        let missing = tmp.path().join("test");
        let hint = path_not_found_hint(&missing, tmp.path(), tmp.path()).await;

        assert_eq!(hint.similar.len(), MAX_SIMILAR);
    }

    #[tokio::test]
    async fn similar_name_short_entry_not_matched() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("he"), b"").unwrap();
        std::fs::write(tmp.path().join("rs"), b"").unwrap();

        let missing = tmp.path().join("helpers_test");
        let hint = path_not_found_hint(&missing, tmp.path(), tmp.path()).await;

        assert!(
            hint.similar.is_empty(),
            "short entries should not match: got {:?}",
            hint.similar
        );
    }

    // â”€â”€ Display formatting â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    #[test]
    fn display_with_suggestion() {
        let hint = PathNotFoundHint {
            suggestion: Some(PathBuf::from("/project/repo/src")),
            similar: Vec::new(),
            atlas: Vec::new(),
            cwd_note: "Note: your current working directory is /project/repo".into(),
        };
        let output = hint.to_string();
        assert!(output.contains("Did you mean /project/repo/src?"));
        assert!(output.contains("Note: your current working directory is"));
    }

    #[test]
    fn display_with_similar() {
        let hint = PathNotFoundHint {
            suggestion: None,
            similar: vec![
                PathBuf::from("/project/helpers.rs"),
                PathBuf::from("/project/helper_test.rs"),
            ],
            atlas: Vec::new(),
            cwd_note: "Note: your current working directory is /project".into(),
        };
        let output = hint.to_string();
        assert!(output.contains("Similar entries in parent directory:"));
        assert!(output.contains("helpers.rs"));
        assert!(output.contains("helper_test.rs"));
    }

    #[test]
    fn display_with_atlas() {
        let hint = PathNotFoundHint {
            suggestion: None,
            similar: Vec::new(),
            atlas: vec![AtlasPathHint {
                rel_path: "scripts/core/ship_roster.gd".into(),
                score: 0.96,
            }],
            cwd_note: "Note: your current working directory is /project".into(),
        };
        let output = hint.to_string();
        assert!(output.contains("Did you mean?"));
        assert!(output.contains("scripts/core/ship_roster.gd"));
        assert!(output.contains("0.96"));
    }

    #[test]
    fn display_empty() {
        let hint = PathNotFoundHint {
            suggestion: None,
            similar: Vec::new(),
            atlas: Vec::new(),
            cwd_note: "Note: your current working directory is /project".into(),
        };
        let output = hint.to_string();
        assert!(!output.contains("Did you mean"));
        assert!(!output.contains("Similar entries"));
        assert!(output.contains("Note: your current working directory is /project"));
    }
}
