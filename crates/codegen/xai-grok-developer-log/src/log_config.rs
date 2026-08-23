//! Shared TOML sidecar for Auto Developer Log / Feature Request Log.
//!
//! Keys (all optional; missing `github_*` means local-only):
//!
//! ```toml
//! dir = "..."
//! github_repo = "owner/name"
//! github_sync = "off" | "manual" | "on-file"   # default off
//! ```

use std::path::{Path, PathBuf};

/// How (and whether) local log JSON is mirrored to GitHub Issues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GithubSyncMode {
    /// Never talk to GitHub (default — no cloud upload).
    #[default]
    Off,
    /// Operator runs `turbo issues sync` / `turbo features sync`.
    Manual,
    /// Best-effort background push after a local write. Never blocks the
    /// `developer_log` / `feature_request_log` tool.
    OnFile,
}

impl GithubSyncMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Manual => "manual",
            Self::OnFile => "on-file",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "false" | "0" | "no" | "disabled" => Some(Self::Off),
            "manual" | "cli" | "on" => Some(Self::Manual),
            "on-file" | "on_file" | "onfile" | "auto" => Some(Self::OnFile),
            _ => None,
        }
    }

    pub fn is_off(self) -> bool {
        matches!(self, Self::Off)
    }
}

impl std::fmt::Display for GithubSyncMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which sidecar file a [`LogFileConfig`] is rendered for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogConfigKind {
    Incident,
    Feature,
}

/// Parsed `$GROK_HOME/developer-log.toml` or `feature-request-log.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LogFileConfig {
    pub dir: Option<PathBuf>,
    pub github_repo: Option<String>,
    pub github_sync: GithubSyncMode,
}

impl LogFileConfig {
    /// True when GitHub sync is configured to run at all (CLI or on-file).
    pub fn github_enabled(&self) -> bool {
        self.github_repo.is_some() && !self.github_sync.is_off()
    }
}

/// Parse a minimal TOML sidecar. Unknown keys are ignored. Default
/// `github_sync` is [`GithubSyncMode::Off`].
pub fn parse_log_toml(raw: &str) -> LogFileConfig {
    let mut cfg = LogFileConfig::default();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, rest)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = unquote(rest.trim());
        match key {
            "dir" if !value.is_empty() => {
                cfg.dir = Some(PathBuf::from(value.replace("\\\\", "\\")));
            }
            "github_repo" if !value.is_empty() => {
                cfg.github_repo = Some(value);
            }
            "github_sync" => {
                if let Some(mode) = GithubSyncMode::parse(&value) {
                    cfg.github_sync = mode;
                }
            }
            _ => {}
        }
    }
    cfg
}

fn unquote(rest: &str) -> String {
    let unquoted = rest
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| rest.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(rest);
    unquoted.replace("\\\\", "\\")
}

/// Render the sidecar. Always writes `github_sync` so operators see the knob.
pub fn render_log_toml(kind: LogConfigKind, cfg: &LogFileConfig) -> String {
    let (title, env, managed) = match kind {
        LogConfigKind::Incident => (
            "Turbo Auto Developer Log — root directory for product incidents",
            "GROK_DEVELOPER_LOG_DIR",
            "turbo issues set-dir",
        ),
        LogConfigKind::Feature => (
            "Turbo Feature Request Log — root for agent product-capability requests",
            "GROK_FEATURE_REQUEST_LOG_DIR",
            "turbo features set-dir",
        ),
    };
    let mut s = String::new();
    s.push_str("# ");
    s.push_str(title);
    s.push('\n');
    s.push_str("# Override with env ");
    s.push_str(env);
    s.push_str("=...\n");
    s.push_str("# Managed by `");
    s.push_str(managed);
    s.push_str("`\n");
    if let Some(dir) = &cfg.dir {
        let escaped = dir.display().to_string().replace('\\', "\\\\");
        s.push_str("dir = \"");
        s.push_str(&escaped);
        s.push_str("\"\n");
    }
    s.push_str(
        "\n# Opt-in GitHub Issues sync (default off — no cloud upload).\n\
         # github_sync = \"off\" | \"manual\" | \"on-file\"\n",
    );
    match cfg
        .github_repo
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
    {
        Some(repo) => {
            s.push_str("github_repo = \"");
            s.push_str(repo);
            s.push_str("\"\n");
        }
        None => s.push_str("# github_repo = \"owner/name\"\n"),
    }
    s.push_str("github_sync = \"");
    s.push_str(cfg.github_sync.as_str());
    s.push_str("\"\n");
    s
}

/// Atomic replace of a sidecar file (Windows-safe: remove dest first).
pub fn write_log_toml_file(path: &Path, body: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, body)?;
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Load a sidecar from disk; missing file → default (local-only).
pub fn load_log_toml_file(path: &Path) -> LogFileConfig {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return LogFileConfig::default();
    };
    parse_log_toml(&raw)
}

/// Should a local write kick a best-effort background GitHub push?
///
/// Never true when `github_repo` is unset or mode is not `on-file`.
pub fn should_spawn_on_file(cfg: &LogFileConfig) -> bool {
    matches!(cfg.github_sync, GithubSyncMode::OnFile)
        && cfg
            .github_repo
            .as_deref()
            .map(str::trim)
            .is_some_and(|r| !r.is_empty())
}

/// `owner/name` — no leading `-` (gh argv safety).
pub fn validate_github_repo(repo: &str) -> Result<&str, String> {
    let repo = repo.trim();
    if repo.is_empty() {
        return Err("empty".into());
    }
    let Some((owner, name)) = repo.split_once('/') else {
        return Err("expected owner/name".into());
    };
    if owner.is_empty()
        || name.is_empty()
        || owner.starts_with('-')
        || name.starts_with('-')
        || repo.contains("..")
        || owner.contains('/')
        || name.contains('/')
        || !owner
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err("expected owner/name".into());
    }
    Ok(repo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_toml_is_local_only() {
        let cfg = parse_log_toml("");
        assert!(cfg.dir.is_none());
        assert!(cfg.github_repo.is_none());
        assert_eq!(cfg.github_sync, GithubSyncMode::Off);
        assert!(!cfg.github_enabled());
        assert!(!should_spawn_on_file(&cfg));
    }

    #[test]
    fn parses_github_keys() {
        let cfg = parse_log_toml(
            r#"
# comment
dir = "D:\\HyperLogs\\developer-log"
github_repo = "danmsheets-dev/turbo-field-logs"
github_sync = "manual"
"#,
        );
        assert_eq!(
            cfg.dir.as_deref(),
            Some(Path::new(r"D:\HyperLogs\developer-log"))
        );
        assert_eq!(
            cfg.github_repo.as_deref(),
            Some("danmsheets-dev/turbo-field-logs")
        );
        assert_eq!(cfg.github_sync, GithubSyncMode::Manual);
        assert!(cfg.github_enabled());
        assert!(!should_spawn_on_file(&cfg));
    }

    #[test]
    fn on_file_requires_repo() {
        let mut cfg = parse_log_toml(r#"github_sync = "on-file""#);
        assert!(!should_spawn_on_file(&cfg));
        cfg.github_repo = Some("o/n".into());
        assert!(should_spawn_on_file(&cfg));
    }

    #[test]
    fn render_roundtrip_preserves_github_keys() {
        let cfg = LogFileConfig {
            dir: Some(PathBuf::from("/tmp/adl")),
            github_repo: Some("o/n".into()),
            github_sync: GithubSyncMode::OnFile,
        };
        let text = render_log_toml(LogConfigKind::Incident, &cfg);
        let again = parse_log_toml(&text);
        assert_eq!(again.github_repo.as_deref(), Some("o/n"));
        assert_eq!(again.github_sync, GithubSyncMode::OnFile);
        assert!(text.contains("github_sync = \"on-file\""));
    }

    #[test]
    fn validate_repo_rejects_flags() {
        assert!(validate_github_repo("-evil/name").is_err());
        assert!(validate_github_repo("owner").is_err());
        assert!(validate_github_repo("danmsheets-dev/turbo-field-logs").is_ok());
    }
}
