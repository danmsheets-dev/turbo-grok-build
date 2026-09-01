//! Process-wide `--confine` / `--workspace-root` startup.
//!
//! Harnesses hand Turbo git worktree path(s) and need a hard guarantee that
//! writes and absolute path resolution stay under those roots. The OS sandbox
//! cannot provide that on Windows (advisory only); this is path-prefix
//! confinement, cross-platform.
//!
//! Roots are exported as `GROK_CONFINE` (`;`-separated) so nested `turbo`
//! processes, MCP servers, and hook subprocesses inherit the same boundary.
//! A nested process may tighten to subdirectories of inherited roots but
//! must not add a path that lies under none of them.

use std::path::{Path, PathBuf};

use xai_grok_tools::types::resources::{
    ENV_GROK_CONFINE, ENV_GROK_CONFINE_INHERIT, join_confine_path_list, parse_confine_path_list,
    path_is_under_any_root, process_confine_roots, set_process_confine_roots,
};

/// Whether this process currently has a confine root stamped (CLI `--confine`
/// or inherited `GROK_CONFINE`). Composition-root binaries use this instead of
/// depending on `xai-grok-tools` directly.
pub fn is_process_confined() -> bool {
    !process_confine_roots().is_empty()
}

/// Canonicalise `path`, verify it is an existing directory, and stamp it as
/// the (single) process confine root. See [`apply_confine_roots`].
pub fn apply_confine_root(path: &Path) -> anyhow::Result<()> {
    apply_confine_roots(&[path.to_path_buf()])
}

/// Apply CLI `--confine` paths, or inherit `GROK_CONFINE` when the flag is
/// omitted. Fail-fast so a typo never silently runs unconfined.
pub fn apply_process_confine(cli_paths: &[PathBuf]) -> anyhow::Result<()> {
    let paths = if !cli_paths.is_empty() {
        cli_paths.to_vec()
    } else {
        match std::env::var(ENV_GROK_CONFINE) {
            Ok(raw) => parse_confine_path_list(&raw),
            Err(_) => return Ok(()),
        }
    };
    if paths.is_empty() {
        return Ok(());
    }
    apply_confine_roots(&paths)
}

/// Canonicalise each path, verify it is an existing directory, and stamp the
/// process confine roots. Fail-fast with a clear error on typo / missing
/// path so harnesses never silently run unconfined.
///
/// Also exports `GROK_CONFINE` (`;`-joined) and `GROK_CONFINE_INHERIT=1`.
/// When `GROK_CONFINE` is already set (nested turbo), every new root must
/// lie under **some** inherited root — adding a sibling outside the inherited
/// set is a hard startup error. Dropping inherited roots (tightening) is
/// allowed.
pub fn apply_confine_roots(paths: &[PathBuf]) -> anyhow::Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut canonicals: Vec<PathBuf> = Vec::with_capacity(paths.len());
    for path in paths {
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
        if !canonicals.iter().any(|existing| {
            canonicalize_compare(existing) == canonicalize_compare(&canonical)
        }) {
            canonicals.push(canonical);
        }
    }

    let inherited = inherited_confine_roots()?;
    if !inherited.is_empty() {
        for canonical in &canonicals {
            if !path_is_under_any_root(canonical, &inherited)
                && !inherited
                    .iter()
                    .any(|inh| canonicalize_compare(canonical) == canonicalize_compare(inh))
            {
                anyhow::bail!(
                    "--confine: cannot widen beyond inherited GROK_CONFINE roots {} \
                     (requested `{}`). Nested turbo may only tighten confinement.",
                    join_confine_path_list(&inherited),
                    canonical.display()
                );
            }
        }
    }

    set_process_confine_roots(canonicals.clone());
    // SAFETY: process startup only; no concurrent env readers that assume
    // these keys are immutable. Export so descendants inherit the boundary.
    let exported = join_confine_path_list(&canonicals);
    unsafe {
        std::env::set_var(ENV_GROK_CONFINE, &exported);
        std::env::set_var(ENV_GROK_CONFINE_INHERIT, "1");
    }
    Ok(())
}

/// Resolve already-exported `GROK_CONFINE` (from a parent turbo) if present.
fn inherited_confine_roots() -> anyhow::Result<Vec<PathBuf>> {
    let Ok(raw) = std::env::var(ENV_GROK_CONFINE) else {
        return Ok(Vec::new());
    };
    let parsed = parse_confine_path_list(&raw);
    if parsed.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(parsed.len());
    for path in parsed {
        match dunce::canonicalize(&path) {
            Ok(c) => out.push(c),
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "--confine: inherited GROK_CONFINE `{}` is not a usable directory: {e}",
                    path.display()
                ));
            }
        }
    }
    Ok(out)
}

fn canonicalize_compare(path: &Path) -> PathBuf {
    xai_grok_tools::types::resources::canonicalize_for_permission(path).compare
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use xai_grok_tools::types::resources::path_is_under_confine_root;

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

    #[test]
    fn apply_confine_roots_exports_semicolon_list() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        unsafe {
            std::env::remove_var(ENV_GROK_CONFINE);
            std::env::remove_var(ENV_GROK_CONFINE_INHERIT);
        }
        apply_confine_roots(&[a.path().to_path_buf(), b.path().to_path_buf()]).expect("apply");
        let exported = std::env::var(ENV_GROK_CONFINE).expect("GROK_CONFINE set");
        let parsed = parse_confine_path_list(&exported);
        assert_eq!(parsed.len(), 2, "exported {exported}");
        unsafe {
            std::env::remove_var(ENV_GROK_CONFINE);
            std::env::remove_var(ENV_GROK_CONFINE_INHERIT);
        }
    }

    #[test]
    fn apply_confine_roots_refuses_sibling_outside_any_inherited() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let sibling = tempfile::tempdir().unwrap();
        let a_canon = dunce::canonicalize(a.path()).unwrap();
        let b_canon = dunce::canonicalize(b.path()).unwrap();
        unsafe {
            std::env::set_var(
                ENV_GROK_CONFINE,
                join_confine_path_list(&[a_canon, b_canon]),
            );
            std::env::set_var(ENV_GROK_CONFINE_INHERIT, "1");
        }
        let err = apply_confine_roots(&[sibling.path().to_path_buf()])
            .expect_err("must refuse widen");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("cannot widen") || msg.contains("inherited"),
            "expected widen error, got: {msg}"
        );
        unsafe {
            std::env::remove_var(ENV_GROK_CONFINE);
            std::env::remove_var(ENV_GROK_CONFINE_INHERIT);
        }
    }

    #[test]
    fn apply_confine_roots_allows_tighten_of_second_inherited() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let nested = b.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let a_canon = dunce::canonicalize(a.path()).unwrap();
        let b_canon = dunce::canonicalize(b.path()).unwrap();
        unsafe {
            std::env::set_var(
                ENV_GROK_CONFINE,
                join_confine_path_list(&[a_canon, b_canon]),
            );
            std::env::set_var(ENV_GROK_CONFINE_INHERIT, "1");
        }
        let result = apply_confine_roots(&[nested]);
        unsafe {
            std::env::remove_var(ENV_GROK_CONFINE);
            std::env::remove_var(ENV_GROK_CONFINE_INHERIT);
        }
        assert!(
            result.is_ok(),
            "tightening to a subdir of the second inherited root must be allowed: {result:?}"
        );
    }
}
