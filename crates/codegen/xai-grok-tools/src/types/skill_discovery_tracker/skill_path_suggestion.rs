//! Registered skill-path lookup for failed `SKILL.md` reads.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::implementations::skills::types::skill_name_from_path;

use super::{SkillManager, canonical_path};

/// A unique registered skill path matching a failed `SKILL.md` read.
#[derive(Debug, Clone)]
pub(crate) struct SkillPathSuggestion {
    /// Real path used by the filesystem backend.
    pub(crate) path: PathBuf,
    /// Model-facing path with a forked worktree prefix rewritten when needed.
    pub(crate) display_path: PathBuf,
}

impl SkillManager {
    /// Find one registered skill whose command or directory identity matches the
    /// parent directory of `requested_path`. Ambiguous matches return `None`.
    ///
    /// Candidates come from the current collections in precedence order — the
    /// listing baseline, held conditional skills, then dynamic discoveries — so
    /// a baseline reload that removes, moves, or disables a skill immediately
    /// stops suggesting it.
    pub(crate) fn suggest_skill_path(&self, requested_path: &Path) -> Option<SkillPathSuggestion> {
        let requested_name = skill_name_from_path(requested_path.to_str()?)?;
        // Fork/display state must be coherent before any path is surfaced:
        // half-seeded state could leak a real worktree path to the model.
        let display_mapping = match (&self.real_cwd_prefix, &self.display_cwd) {
            (Some(real), Some(display)) => Some((real.as_str(), display.as_str())),
            (None, None) => None,
            _ => return None,
        };

        let mut owned_paths = HashSet::new();
        let mut suggestion: Option<SkillPathSuggestion> = None;
        // Count every eligible same-name registration, including the failed
        // path itself: skipping that path without counting it would let a
        // second same-named skill look unique and get suggested.
        let mut match_count = 0usize;
        for skill in self
            .startup_skills
            .iter()
            .chain(self.conditional.held())
            .chain(&self.discovered_skills)
        {
            let skill_path = Path::new(&skill.path);
            // Accept both OS-native absolute paths and POSIX absolute forms
            // (`/home/...`). On Windows, `Path::is_absolute` requires a drive
            // prefix, so a leading-`/` skill registry path (tests, Git Bash
            // homes, cross-host session dumps) was silently skipped and
            // suggestions always returned `None`.
            if !is_absolute_skill_path(skill_path) {
                continue;
            }
            let canonical = canonical_path(&skill.path);
            // The highest-precedence record owns its canonical path outright:
            // a shadowed record must not be suggested (nor count as ambiguity)
            // even when the owner is disabled or otherwise ineligible.
            if !owned_paths.insert(canonical.clone()) {
                continue;
            }
            if !skill.enabled {
                continue;
            }
            let Some(directory_name) = skill_name_from_path(&skill.path) else {
                continue;
            };
            if skill.name != requested_name && directory_name != requested_name {
                continue;
            }
            match_count += 1;
            if match_count > 1 {
                return None;
            }
            if canonical == requested_path {
                // Already the failed read target — not a suggestion, but it
                // still occupied the single unique-match slot above.
                continue;
            }

            let display_path = match display_mapping {
                Some((real, display)) => skill_path
                    .strip_prefix(real)
                    .map(|relative| join_model_display_path(display, relative))
                    .unwrap_or_else(|_| skill_path.to_path_buf()),
                None => skill_path.to_path_buf(),
            };
            suggestion = Some(SkillPathSuggestion {
                path: skill_path.to_path_buf(),
                display_path,
            });
        }
        suggestion
    }
}

/// Whether `path` is absolute for skill-registry purposes.
///
/// [`Path::is_absolute`] on Windows requires a drive/UNC prefix, so a path
/// string starting with `/` (POSIX absolute, common in skill paths and tests)
/// is rejected. Treat a leading `/` as absolute on every platform so lookup
/// stays host-agnostic for string-stored skill paths.
fn is_absolute_skill_path(path: &Path) -> bool {
    if path.is_absolute() {
        return true;
    }
    path.to_str().is_some_and(|s| s.starts_with('/'))
}

/// Join a display-cwd prefix with a relative skill path for model-facing text.
///
/// Uses the separator style of `display` (forward slashes for POSIX display
/// roots) so Windows `Path::join` does not inject `\` into
/// `/display/project/.grok/skills/...` spellings.
fn join_model_display_path(display: &str, relative: &Path) -> PathBuf {
    let use_forward = display.contains('/') || !display.contains('\\');
    let sep = if use_forward { "/" } else { "\\" };
    let rel: String = relative
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str().map(str::to_owned),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(sep);
    let base = display.trim_end_matches(['/', '\\']);
    if rel.is_empty() {
        PathBuf::from(base)
    } else {
        PathBuf::from(format!("{base}{sep}{rel}"))
    }
}

#[cfg(test)]
#[path = "skill_path_suggestion_tests.rs"]
mod tests;
