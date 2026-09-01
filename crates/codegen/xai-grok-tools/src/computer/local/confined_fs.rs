//! Filesystem choke point for `--confine`.
//!
//! Every write/delete goes through [`path_is_under_confine_root`] so tools that
//! bypass the permission manager (or race it via TOCTOU) still cannot leave the
//! root. Reads are left unrestricted — confine is a write boundary.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::computer::types::{AsyncFileSystem, ComputerError};
use crate::types::resources::{emit_confine_violation, process_confine_roots};

/// Shared write-root list so `/folder add|remove` can update the chokepoint
/// without rebuilding the session FS stack.
pub type ConfineRootsHandle = Arc<RwLock<Vec<PathBuf>>>;

/// Decorator that refuses writes/deletes outside the process confine root.
pub struct ConfinedFs {
    inner: Arc<dyn AsyncFileSystem>,
    roots: ConfineRootsHandle,
    /// When true, an empty live root list is unconfined (session `/folder`
    /// start). When false, empty roots deny every write (process confine).
    empty_is_unconfined: bool,
}

impl ConfinedFs {
    /// Wrap `inner` so writes/deletes must stay under `root`.
    pub fn new(inner: Arc<dyn AsyncFileSystem>, root: PathBuf) -> Self {
        Self::with_roots(inner, vec![root])
    }

    /// Wrap `inner` so writes/deletes must stay under **any** of `roots`.
    /// Empty `roots` denies every write (fail closed — a confine wrapper with
    /// no roots is a misconfiguration, not unconfined).
    pub fn with_roots(inner: Arc<dyn AsyncFileSystem>, roots: Vec<PathBuf>) -> Self {
        Self::with_shared_roots(inner, Arc::new(RwLock::new(roots)))
    }

    /// Wrap `inner` with a shared root list (live `/folder` updates).
    pub fn with_shared_roots(inner: Arc<dyn AsyncFileSystem>, roots: ConfineRootsHandle) -> Self {
        Self {
            inner,
            roots,
            empty_is_unconfined: false,
        }
    }

    /// Session wrapper: empty roots mean unconfined; `/folder add` can jail later.
    pub fn with_shared_session_roots(
        inner: Arc<dyn AsyncFileSystem>,
        roots: ConfineRootsHandle,
    ) -> Self {
        Self {
            inner,
            roots,
            empty_is_unconfined: true,
        }
    }

    /// Handle for later [`set_roots`].
    pub fn roots_handle(&self) -> ConfineRootsHandle {
        Arc::clone(&self.roots)
    }

    /// Replace the live write-root set.
    pub fn set_roots(handle: &ConfineRootsHandle, roots: Vec<PathBuf>) {
        if let Ok(mut guard) = handle.write() {
            *guard = roots;
        }
    }

    /// Wrap `inner` with the process confine roots when any are active;
    /// otherwise return `inner` unchanged.
    pub fn wrap_if_confined(inner: Arc<dyn AsyncFileSystem>) -> Arc<dyn AsyncFileSystem> {
        let roots = process_confine_roots();
        if roots.is_empty() {
            inner
        } else {
            Arc::new(Self::with_roots(inner, roots.to_vec()))
        }
    }

    fn live_roots(&self) -> Vec<PathBuf> {
        let Ok(guard) = self.roots.read() else {
            return Vec::new();
        };
        guard
            .iter()
            .filter(|root| {
                std::fs::metadata(root)
                    .map(|m| m.is_dir())
                    .unwrap_or(false)
            })
            .cloned()
            .collect()
    }

    fn check_write(&self, path: &Path, op: &str) -> Result<(), ComputerError> {
        let live = self.live_roots();
        if live.is_empty() {
            if self.empty_is_unconfined {
                return Ok(());
            }
            let missing = self
                .roots
                .read()
                .ok()
                .and_then(|g| g.first().cloned())
                .unwrap_or_else(|| path.to_path_buf());
            return Err(
                crate::types::resources::WriteRootError::ConfineRootMissing { path: missing }
                    .into_computer_error(),
            );
        }
        if crate::types::resources::path_is_under_any_root(path, &live) {
            return Ok(());
        }
        let root_s = live
            .iter()
            .map(|r| r.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let path_s = path.display().to_string();
        let canon = crate::types::resources::canonicalize_for_permission(path);
        let resolved = canon.display.to_string_lossy().into_owned();
        emit_confine_violation(op, &path_s, &resolved, &root_s, "fs-write-chokepoint");
        Err(ComputerError::io_with_kind(
            format!(
                "Denied by confine root: `{path_s}` is outside `{root_s}` \
                 (resolved: `{resolved}`; rule=fs-write-chokepoint)"
            ),
            std::io::ErrorKind::PermissionDenied,
        ))
    }
}

#[async_trait::async_trait]
impl AsyncFileSystem for ConfinedFs {
    async fn read_file(&self, path: &Path) -> Result<Vec<u8>, ComputerError> {
        self.inner.read_file(path).await
    }

    async fn read_file_prefix(
        &self,
        path: &Path,
        max_bytes: usize,
    ) -> Result<Vec<u8>, ComputerError> {
        self.inner.read_file_prefix(path, max_bytes).await
    }

    async fn read_file_line_count(&self, path: &Path) -> Result<usize, ComputerError> {
        self.inner.read_file_line_count(path).await
    }

    async fn read_file_lines(
        &self,
        path: &Path,
        start_line: usize,
        limit: usize,
    ) -> Result<Vec<u8>, ComputerError> {
        self.inner.read_file_lines(path, start_line, limit).await
    }

    async fn read_file_ends_with_newline(&self, path: &Path) -> Result<bool, ComputerError> {
        self.inner.read_file_ends_with_newline(path).await
    }

    async fn write_file(&self, path: &Path, data: &[u8]) -> Result<(), ComputerError> {
        self.check_write(path, "fs.write_file")?;
        // Parent create is also a write-side effect — check the parent too when
        // it would be materialised outside the root.
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            self.check_write(parent, "fs.create_dir_all")?;
        }
        self.inner.write_file(path, data).await
    }

    async fn delete_file(&self, path: &Path) -> Result<(), ComputerError> {
        self.check_write(path, "fs.delete_file")?;
        self.inner.delete_file(path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer::local::LocalFs;

    #[tokio::test]
    async fn write_outside_root_is_denied() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root_path = dunce::canonicalize(root.path()).unwrap();
        // ConfinedFs carries its own root — do not stamp PROCESS_CONFINE_ROOT
        // (OnceLock first-write-wins would poison sibling tests with a dropped
        // tempdir path).
        let fs = ConfinedFs::new(Arc::new(LocalFs), root_path);
        let target = outside.path().join("pwned.txt");
        let err = fs
            .write_file(&target, b"nope")
            .await
            .expect_err("outside write must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("confine") || msg.contains("outside"),
            "expected confine denial, got: {msg}"
        );
        assert!(!target.exists(), "file must not have been written");
    }

    #[tokio::test]
    async fn write_inside_root_is_allowed() {
        let root = tempfile::tempdir().unwrap();
        let root_path = dunce::canonicalize(root.path()).unwrap();
        let fs = ConfinedFs::new(Arc::new(LocalFs), root_path);
        let target = root.path().join("ok.txt");
        fs.write_file(&target, b"yes").await.expect("inside write");
        assert_eq!(std::fs::read(&target).unwrap(), b"yes");
    }

    #[tokio::test]
    async fn write_fails_closed_when_confine_root_is_gone() {
        let root = tempfile::tempdir().unwrap();
        let root_path = dunce::canonicalize(root.path()).unwrap();
        let target = root.path().join("orphan.txt");
        let fs = ConfinedFs::new(Arc::new(LocalFs), root_path);
        // Drop the root directory (tombstone).
        drop(root);
        let err = fs
            .write_file(&target, b"nope")
            .await
            .expect_err("missing confine root must fail closed");
        let msg = err.to_string();
        assert!(
            msg.contains("worktree_tombstone") || msg.contains("cwd_missing"),
            "expected tombstone error, got: {msg}"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn absolute_windows_escape_is_denied() {
        let root = tempfile::tempdir().unwrap();
        let root_path = dunce::canonicalize(root.path()).unwrap();
        let fs = ConfinedFs::new(Arc::new(LocalFs), root_path);
        // Absolute path outside the worktree (drive-letter form).
        let outside =
            std::env::temp_dir().join(format!("hyper-confine-escape-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&outside);
        let err = fs
            .write_file(&outside, b"pwned")
            .await
            .expect_err("absolute outside must fail");
        assert!(
            err.to_string().contains("confine") || err.to_string().contains("outside"),
            "{}",
            err
        );
        assert!(!outside.exists());
    }

    #[tokio::test]
    async fn write_under_additional_root_is_allowed() {
        let primary = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        let primary_path = dunce::canonicalize(primary.path()).unwrap();
        let extra_path = dunce::canonicalize(extra.path()).unwrap();
        let fs = ConfinedFs::with_roots(
            Arc::new(LocalFs),
            vec![primary_path, extra_path.clone()],
        );
        let target = extra.path().join("from-extra.txt");
        fs.write_file(&target, b"extra")
            .await
            .expect("write under additional root");
        assert_eq!(std::fs::read(&target).unwrap(), b"extra");
        assert!(!extra_path.as_os_str().is_empty());
    }

    #[tokio::test]
    async fn write_outside_all_roots_is_denied_when_extras_attached() {
        let primary = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let fs = ConfinedFs::with_roots(
            Arc::new(LocalFs),
            vec![
                dunce::canonicalize(primary.path()).unwrap(),
                dunce::canonicalize(extra.path()).unwrap(),
            ],
        );
        let target = outside.path().join("pwned.txt");
        let err = fs
            .write_file(&target, b"nope")
            .await
            .expect_err("outside both roots must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("confine") || msg.contains("outside"),
            "expected confine denial, got: {msg}"
        );
        assert!(!target.exists());
    }
}
