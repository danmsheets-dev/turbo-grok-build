//! Parse .envrc files and extract environment variables.
//!
//! This module provides a way to load environment variables from `.envrc` files.
//! It uses a two-tier approach:
//!
//! 1. **Try `direnv export json`** - If direnv is installed, use it for full compatibility
//! 2. **Fallback to bash** - Run .envrc in a bash subshell with direnv stubs
//!
//! This approach handles:
//! - Variable expansion ($HOME, ${VAR:-default})
//! - Command substitution ($(git rev-parse ...))
//! - Conditional logic (if/then/else)
//! - direnv helper functions (source_up_if_exists, PATH_add, etc.)

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// Stub implementations of common direnv helper functions.
/// These are prepended to the .envrc before execution when direnv is not available.
const DIRENV_STUBS: &str = r#"
# Stub direnv helper functions
source_up_if_exists() { :; }
source_up() { :; }
source_env_if_exists() {
    if [ -f "$1" ]; then
        . "$1"
    fi
}
source_env() {
    if [ -f "$1" ]; then
        . "$1"
    fi
}
PATH_add() {
    export PATH="$PWD/$1:$PATH"
}
path_add() {
    PATH_add "$@"
}
layout() { :; }
use() { :; }
watch_file() { :; }
"#;

/// Load environment variables from .envrc file in the given directory.
///
/// Returns a HashMap of environment variables that were set/modified by the .envrc.
/// Returns None if no .envrc exists or if parsing fails.
pub fn load_envrc(dir: &Path) -> Option<HashMap<String, String>> {
    let envrc_path = dir.join(".envrc");
    if !envrc_path.exists() {
        tracing::debug!(?dir, ".envrc not found");
        return None;
    }

    // Try direnv first (most reliable if installed)
    if let Some(env) = try_direnv_export(dir) {
        return Some(env);
    }

    // Fall back to bash subshell approach
    load_envrc_via_bash(dir)
}

/// Try to use `direnv export json` to load environment variables.
/// Returns None if direnv is not installed or fails.
fn try_direnv_export(dir: &Path) -> Option<HashMap<String, String>> {
    let mut cmd = Command::new("direnv");
    cmd.args(["export", "json"])
        .current_dir(dir)
        .stdin(std::process::Stdio::null());
    xai_grok_tools::util::detach_std_command(&mut cmd);
    let output = cmd.output().ok()?;

    if !output.status.success() {
        // direnv not allowed, or other error
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("not allowed") {
            tracing::debug!(?dir, %stderr, "direnv export failed");
        }
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        // No changes from direnv
        return None;
    }

    // Parse JSON output: {"VAR": "value", ...}
    // Note: direnv also outputs null for vars to unset, but we ignore those
    match serde_json::from_str::<HashMap<String, serde_json::Value>>(&stdout) {
        Ok(json) => {
            let env: HashMap<String, String> = json
                .into_iter()
                .filter_map(|(k, v)| {
                    if let serde_json::Value::String(s) = v {
                        Some((k, s))
                    } else {
                        None // Skip null values (unset)
                    }
                })
                .collect();

            if env.is_empty() {
                None
            } else {
                tracing::info!(?dir, count = env.len(), "Loaded environment via direnv");
                Some(env)
            }
        }
        Err(e) => {
            tracing::warn!(?dir, ?e, "Failed to parse direnv JSON output");
            None
        }
    }
}

/// Quote a host path for embedding in a double-quoted bash string.
///
/// On Windows, normalizes `\` → `/` so Git Bash / MSYS does not treat path
/// separators as escape sequences (`\t`, `\U`, …). On Unix, backslashes are
/// preserved as literal filename characters and escaped for bash safety.
/// Also escapes `$`, `` ` ``, and `"`.
fn bash_quote_path(path: &Path) -> String {
    let mut out = String::with_capacity(path.as_os_str().len() + 8);
    for ch in path.to_string_lossy().chars() {
        match ch {
            // Windows path separators only — never rewrite Unix `\` in names.
            #[cfg(windows)]
            '\\' => out.push('/'),
            #[cfg(not(windows))]
            '\\' => {
                out.push('\\');
                out.push('\\');
            }
            '"' | '$' | '`' => {
                out.push('\\');
                out.push(ch);
            }
            c => out.push(c),
        }
    }
    out
}

/// Load environment by running .envrc in a bash subshell.
/// This is the fallback when direnv is not installed.
fn load_envrc_via_bash(dir: &Path) -> Option<HashMap<String, String>> {
    let envrc_path = dir.join(".envrc");
    let bash = xai_grok_config::shell::resolve_bash_executable().or_else(|| {
        tracing::warn!(?envrc_path, "No bash executable found for .envrc fallback");
        None
    })?;

    // Build a script that:
    // 1. cd into the project
    // 2. Prefer Windows-form $PWD on Git Bash (`pwd -W`) so PATH_add / $PWD
    //    exports are usable by native Windows consumers (MSYS otherwise maps
    //    TEMP dirs to `/tmp/...` and drive letters to `/c/...`).
    // 3. Includes direnv stubs
    // 4. Sources the .envrc
    // 5. Outputs all env vars as KEY=VALUE pairs (null-separated for safety)
    let script = format!(
        r#"
set -e
cd "{dir}"
# Git Bash: surface a Windows path for $PWD when available.
if WIN_PWD=$(pwd -W 2>/dev/null); then
  WIN_PWD=$(printf '%s' "$WIN_PWD" | tr '\\' '/')
  export PWD="$WIN_PWD"
fi
{stubs}
. "{envrc}"
# Output all environment variables, null-separated
env -0
"#,
        dir = bash_quote_path(dir),
        stubs = DIRENV_STUBS,
        envrc = bash_quote_path(&envrc_path),
    );

    // Capture baseline environment (before running .envrc)
    let baseline: HashMap<String, String> = std::env::vars().collect();

    // Run the script and capture output (Git Bash on Windows; /bin/bash on Unix).
    let mut bash_cmd = Command::new(&bash);
    bash_cmd
        .arg("-c")
        .arg(&script)
        .current_dir(dir)
        .stdin(std::process::Stdio::null());
    xai_grok_tools::util::detach_std_command(&mut bash_cmd);
    let output = bash_cmd.output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            tracing::warn!(
                ?envrc_path,
                bash = %bash,
                %stderr,
                "Failed to execute .envrc via bash"
            );
            return None;
        }
        Err(e) => {
            tracing::warn!(?envrc_path, bash = %bash, ?e, "Failed to run bash for .envrc");
            return None;
        }
    };

    // Parse the null-separated KEY=VALUE pairs
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut result: HashMap<String, String> = HashMap::new();

    for entry in stdout.split('\0') {
        if entry.is_empty() {
            continue;
        }
        if let Some((key, value)) = entry.split_once('=') {
            // Skip internal/noise variables
            let ignored_keys = ["_", "SHLVL", "PWD", "OLDPWD"];
            if ignored_keys.contains(&key) {
                continue;
            }
            // Only include vars that are new or changed from baseline
            match baseline.get(key) {
                Some(baseline_value) if baseline_value == value => {
                    // Unchanged, skip
                }
                _ => {
                    // New or changed
                    result.insert(key.to_string(), value.to_string());
                }
            }
        }
    }

    if result.is_empty() {
        tracing::debug!(?envrc_path, "No environment changes from .envrc");
        None
    } else {
        tracing::info!(
            ?envrc_path,
            count = result.len(),
            "Loaded environment from .envrc via bash"
        );
        Some(result)
    }
}

/// Load .envrc and return the environment, or empty HashMap on failure.
pub fn load_envrc_or_empty(dir: &Path) -> HashMap<String, String> {
    load_envrc(dir).unwrap_or_default()
}

/// [`load_envrc_or_empty`] gated on folder-trust: loads the repo-local `.envrc`
/// (executed in a bash subshell) only when `trusted`, else returns an empty map.
/// The shell call sites pass the `project_scope_allowed` verdict so the "run a
/// cloned repo's `.envrc` only when the folder is trusted" rule lives in ONE
/// place (mirrors `permission::claude_settings::load_claude_env_with_project`).
pub fn load_envrc_or_empty_when_trusted(dir: &Path, trusted: bool) -> HashMap<String, String> {
    if trusted {
        load_envrc_or_empty(dir)
    } else {
        HashMap::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Normalize path separators for host vs Git Bash / MSYS `$PWD` forms.
    fn path_norm(s: &str) -> String {
        s.replace('\\', "/").to_ascii_lowercase()
    }

    /// True when `got` contains `host_path` under Win32, MSYS, or TEMP→/tmp form.
    fn path_contains_host(got: &str, host_path: &Path) -> bool {
        let got_n = path_norm(got);
        let host_n = path_norm(&host_path.to_string_lossy());
        if got_n.contains(&host_n) {
            return true;
        }
        // Git Bash often reports `C:\foo` as `/c/foo`.
        let bytes = host_n.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
            let drive = bytes[0] as char;
            let rest = &host_n[2..];
            let msys = format!("/{drive}{rest}");
            if got_n.contains(&msys) {
                return true;
            }
        }
        // TEMP dirs are frequently mounted at `/tmp/<name>` under Git Bash.
        if let Some(name) = host_path.file_name().and_then(|n| n.to_str()) {
            let tmp_form = format!("/tmp/{}", path_norm(name));
            if got_n.contains(&tmp_form) {
                return true;
            }
        }
        // Last resort: unique temp directory leaf must appear.
        if let Some(name) = host_path.file_name().and_then(|n| n.to_str()) {
            if got_n.contains(&path_norm(name)) {
                return true;
            }
        }
        false
    }

    #[test]
    fn test_simple_export() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".envrc"), "export FOO=bar\n").unwrap();

        let Some(env) = load_envrc(dir.path()) else {
            // Soft-fail when neither direnv nor bash is available (CI without Git).
            if xai_grok_config::shell::resolve_bash_executable().is_none() {
                return;
            }
            panic!("load_envrc returned None despite bash being available");
        };
        assert_eq!(env.get("FOO"), Some(&"bar".to_string()));
    }

    #[test]
    fn test_variable_expansion() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".envrc"), "export MY_DIR=$PWD/subdir\n").unwrap();

        let Some(env) = load_envrc(dir.path()) else {
            if xai_grok_config::shell::resolve_bash_executable().is_none() {
                return;
            }
            panic!("load_envrc returned None despite bash being available");
        };
        let my_dir = env.get("MY_DIR").expect("MY_DIR set by .envrc");
        assert!(
            path_contains_host(my_dir, dir.path()) && path_norm(my_dir).ends_with("/subdir"),
            "MY_DIR={my_dir:?} should contain host path {host:?}/subdir",
            host = dir.path()
        );
    }

    #[test]
    fn test_no_envrc() {
        let dir = TempDir::new().unwrap();
        assert!(load_envrc(dir.path()).is_none());
    }

    #[test]
    fn test_path_add() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".envrc"), "PATH_add bin\n").unwrap();

        let Some(env) = load_envrc(dir.path()) else {
            if xai_grok_config::shell::resolve_bash_executable().is_none() {
                return;
            }
            panic!("load_envrc returned None despite bash being available");
        };
        let path = env.get("PATH").expect("PATH set by PATH_add");
        let bin = dir.path().join("bin");
        assert!(
            path_contains_host(path, &bin),
            "PATH={path:?} should contain {bin:?}"
        );
    }

    #[test]
    fn test_conditional() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join(".envrc"),
            r#"
if [ -d "$PWD" ]; then
    export EXISTS=yes
else
    export EXISTS=no
fi
"#,
        )
        .unwrap();

        let Some(env) = load_envrc(dir.path()) else {
            if xai_grok_config::shell::resolve_bash_executable().is_none() {
                return;
            }
            panic!("load_envrc returned None despite bash being available");
        };
        assert_eq!(env.get("EXISTS"), Some(&"yes".to_string()));
    }
}
