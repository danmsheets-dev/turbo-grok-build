//! `turbo secret get <name>` — return a vault handle, never the raw secret.

use anyhow::{Result, bail};
use clap::Subcommand;

#[derive(Debug, clap::Args, Clone)]
pub struct SecretArgs {
    #[command(subcommand)]
    pub command: SecretCommand,
}

#[derive(Debug, Subcommand, Clone)]
pub enum SecretCommand {
    /// Resolve a named secret to an opaque handle (never prints the value)
    Get {
        /// Secret name (`$GROK_HOME/secrets/<name>` or `GROK_SECRET_<NAME>`)
        name: String,
        /// Emit JSON `{handle,name,source}` (still no secret bytes)
        #[arg(long)]
        json: bool,
    },
}

pub fn run(args: SecretArgs) -> Result<()> {
    match args.command {
        SecretCommand::Get { name, json } => match xai_grok_secrets::secret_get(&name, json) {
            Ok(out) => {
                println!("{out}");
                Ok(())
            }
            Err(xai_grok_secrets::VaultError::NotFound) => bail!(
                "secret `{name}` not found (env {} or {})",
                xai_grok_secrets::env_key_for_secret_name(&name),
                xai_grok_secrets::secrets_dir(&xai_grok_config::grok_home())
                    .join(&name)
                    .display()
            ),
            Err(xai_grok_secrets::VaultError::InvalidName) => {
                bail!("invalid secret name `{name}` (use [A-Za-z0-9_][A-Za-z0-9_-]{{0,63}})")
            }
            Err(e) => bail!("{e}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Command, PagerArgs};
    use clap::Parser as _;

    fn parse_secret(argv: &[&str]) -> SecretCommand {
        let args = PagerArgs::try_parse_from(argv).expect("args should parse");
        match args.command {
            Some(Command::Secret(SecretArgs { command })) => command,
            other => panic!("expected secret, got {other:?}"),
        }
    }

    #[test]
    fn secret_get_parses_name() {
        let cmd = parse_secret(&["turbo", "secret", "get", "github_token"]);
        match cmd {
            SecretCommand::Get { name, json } => {
                assert_eq!(name, "github_token");
                assert!(!json);
            }
        }
    }

    #[test]
    fn missing_secret_fails_closed() {
        let err = xai_grok_secrets::Vault::default()
            .get_handle("no_such_secret")
            .unwrap_err();
        assert!(err.is_not_found());
        let canary = ["missing", "CanaryValue99"].concat();
        assert!(!format!("{err}").contains(&canary));
    }
}
