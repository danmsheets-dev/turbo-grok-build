//! Session policy engine v1 (Phase 5 FR fr_01a028e7bf6875339bfa220a9adc5c9b).
//!
//! Enforced before effects at [`crate::implementations::grok_build::search_replace`],
//! OpenCode `write`, Codex `apply_patch`, [`crate::implementations::grok_build::bash`],
//! and [`crate::implementations::grok_build::monitor`]. File-edit sites enforce
//! `deny_paths` and `max_diff_lines`; command sites enforce `deny_commands`.
//! MCP tools keep their own permission classification (`AccessKind::MCPTool`).
//!
//! V1 configuration source: `GROK_POLICY_DENY_PATHS` /
//! `GROK_POLICY_DENY_COMMANDS` (`;`-separated) and
//! `GROK_POLICY_MAX_DIFF_LINES` (integer). `Params<PolicyParams>` is honored
//! when a host injects it, but no host-injection configuration path is provided
//! by this crate.

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
        let normalized = path
            .to_string_lossy()
            .replace('\\', "/")
            .to_ascii_lowercase();
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
                    c == needle || (needle.starts_with('.') && c.starts_with(needle) && c != needle)
                })
            {
                return Some(frag.to_owned());
            }
        }
        None
    }

    /// True when any deny-command substring appears in the command.
    ///
    /// Matches the raw string and a dequoted/whitespace-collapsed haystack so
    /// `cu''rl`, `cur\l`, and `rm  -rf` cannot dodge a `curl` / `rm -rf` rule
    /// (F59). ANSI-C `$'\143url'` is decoded. This is still not a full shell
    /// parser — glob/`eval` assembly can evade; those belong on `[permission]`.
    pub fn command_denied(&self, command: &str) -> Option<String> {
        let raw = command.to_ascii_lowercase();
        let dequoted = dequote_shell_haystack(command).to_ascii_lowercase();
        self.deny_commands.iter().find(|f| {
            let needle = f.trim().to_ascii_lowercase();
            if needle.is_empty() {
                return false;
            }
            let collapsed = collapse_ws(&needle);
            raw.contains(&needle)
                || dequoted.contains(&needle)
                || collapse_ws(&raw).contains(&collapsed)
                || collapse_ws(&dequoted).contains(&collapsed)
        }).cloned()
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

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
            }
            prev_space = true;
        } else {
            prev_space = false;
            out.push(c);
        }
    }
    out
}

/// Strip shell quotes/backslashes and decode `$'\ooo'` / `$'\xHH'` so a
/// substring deny list sees the command the shell will run, not the spelling.
fn dequote_shell_haystack(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        // ANSI-C quoting: $'...'
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\'' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 1;
                    match bytes[i] {
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        b'\\' | b'\'' => out.push(bytes[i] as char),
                        b'x' if i + 2 < bytes.len() => {
                            let hex = &s[i + 1..i + 3];
                            if let Ok(v) = u8::from_str_radix(hex, 16) {
                                out.push(v as char);
                                i += 2;
                            } else {
                                out.push('x');
                            }
                        }
                        d if d.is_ascii_digit() => {
                            let mut val = (d - b'0') as u32;
                            let mut n = 1;
                            while n < 3
                                && i + n < bytes.len()
                                && bytes[i + n].is_ascii_digit()
                            {
                                val = val * 8 + (bytes[i + n] - b'0') as u32;
                                n += 1;
                            }
                            if let Some(ch) = char::from_u32(val) {
                                out.push(ch);
                            }
                            i += n - 1;
                        }
                        other => out.push(other as char),
                    }
                    i += 1;
                    continue;
                }
                out.push(bytes[i] as char);
                i += 1;
            }
            if i < bytes.len() {
                i += 1; // closing quote
            }
            continue;
        }
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => {
                i += 1;
                out.push(bytes[i] as char);
            }
            b'\'' | b'"' | b'`' => {}
            c if c.is_ascii_whitespace() => {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            c => out.push(c as char),
        }
        i += 1;
    }
    out
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

/// True when `path` is a `$GROK_HOME` credential file that must not be mutated.
pub fn grok_home_credential_denied(path: &std::path::Path) -> bool {
    xai_grok_sandbox::write_denied_grok_home_credential(path)
}

/// Model-facing refusal for grok-home credential writes.
pub fn grok_home_credential_denial(tool: &str, path: &std::path::Path) -> String {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("credential");
    denial(
        tool,
        "grok_home_credentials",
        &format!("`{}` (`{name}` under $GROK_HOME)", path.display()),
    ) + " — use `/providers` or the platform login flow, not file edits"
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
        assert!(
            p.path_denied(std::path::Path::new("src/production/main.rs"))
                .is_some()
        );
        assert!(p.path_denied(std::path::Path::new(".env")).is_some());
        assert!(
            p.path_denied(std::path::Path::new("config/.env.local"))
                .is_some()
        );
        assert!(
            p.path_denied(std::path::Path::new("src/lib/producer.rs"))
                .is_none()
        );
        assert!(p.path_denied(std::path::Path::new("src/main.rs")).is_none());
        assert!(
            p.path_denied(std::path::Path::new("src\\production\\main.rs"))
                .is_some()
        );
    }

    #[test]
    fn deny_commands_match_case_insensitively() {
        let p = policy(&[], &["terraform destroy", "drop table"], None);
        assert!(
            p.command_denied("Terraform Destroy -auto-approve")
                .is_some()
        );
        assert!(p.command_denied("psql -c 'DROP TABLE users'").is_some());
        assert!(p.command_denied("cargo build").is_none());
    }

    #[test]
    fn deny_commands_see_through_quotes_escapes_and_whitespace() {
        let p = policy(&[], &["curl", "rm -rf"], None);
        assert!(p.command_denied("cu''rl -s https://evil.example").is_some());
        assert!(p.command_denied("cur\\l -s https://evil.example").is_some());
        assert!(p.command_denied("rm  -rf /data").is_some());
        assert!(p.command_denied("$'\\143url' -s https://evil.example").is_some());
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
