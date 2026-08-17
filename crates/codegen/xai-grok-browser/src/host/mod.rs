//! Sidecar host entry (`turbo browser-host`).
//!
//! Task 3 resolves defaults and stubs the process. Task 4 replaces the
//! Windows body with a WebView2 HWND. This module must stay free of
//! WebView2 / Win32 dependencies.

use std::path::PathBuf;

use crate::profile::{agent_browser_user_data_dir, pipe_name};

/// Arguments for [`run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostArgs {
    /// Pager/session id (same segment used in the default pipe name).
    pub session_id: String,
    /// Named pipe. Empty means [`pipe_name`].
    pub pipe: String,
    /// WebView2 user-data-dir. Empty means [`agent_browser_user_data_dir`].
    pub user_data_dir: PathBuf,
}

/// Host startup failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    /// Sidecar is not implemented outside Windows.
    #[error("turbo browser-host is Windows-only in v1")]
    WindowsOnly,
}

impl HostError {
    /// Process exit code for this error (`2` for Windows-only).
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::WindowsOnly => 2,
        }
    }
}

impl HostArgs {
    /// Fill empty `pipe` / `user_data_dir` with product defaults.
    pub fn resolve_defaults(mut self) -> Self {
        if self.pipe.is_empty() {
            self.pipe = pipe_name(&self.session_id);
        }
        if self.user_data_dir.as_os_str().is_empty() {
            self.user_data_dir = agent_browser_user_data_dir();
        }
        self
    }
}

/// Run the Agent WebView sidecar (Windows stub in v1).
///
/// Non-Windows builds return [`HostError::WindowsOnly`] (CLI maps to exit 2).
pub fn run(args: HostArgs) -> Result<(), HostError> {
    let args = args.resolve_defaults();
    #[cfg(windows)]
    {
        eprintln!(
            "turbo browser-host: stub (session={} pipe={} user-data-dir={})",
            args.session_id,
            args.pipe,
            args.user_data_dir.display()
        );
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        eprintln!("turbo browser-host is Windows-only in v1");
        Err(HostError::WindowsOnly)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_pipe_and_profile_resolve_to_defaults() {
        let resolved = HostArgs {
            session_id: "abc".into(),
            pipe: String::new(),
            user_data_dir: PathBuf::new(),
        }
        .resolve_defaults();
        assert_eq!(resolved.session_id, "abc");
        assert_eq!(resolved.pipe, pipe_name("abc"));
        assert_eq!(resolved.user_data_dir, agent_browser_user_data_dir());
    }

    #[test]
    fn explicit_pipe_and_profile_are_kept() {
        let pipe = r"\\.\pipe\custom-browser";
        let dir = PathBuf::from("/tmp/custom-agent-browser");
        let resolved = HostArgs {
            session_id: "abc".into(),
            pipe: pipe.into(),
            user_data_dir: dir.clone(),
        }
        .resolve_defaults();
        assert_eq!(resolved.pipe, pipe);
        assert_eq!(resolved.user_data_dir, dir);
    }

    #[test]
    fn windows_only_maps_to_exit_code_two() {
        assert_eq!(HostError::WindowsOnly.exit_code(), 2);
    }

    #[cfg(windows)]
    #[test]
    fn windows_stub_returns_ok() {
        run(HostArgs {
            session_id: "stub-sess".into(),
            pipe: String::new(),
            user_data_dir: PathBuf::new(),
        })
        .expect("Windows stub must succeed");
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_is_windows_only() {
        let err = run(HostArgs {
            session_id: "stub-sess".into(),
            pipe: String::new(),
            user_data_dir: PathBuf::new(),
        })
        .expect_err("non-Windows host must refuse");
        assert_eq!(err, HostError::WindowsOnly);
        assert_eq!(err.exit_code(), 2);
    }
}
