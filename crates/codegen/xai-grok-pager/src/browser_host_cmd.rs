//! Hidden `turbo browser-host` sidecar (Agent WebView2 window).
//!
//! Do not run this interactively. The TUI launches the same `turbo.exe` with
//! this subcommand and talks JSON-RPC over a session-private named pipe.

use std::path::PathBuf;

use xai_grok_browser::host::{HostArgs, HostError};

/// CLI args for `turbo browser-host`.
#[derive(Debug, clap::Args, Clone, PartialEq, Eq)]
pub struct BrowserHostArgs {
    /// Pager/session id (same segment used in the pipe name).
    #[arg(long)]
    pub session_id: String,
    /// Named pipe. Default: \\.\pipe\turbo-browser-<session_id>
    #[arg(long)]
    pub pipe: Option<String>,
    /// WebView2 user-data-dir. Default: $GROK_HOME/agent-browser
    #[arg(long)]
    pub user_data_dir: Option<PathBuf>,
}

/// Run the sidecar host and return after it exits.
pub fn run(args: BrowserHostArgs) -> Result<(), HostError> {
    xai_grok_browser::host::run(HostArgs {
        session_id: args.session_id,
        pipe: args.pipe.unwrap_or_default(),
        user_data_dir: args.user_data_dir.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::cli::{Command, PagerArgs};
    use clap::{CommandFactory, Parser};

    #[test]
    fn parses_required_session_id() {
        let args = PagerArgs::try_parse_from(["turbo", "browser-host", "--session-id", "sess-1"])
            .expect("browser-host parses");
        match args.command {
            Some(Command::BrowserHost(host)) => {
                assert_eq!(
                    host,
                    BrowserHostArgs {
                        session_id: "sess-1".into(),
                        pipe: None,
                        user_data_dir: None,
                    }
                );
            }
            other => panic!("expected BrowserHost, got {other:?}"),
        }
    }

    #[test]
    fn parses_pipe_and_user_data_dir_overrides() {
        let args = PagerArgs::try_parse_from([
            "turbo",
            "browser-host",
            "--session-id",
            "sess-2",
            "--pipe",
            r"\\.\pipe\custom",
            "--user-data-dir",
            r"C:\tmp\agent-browser",
        ])
        .expect("browser-host overrides parse");
        match args.command {
            Some(Command::BrowserHost(host)) => {
                assert_eq!(host.session_id, "sess-2");
                assert_eq!(host.pipe.as_deref(), Some(r"\\.\pipe\custom"));
                assert_eq!(
                    host.user_data_dir.as_deref(),
                    Some(std::path::Path::new(r"C:\tmp\agent-browser"))
                );
            }
            other => panic!("expected BrowserHost, got {other:?}"),
        }
    }

    #[test]
    fn session_id_is_required() {
        assert!(PagerArgs::try_parse_from(["turbo", "browser-host"]).is_err());
    }

    #[test]
    fn hidden_from_top_level_help() {
        let help = PagerArgs::command().render_help().to_string();
        assert!(
            !help.contains("browser-host"),
            "hidden sidecar must not appear in --help: {help}"
        );
    }
}
