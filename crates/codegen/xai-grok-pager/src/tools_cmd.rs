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
//! Respects `GROK_SUBAGENTS=0` / `[subagents] enabled = false` by omitting
//! `spawn_subagent` when disabled.

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
    },
}

pub fn run(args: ToolsArgs) -> Result<()> {
    match args.command {
        ToolsCommand::List {
            json,
            require,
            preset,
        } => list_tools(json, &require, &preset),
    }
}

fn subagents_enabled_from_env_and_config() -> bool {
    // Env kill-switch first (matches boot card / shell).
    if let Ok(v) = std::env::var("GROK_SUBAGENTS") {
        let s = v.trim().to_ascii_lowercase();
        if matches!(s.as_str(), "0" | "false" | "off" | "no") {
            return false;
        }
        if matches!(s.as_str(), "1" | "true" | "on" | "yes") {
            return true;
        }
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

fn client_name(tc: &ToolConfig) -> String {
    if let Some(n) = &tc.name_override {
        return n.clone();
    }
    tc.id
        .rsplit_once(':')
        .map(|(_, id)| id.to_string())
        .unwrap_or_else(|| tc.id.clone())
}

fn tool_names_for_preset(preset: &str, subagents_enabled: bool) -> Result<Vec<String>> {
    let config = if preset.trim().eq_ignore_ascii_case("grok-build")
        || preset.trim().is_empty()
    {
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

    if !subagents_enabled {
        names.retain(|n| n != "spawn_subagent");
    }
    Ok(names)
}

fn list_tools(json: bool, require: &[String], preset: &str) -> Result<()> {
    let subagents_enabled = subagents_enabled_from_env_and_config();
    let names = tool_names_for_preset(preset, subagents_enabled)?;

    let mut missing: Vec<String> = Vec::new();
    for req in require {
        if !names.iter().any(|n| n == req) {
            missing.push(req.clone());
        }
    }

    if json {
        let v = serde_json::json!({
            "schemaVersion": 1,
            "preset": preset,
            "subagents_enabled": subagents_enabled,
            "tools": names,
            "require": require,
            "missing": missing,
            "ok": missing.is_empty(),
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!("Turbo tools list (preset={preset}, subagents_enabled={subagents_enabled})");
        for n in &names {
            println!("  {n}");
        }
        println!("  ({} tools)", names.len());
        if !require.is_empty() {
            if missing.is_empty() {
                println!("require: OK ({})", require.join(", "));
            } else {
                println!("require: MISSING ({})", missing.join(", "));
            }
        }
    }

    if !missing.is_empty() {
        bail!(
            "required tool(s) not registered: {}",
            missing.join(", ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_preset_includes_spawn_subagent() {
        let names = tool_names_for_preset("grok-build", true).expect("toolset");
        assert!(
            names.iter().any(|n| n == "spawn_subagent"),
            "spawn_subagent missing from {:?}",
            names
        );
        assert!(names.iter().any(|n| n == "developer_log"));
        assert!(names.iter().any(|n| n == "run_terminal_command"));
    }

    #[test]
    fn subagents_disabled_strips_spawn() {
        let names = tool_names_for_preset("grok-build", false).expect("toolset");
        assert!(!names.iter().any(|n| n == "spawn_subagent"));
    }
}
