//! Workspace identity from canonical absolute paths.

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};

/// Prefix used for workspace ids (`w_<hex>`).
pub const WORKSPACE_ID_PREFIX: &str = "w_";

/// Resolve a workspace root to a canonical absolute path (Windows-safe via `dunce`).
///
/// - Absolute paths are canonicalized when they exist; otherwise lexically normalized.
/// - Relative paths join `current_dir` first.
/// - Trailing separators are stripped.
/// - On Windows, drive letter is uppercased for stable identity display.
pub fn canonicalize_root(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let cwd = std::env::current_dir().map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        cwd.join(path)
    };

    let canonical = dunce::canonicalize(&absolute).unwrap_or_else(|_| {
        // Path may not exist yet (tests); fall back to lexical normalize.
        normalize_lexically(&absolute)
    });

    Ok(strip_trailing_sep(canonical))
}

/// Compute `workspace_id` from a path: `w_` + blake3(canonical path string).
///
/// The hash input is the canonical absolute path rendered with `/` separators
/// and, on Windows, lowercased so case variants of the same folder share an id.
pub fn workspace_id_for_path(path: &Path) -> Result<String> {
    let canonical = canonicalize_root(path)?;
    Ok(workspace_id_for_canonical(&canonical))
}

/// Compute workspace id from an already-canonical path.
pub fn workspace_id_for_canonical(canonical: &Path) -> String {
    let mut key = path_to_identity_key(canonical);
    if cfg!(windows) {
        key = key.to_ascii_lowercase();
    }
    let hash = blake3::hash(key.as_bytes());
    let hex = hash.to_hex();
    // 32 hex chars is plenty for local store uniqueness and keeps dir names short.
    format!("{WORKSPACE_ID_PREFIX}{}", &hex[..32])
}

/// Stable string form of a path for hashing / comparison.
pub fn path_to_identity_key(path: &Path) -> String {
    let s = path.to_string_lossy();
    // Normalize separators to `/` for cross-platform identity stability in tests.
    let mut out = s.replace('\\', "/");
    while out.ends_with('/') && out.len() > 1 {
        // Keep root `/` or `C:/` style
        if out.len() == 3 && out.as_bytes()[1] == b':' {
            break;
        }
        if out == "/" {
            break;
        }
        out.pop();
    }
    // Uppercase Windows drive letter for display consistency in key before lowercasing.
    if out.len() >= 2 && out.as_bytes()[1] == b':' {
        let mut chars: Vec<char> = out.chars().collect();
        chars[0] = chars[0].to_ascii_uppercase();
        out = chars.into_iter().collect();
    }
    out
}

fn strip_trailing_sep(mut path: PathBuf) -> PathBuf {
    // PathBuf normally doesn't keep trailing sep, but be defensive for string roots.
    let s = path.to_string_lossy();
    if (s.ends_with('/') || s.ends_with('\\')) && s.len() > 1 {
        let trimmed = s.trim_end_matches(['/', '\\']);
        // Preserve Windows drive root `C:\`
        if trimmed.ends_with(':') {
            return path;
        }
        path = PathBuf::from(trimmed);
    }
    path
}

fn normalize_lexically(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(s) => out.push(s),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// Resolve `$GROK_HOME` or `~/.grok` without depending on `xai-grok-config`.
pub fn resolve_grok_home() -> PathBuf {
    if let Ok(v) = std::env::var("GROK_HOME") {
        return PathBuf::from(v);
    }
    #[allow(deprecated)]
    let home = std::env::home_dir()
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .or_else(|| dirs::home_dir())
        .unwrap_or_else(|| PathBuf::from("."));
    dunce::canonicalize(&home).unwrap_or(home).join(".grok")
}

/// Default durable store root: `~/.grok/workspace-trees`.
pub fn default_store_root() -> PathBuf {
    resolve_grok_home().join("workspace-trees")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_id_is_stable_and_prefixed() {
        let p = if cfg!(windows) {
            Path::new(r"C:\Apps\demo")
        } else {
            Path::new("/tmp/demo-workspace-tree-id")
        };
        // May not exist; still produces id from lexical path.
        let id1 = workspace_id_for_path(p).unwrap();
        let id2 = workspace_id_for_path(p).unwrap();
        assert_eq!(id1, id2);
        assert!(id1.starts_with("w_"));
        assert_eq!(id1.len(), 2 + 32);
    }

    #[test]
    #[cfg(windows)]
    fn windows_case_folds_for_id() {
        let a = workspace_id_for_canonical(Path::new(r"C:\Foo\Bar"));
        let b = workspace_id_for_canonical(Path::new(r"c:\foo\bar"));
        assert_eq!(a, b);
    }
}
