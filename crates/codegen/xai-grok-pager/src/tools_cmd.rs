//! `turbo tools` — headless schema assert for registered model-facing tools.
//!
//! RC2: CI/dogfood can prove `spawn_subagent` (and peers) are present after
//! config resolve without a model turn:
//!
//! ```text
//! turbo tools list
//! turbo tools list --json
//! turbo tools list --require spawn_subagent --require developer_log
//! ```
//!
//! Mirrors key [`xai_grok_agent::builder::AgentBuilder`] registration gates so
//! the list is closer to what a live primary session actually exposes:
//! - `GROK_SUBAGENTS=0` / `[subagents] enabled = false` strips `spawn_subagent`
//! - empty subagent discovery (same as builder) also strips `spawn_subagent`
//! - when spawn is stripped and no background-capable bash remains, lifecycle
//!   tools (`get_command_or_subagent_output`, `wait_commands_or_subagents`,
//!   `kill_command_or_subagent`) are pruned (builder parity)

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::Subcommand;
use xai_grok_agent::config::workspace_grok_build_toolset;
use xai_grok_tools::registry::types::ToolConfig;

#[derive(Debug, clap::Args, Clone)]
pub struct ToolsArgs {
    #[command(subcommand)]
    pub command: ToolsCommand,
}

#[derive(Debug, Subcommand, Clone)]
pub enum ToolsCommand {
    /// Print registered client-facing tool names for the default grok-build toolset
    List {
        /// Emit JSON (`schemaVersion`, `subagents_enabled`, `tools[]`)
        #[arg(long)]
        json: bool,
        /// Exit 1 if this client-facing name is missing (repeatable)
        #[arg(long = "require", value_name = "NAME")]
        require: Vec<String>,
        /// Toolset preset (default: `grok-build` workspace set)
        #[arg(long, default_value = "grok-build")]
        preset: String,
        /// Working directory for subagent discovery (default: process cwd)
        #[arg(long, value_name = "PATH")]
        cwd: Option<PathBuf>,
    },
}

pub fn run(args: ToolsArgs) -> Result<()> {
    match args.command {
        ToolsCommand::List {
            json,
            require,
            preset,
            cwd,
        } => list_tools(json, &require, &preset, cwd.as_deref()),
    }
}

fn subagents_enabled_from_env_and_config() -> bool {
    // Same vocabulary as shell SubagentsConfig / xai_grok_config::env_bool (C16).
    if let Some(v) = xai_grok_config::env_bool("GROK_SUBAGENTS") {
        return v;
    }
    // Disk config: [subagents] enabled = false
    if let Ok(cfg) = xai_grok_shell::config::load_effective_config_disk_only() {
        if let Some(table) = cfg.get("subagents").and_then(|v| v.as_table()) {
            if let Some(enabled) = table.get("enabled").and_then(|v| v.as_bool()) {
                return enabled;
            }
        }
    }
    true
}

fn subagent_toggle_from_config() -> HashMap<String, bool> {
    let mut toggle = HashMap::new();
    if let Ok(cfg) = xai_grok_shell::config::load_effective_config_disk_only() {
        if let Some(table) = cfg
            .get("subagents")
            .and_then(|v| v.get("toggle"))
            .and_then(|v| v.as_table())
        {
            for (k, v) in table {
                if let Some(b) = v.as_bool() {
                    toggle.insert(k.clone(), b);
                }
            }
        }
    }
    toggle
}

fn client_name(tc: &ToolConfig) -> String {
    if let Some(n) = &tc.name_override {
        return n.clone();
    }
    tc.id
        .rsplit_once(':')
        .map(|(_, id)| id.to_string())
        .unwrap_or_else(|| tc.id.clone())
}

/// AgentBuilder-aligned gates applied to the static preset tool list.
#[derive(Debug, Clone, Default)]
struct AgentBuilderGates {
    subagents_enabled: bool,
    discovery_count: usize,
    spawn_stripped: bool,
    lifecycle_pruned: bool,
    reasons: Vec<String>,
}

fn tool_names_for_preset(
    preset: &str,
    subagents_enabled: bool,
    cwd: &Path,
) -> Result<(Vec<String>, AgentBuilderGates)> {
    let config = if preset.trim().eq_ignore_ascii_case("grok-build") || preset.trim().is_empty() {
        workspace_grok_build_toolset()
    } else {
        xai_grok_agent::config::toolset_for_preset(preset).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown toolset preset '{preset}' (try: {})",
                xai_grok_agent::config::preset_names().join(", ")
            )
        })?
    };

    let mut names: Vec<String> = config.tools.iter().map(client_name).collect();
    names.sort();
    names.dedup();

    let toggle = subagent_toggle_from_config();
    let discovered = xai_grok_agent::discovery::all_subagents(cwd, &toggle);
    let discovery_count = discovered.len();

    let mut gates = AgentBuilderGates {
        subagents_enabled,
        discovery_count,
        ..Default::default()
    };

    // AgentBuilder strips TaskTool when subagents disabled OR discovery empty.
    let mut spawn_stripped = false;
    if !subagents_enabled {
        names.retain(|n| n != "spawn_subagent");
        spawn_stripped = true;
        gates.reasons.push("subagents_disabled".into());
    } else if discovery_count == 0 {
        names.retain(|n| n != "spawn_subagent");
        spawn_stripped = true;
        gates.reasons.push("empty_discovery".into());
    }
    gates.spawn_stripped = spawn_stripped;

    // Builder also prunes lifecycle tools when task is stripped and no
    // background-capable bash satisfier remains. Workspace preset always
    // includes run_terminal_command; only prune when bash was already absent.
    if spawn_stripped {
        let has_bash = names.iter().any(|n| {
            matches!(
                n.as_str(),
                "run_terminal_command" | "run_terminal_cmd" | "bash"
            )
        });
        if !has_bash {
            names.retain(|n| {
                !matches!(
                    n.as_str(),
                    "get_command_or_subagent_output"
                        | "get_task_output"
                        | "wait_commands_or_subagents"
                        | "wait_tasks"
                        | "kill_command_or_subagent"
                        | "kill_task"
                )
            });
            gates.lifecycle_pruned = true;
            gates.reasons.push("lifecycle_no_bash_satisfier".into());
        }
    }

    Ok((names, gates))
}

fn list_tools(json: bool, require: &[String], preset: &str, cwd: Option<&Path>) -> Result<()> {
    let subagents_enabled = subagents_enabled_from_env_and_config();
    let cwd_owned = match cwd {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let (names, gates) = tool_names_for_preset(preset, subagents_enabled, &cwd_owned)?;

    let mut missing: Vec<String> = Vec::new();
    for req in require {
        if !names.iter().any(|n| n == req) {
            missing.push(req.clone());
        }
    }

    if json {
        let v = serde_json::json!({
            "schemaVersion": 2,
            "preset": preset,
            "cwd": cwd_owned.display().to_string(),
            "subagents_enabled": gates.subagents_enabled,
            "discovery_count": gates.discovery_count,
            "agent_builder_gates": {
                "spawn_stripped": gates.spawn_stripped,
                "lifecycle_pruned": gates.lifecycle_pruned,
                "reasons": gates.reasons,
            },
            "tools": names,
            "require": require,
            "missing": missing,
            "ok": missing.is_empty(),
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!(
            "Turbo tools list (preset={preset}, subagents_enabled={}, discovery={})",
            gates.subagents_enabled, gates.discovery_count
        );
        for n in &names {
            println!("  {n}");
        }
        println!("  ({} tools)", names.len());
        if !gates.reasons.is_empty() {
            println!("  agent_builder_gates: {}", gates.reasons.join(", "));
        }
        if !require.is_empty() {
            if missing.is_empty() {
                println!("require: OK ({})", require.join(", "));
            } else {
                println!("require: MISSING ({})", missing.join(", "));
            }
        }
    }

    if !missing.is_empty() {
        bail!("required tool(s) not registered: {}", missing.join(", "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_preset_includes_spawn_subagent() {
        let cwd = std::env::current_dir().expect("cwd");
        let (names, gates) = tool_names_for_preset("grok-build", true, &cwd).expect("toolset");
        // Live monorepo discovery is non-empty (built-ins); spawn stays.
        if gates.discovery_count > 0 {
            assert!(
                names.iter().any(|n| n == "spawn_subagent"),
                "spawn_subagent missing from {:?} (discovery={})",
                names,
                gates.discovery_count
            );
        }
        assert!(names.iter().any(|n| n == "developer_log"));
        assert!(
            names.iter().any(|n| n == "feature_request_log"),
            "feature_request_log missing from {:?}",
            names
        );
        assert!(names.iter().any(|n| n == "run_terminal_command"));
    }

    #[test]
    fn subagents_disabled_strips_spawn() {
        let cwd = std::env::current_dir().expect("cwd");
        let (names, gates) = tool_names_for_preset("grok-build", false, &cwd).expect("toolset");
        assert!(!names.iter().any(|n| n == "spawn_subagent"));
        assert!(gates.spawn_stripped);
        assert!(gates.reasons.iter().any(|r| r == "subagents_disabled"));
    }

    #[test]
    #[serial_test::serial]
    fn env_bool_accepts_enabled_disabled() {
        // Mirror shell: enabled/disabled must not fall through to disk config.
        {
            let _g = xai_grok_test_support::EnvGuard::set("GROK_SUBAGENTS", "disabled");
            assert!(!subagents_enabled_from_env_and_config());
        }
        {
            let _g = xai_grok_test_support::EnvGuard::set("GROK_SUBAGENTS", "enabled");
            assert!(subagents_enabled_from_env_and_config());
        }
    }
}
