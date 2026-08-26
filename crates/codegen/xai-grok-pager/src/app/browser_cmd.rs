//! `turbo browser` — Agent WebView profile maintenance.

use anyhow::{Context, Result};
use clap::Subcommand;
use xai_grok_browser::profile::{agent_browser_user_data_dir, reset_agent_browser_profile};

/// CLI args for `turbo browser`.
#[derive(Debug, clap::Args, Clone)]
pub struct BrowserArgs {
    #[command(subcommand)]
    pub command: BrowserCommand,
}

/// Subcommands under `turbo browser`.
#[derive(Debug, Subcommand, Clone)]
pub enum BrowserCommand {
    /// Delete the persisted Agent WebView profile so grok.com login starts clean
    #[command(name = "reset-profile")]
    ResetProfile {
        /// Print the profile path without deleting it
        #[arg(long)]
        dry_run: bool,
    },
}

/// Run `turbo browser …` and return after it prints.
pub fn run(args: BrowserArgs) -> Result<()> {
    match args.command {
        BrowserCommand::ResetProfile { dry_run } => reset_profile(dry_run),
    }
}

fn reset_profile(dry_run: bool) -> Result<()> {
    let dir = agent_browser_user_data_dir();
    if dry_run {
        println!("would reset Agent WebView profile at {}", dir.display());
        return Ok(());
    }
    match reset_agent_browser_profile() {
        Ok(true) => {
            println!("reset Agent WebView profile at {}", dir.display());
            Ok(())
        }
        Ok(false) => {
            println!("Agent WebView profile not present at {}", dir.display());
            Ok(())
        }
        Err(e) => Err(e)
            .with_context(|| format!("failed to reset Agent WebView profile at {}", dir.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::cli::{Command, PagerArgs};
    use clap::Parser;

    #[test]
    fn parses_reset_profile() {
        let args = PagerArgs::try_parse_from(["turbo", "browser", "reset-profile"])
            .expect("browser reset-profile parses");
        match args.command {
            Some(Command::Browser(BrowserArgs {
                command: BrowserCommand::ResetProfile { dry_run: false },
            })) => {}
            other => panic!("expected Browser reset-profile, got {other:?}"),
        }
    }

    #[test]
    fn parses_reset_profile_dry_run() {
        let args = PagerArgs::try_parse_from(["turbo", "browser", "reset-profile", "--dry-run"])
            .expect("browser reset-profile --dry-run parses");
        match args.command {
            Some(Command::Browser(BrowserArgs {
                command: BrowserCommand::ResetProfile { dry_run: true },
            })) => {}
            other => panic!("expected dry-run, got {other:?}"),
        }
    }
}
