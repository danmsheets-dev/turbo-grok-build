//! Process-wide `--confine` / `--workspace-root` startup.
//!
//! Harnesses hand Hyper a git worktree path and need a hard guarantee that
//! writes and absolute path resolution stay under that root. The OS sandbox
//! cannot provide that on Windows (advisory only); this is path-prefix
//! confinement, cross-platform.
//!
//! The root is also exported as `GROK_CONFINE` so nested `hyper` processes,
//! MCP servers, and hook subprocesses inherit the same boundary. A nested
//! process may tighten the root to a subdirectory but must not widen it.

use std::path::{Path, PathBuf};

use xai_grok_tools::types::resources::{
    ENV_GROK_CONFINE, ENV_GROK_CONFINE_INHERIT, path_is_under_confine_root,
    process_confine_root, set_process_confine_root,
};

/// Whether this process currently has a confine root stamped (CLI `--confine`
/// or inherited `GROK_CONFINE`). Composition-root binaries use this instead of
/// depending on `xai-grok-tools` directly.
pub fn is_process_confined() -> bool {
    process_confine_root().is_some()
}

/// Canonicalise `path`, verify it is an existing directory, and stamp it as
/// the process confine root. Fail-fast with a clear error on typo / missing
/// path so harnesses never silently run unconfined.
///
/// Also exports `GROK_CONFINE` (and `GROK_CONFINE_INHERIT=1`) so every child
/// process inherits the root. When `GROK_CONFINE` is already set (nested
/// hyper), the new root must itself lie under the inherited root — widening
/// is a hard startup error.
pub fn apply_confine_root(path: &Path) -> anyhow::Result<()> {
    let meta = std::fs::metadata(path).map_err(|e| {
        anyhow::anyhow!(
            "--confine: path `{}` does not exist or is inaccessible: {e}",
            path.display()
        )
    })?;
    if !meta.is_dir() {
        anyhow::bail!("--confine: path `{}` is not a directory", path.display());
    }
    let canonical = dunce::canonicalize(path).map_err(|e| {
        anyhow::anyhow!(
            "--confine: failed to canonicalize `{}`: {e}",
            path.display()
        )
    })?;

    // Nested hyper: refuse to widen beyond the inherited confine root.
    if let Some(inherited) = inherited_confine_root()? {
        if !path_is_under_confine_root(&canonical, &inherited)
            && canonicalize_compare(&canonical) != canonicalize_compare(&inherited)
        {
            anyhow::bail!(
                "--confine: cannot widen beyond inherited GROK_CONFINE root `{}` \
                 (requested `{}`). Nested hyper may only tighten confinement.",
                inherited.display(),
                canonical.display()
            );
        }
    }

    set_process_confine_root(canonical.clone());
    // SAFETY: process startup only; no concurrent env readers that assume
    // these keys are immutable. Export so descendants inherit the boundary.
    unsafe {
        std::env::set_var(ENV_GROK_CONFINE, &canonical);
        std::env::set_var(ENV_GROK_CONFINE_INHERIT, "1");
    }
    Ok(())
}

/// Resolve an already-exported `GROK_CONFINE` (from a parent hyper) if present.
fn inherited_confine_root() -> anyhow::Result<Option<PathBuf>> {
    let Ok(raw) = std::env::var(ENV_GROK_CONFINE) else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Ok(None);
    }
    let path = PathBuf::from(raw);
    // If the process root is already stamped, that is the effective inherit
    // baseline (parent already applied). Prefer the env form so a fresh
    // nested process that has not yet set OnceLock still sees the parent.
    match dunce::canonicalize(&path) {
        Ok(c) => Ok(Some(c)),
        Err(e) => Err(anyhow::anyhow!(
            "--confine: inherited GROK_CONFINE `{}` is not a usable directory: {e}",
            path.display()
        )),
    }
}

fn canonicalize_compare(path: &Path) -> PathBuf {
    xai_grok_tools::types::resources::canonicalize_for_permission(path).compare
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Env mutation is process-global; serialise tests that touch GROK_CONFINE.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn apply_confine_root_exports_grok_confine() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        // Clear any prior inherit so this test owns the env.
        unsafe {
            std::env::remove_var(ENV_GROK_CONFINE);
            std::env::remove_var(ENV_GROK_CONFINE_INHERIT);
        }
        apply_confine_root(&root).expect("apply");
        let exported = std::env::var(ENV_GROK_CONFINE).expect("GROK_CONFINE set");
        let exported_path = PathBuf::from(exported);
        assert!(
            path_is_under_confine_root(&exported_path, &root)
                || dunce::canonicalize(&exported_path).ok().as_ref()
                    == dunce::canonicalize(&root).ok().as_ref(),
            "exported GROK_CONFINE must match applied root"
        );
        assert_eq!(
            std::env::var(ENV_GROK_CONFINE_INHERIT).ok().as_deref(),
            Some("1")
        );
    }

    #[test]
    fn apply_confine_root_refuses_to_widen_inherited() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let outer = tempfile::tempdir().unwrap();
        let sibling = tempfile::tempdir().unwrap();
        let outer_canon = dunce::canonicalize(outer.path()).unwrap();
        unsafe {
            std::env::set_var(ENV_GROK_CONFINE, &outer_canon);
            std::env::set_var(ENV_GROK_CONFINE_INHERIT, "1");
        }
        let err = apply_confine_root(sibling.path()).expect_err("must refuse widen");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("cannot widen") || msg.contains("inherited"),
            "expected widen error, got: {msg}"
        );
        // Cleanup so other tests see a clean env.
        unsafe {
            std::env::remove_var(ENV_GROK_CONFINE);
            std::env::remove_var(ENV_GROK_CONFINE_INHERIT);
        }
    }

    #[test]
    fn apply_confine_root_allows_tighten_to_subdir() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let outer = tempfile::tempdir().unwrap();
        let inner = outer.path().join("nested");
        std::fs::create_dir(&inner).unwrap();
        let outer_canon = dunce::canonicalize(outer.path()).unwrap();
        unsafe {
            std::env::set_var(ENV_GROK_CONFINE, &outer_canon);
            std::env::set_var(ENV_GROK_CONFINE_INHERIT, "1");
        }
        // OnceLock may already be set by a prior test in this process; the
        // widen check is what we care about here (must not error).
        let result = apply_confine_root(&inner);
        unsafe {
            std::env::remove_var(ENV_GROK_CONFINE);
            std::env::remove_var(ENV_GROK_CONFINE_INHERIT);
        }
        assert!(
            result.is_ok(),
            "tightening to a subdir must be allowed: {result:?}"
        );
    }
}
