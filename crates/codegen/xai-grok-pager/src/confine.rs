//! Process-wide `--confine` / `--workspace-root` startup.
//!
//! Harnesses hand Hyper a git worktree path and need a hard guarantee that
//! writes and absolute path resolution stay under that root. The OS sandbox
//! cannot provide that on Windows (advisory only); this is path-prefix
//! confinement, cross-platform.

use std::path::Path;

/// Canonicalise `path`, verify it is an existing directory, and stamp it as
/// the process confine root. Fail-fast with a clear error on typo / missing
/// path so harnesses never silently run unconfined.
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
    xai_grok_tools::types::resources::set_process_confine_root(canonical);
    Ok(())
}
