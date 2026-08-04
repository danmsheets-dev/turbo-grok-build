//! Filesystem choke point for `--confine`.
//!
//! Every write/delete goes through [`path_is_under_confine_root`] so tools that
//! bypass the permission manager (or race it via TOCTOU) still cannot leave the
//! root. Reads are left unrestricted — confine is a write boundary.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::computer::types::{AsyncFileSystem, ComputerError};
use crate::types::resources::{
    emit_confine_violation, path_is_under_confine_root, process_confine_root,
};

/// Decorator that refuses writes/deletes outside the process confine root.
pub struct ConfinedFs {
    inner: Arc<dyn AsyncFileSystem>,
    root: PathBuf,
}

impl ConfinedFs {
    /// Wrap `inner` so writes/deletes must stay under `root`.
    pub fn new(inner: Arc<dyn AsyncFileSystem>, root: PathBuf) -> Self {
        Self { inner, root }
    }

    /// Wrap `inner` with the process confine root when one is active; otherwise
    /// return `inner` unchanged.
    pub fn wrap_if_confined(inner: Arc<dyn AsyncFileSystem>) -> Arc<dyn AsyncFileSystem> {
        match process_confine_root() {
            Some(root) => Arc::new(Self::new(inner, root.clone())),
            None => inner,
        }
    }

    fn check_write(&self, path: &Path, op: &str) -> Result<(), ComputerError> {
        // RC13 Wave A: fail closed when the confine root itself is gone
        // (worktree tombstone / pruned isolation tree). Writing into a missing
        // root would either recreate a partial tree outside git or succeed on
        // the wrong volume after path rewrite — never allow that silently.
        crate::types::resources::enforce_write_roots(None, Some(self.root.as_path()))
            .map_err(|e| e.into_computer_error())?;
        if path_is_under_confine_root(path, &self.root) {
            return Ok(());
        }
        let root_s = self.root.display().to_string();
        let path_s = path.display().to_string();
        let canon = crate::types::resources::canonicalize_for_permission(path);
        let resolved = canon.display.to_string_lossy().into_owned();
        emit_confine_violation(
            op,
            &path_s,
            &resolved,
            &root_s,
            "fs-write-chokepoint",
        );
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
        let outside = std::env::temp_dir().join(format!(
            "hyper-confine-escape-{}.txt",
            std::process::id()
        ));
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
}
