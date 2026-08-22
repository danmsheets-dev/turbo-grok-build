//! Session policy engine v1 (Phase 5 FR fr_01a028e7bf6875339bfa220a9adc5c9b).
//!
//! Enforced at the two built-in mutation chokepoints — [`crate::implementations::grok_build::search_replace`]
//! (every file edit) and [`crate::implementations::grok_build::bash`] (shell
//! effects) — so violations fail the tool closed with a named error instead of
//! relying on prompt text. MCP tools keep their own permission classification
//! (`AccessKind::MCPTool`); extending policy there is a documented follow-up.
//!
//! Config sources (later wins):
//! 1. Env: `GROK_POLICY_DENY_PATHS` / `GROK_POLICY_DENY_COMMANDS`
//!    (`;`-separated), `GROK_POLICY_MAX_DIFF_LINES` (integer).
//! 2. `Params<PolicyParams>` when a host injects one (same fields).

use serde::{Deserialize, Serialize};

/// Host/config-injected policy params.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyParams {
    /// Path fragments refused for edits (component-suffix match, case-insensitive).
    #[serde(default)]
    pub deny_paths: Vec<String>,
    /// Command substrings refused in bash (case-insensitive).
    #[serde(default)]
    pub deny_commands: Vec<String>,
    /// Refuse an edit whose replacement adds more than this many lines.
    #[serde(default)]
    pub max_diff_lines: Option<u64>,
}

impl PolicyParams {
    /// Resolve from env first (so operators can gate without code changes),
    /// falling back to injected params for unset keys.
    pub fn resolve(injected: Option<&Self>) -> Self {
        let mut out = injected.cloned().unwrap_or_default();
        if let Ok(v) = std::env::var("GROK_POLICY_DENY_PATHS") {
            let parsed = split_env_list(&v);
            if !parsed.is_empty() {
                out.deny_paths = parsed;
            }
        }
        if let Ok(v) = std::env::var("GROK_POLICY_DENY_COMMANDS") {
            let parsed = split_env_list(&v);
            if !parsed.is_empty() {
                out.deny_commands = parsed;
            }
        }
        if let Ok(v) = std::env::var("GROK_POLICY_MAX_DIFF_LINES")
            && let Ok(n) = v.trim().parse::<u64>()
        {
            out.max_diff_lines = Some(n);
        }
        out
    }

    /// True when any deny-path fragment matches a path component suffix
    /// (`target/` matches `repo/target/debug`, `.env` matches `.env.local`).
    pub fn path_denied(&self, path: &std::path::Path) -> Option<String> {
        if self.deny_paths.is_empty() {
            return None;
        }
        let normalized = path.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
        for frag in &self.deny_paths {
            let frag = frag.trim();
            if frag.is_empty() {
                continue;
            }
            let needle = frag.replace('\\', "/").to_ascii_lowercase();
            let needle = needle.strip_suffix('/').unwrap_or(&needle);
            if normalized.contains(&format!("/{needle}/"))
                || normalized.ends_with(&format!("/{needle}"))
                || normalized.split('/').any(|c| {
                    c == needle
                        || (needle.starts_with('.')
                            && c.starts_with(needle)
                            && c != needle)
                })
            {
                return Some(frag.to_owned());
            }
        }
        None
    }

    /// True when any deny-command substring appears in the command.
    pub fn command_denied(&self, command: &str) -> Option<String> {
        let lower = command.to_ascii_lowercase();
        self.deny_commands
            .iter()
            .filter(|f| !f.trim().is_empty())
            .find(|f| lower.contains(f.trim().to_ascii_lowercase().as_str()))
            .cloned()
    }

    /// True when an edit adding `added_lines` exceeds the configured ceiling.
    pub fn diff_exceeds_limit(&self, added_lines: u64) -> Option<(u64, u64)> {
        let max = self.max_diff_lines?;
        if added_lines > max {
            Some((added_lines, max))
        } else {
            None
        }
    }
}

fn split_env_list(raw: &str) -> Vec<String> {
    raw.split([';', ','])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Named refusal message builders shared by the enforcement sites.
pub fn denial(tool: &str, rule: &str, detail: &str) -> String {
    format!("policy denied ({tool}): rule `{rule}` matched {detail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(deny_paths: &[&str], deny_commands: &[&str], max: Option<u64>) -> PolicyParams {
        PolicyParams {
            deny_paths: deny_paths.iter().map(|s| s.to_string()).collect(),
            deny_commands: deny_commands.iter().map(|s| s.to_string()).collect(),
            max_diff_lines: max,
        }
    }

    #[test]
    fn deny_paths_match_components_and_suffixes() {
        let p = policy(&["production", ".env"], &[], None);
        assert!(p.path_denied(std::path::Path::new("src/production/main.rs")).is_some());
        assert!(p.path_denied(std::path::Path::new(".env")).is_some());
        assert!(p.path_denied(std::path::Path::new("config/.env.local")).is_some());
        assert!(p.path_denied(std::path::Path::new("src/lib/producer.rs")).is_none());
        assert!(p.path_denied(std::path::Path::new("src/main.rs")).is_none());
    }

    #[test]
    fn deny_commands_match_case_insensitively() {
        let p = policy(&[], &["terraform destroy", "drop table"], None);
        assert!(p.command_denied("Terraform Destroy -auto-approve").is_some());
        assert!(p.command_denied("psql -c 'DROP TABLE users'").is_some());
        assert!(p.command_denied("cargo build").is_none());
    }

    #[test]
    fn diff_limit_reports_pair_when_exceeded() {
        let p = policy(&[], &[], Some(200));
        assert_eq!(p.diff_exceeds_limit(199), None);
        assert_eq!(p.diff_exceeds_limit(201), Some((201, 200)));
        assert!(policy(&[], &[], None).diff_exceeds_limit(999_999).is_none());
    }

    #[test]
    fn denial_message_names_tool_and_rule() {
        let msg = denial("search_replace", "deny_paths", "production");
        assert!(msg.starts_with("policy denied (search_replace)"));
        assert!(msg.contains("deny_paths"));
    }
}
