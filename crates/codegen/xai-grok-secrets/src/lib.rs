//! Named secrets vault and outbound redaction for Turbo Build.
//!
//! - Files: `$GROK_HOME/secrets/<name>` (Unix 0600 / owner-only)
//! - Env: `GROK_SECRET_<NAME>` (hyphens fold to `_`, env wins)
//! - [`secret_get`] returns `vault:<name>` — never raw bytes
//! - [`apply_vault_to_mcp_env`] injects values into MCP stdio env at handshake
//! - [`redact_secrets`] also substitutes installed vault values (developer-log)

mod sanitizer;
pub mod vault;

pub use sanitizer::{
    MIN_VAULT_REDACT_LEN, REDACTED, redact_json_string_values, redact_known_secret_values,
    redact_secrets, redact_secrets_with_values, redact_url, redact_user_paths, walk_json_strings,
};
pub use vault::{
    SecretHandle, SecretSource, SecretValue, Vault, VaultError, apply_vault_to_mcp_env,
    ensure_process_vault_redaction, env_key_for_secret_name, handle_json,
    install_vault_redaction_values, is_valid_secret_name, secret_get, secret_get_from, secrets_dir,
};
