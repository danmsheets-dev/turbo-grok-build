//! `/tree` — show workspace tree atlas status / inject preview / refresh.

use crate::slash::command::{AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand};

const USAGE: &str =
    "Usage: /tree [status|doctor|inject-preview|refresh|resolve <name>]  (default: status)";

pub struct TreeCommand;

impl SlashCommand for TreeCommand {
    fn name(&self) -> &str {
        "tree"
    }

    fn aliases(&self) -> &[&str] {
        &["workspace-tree", "atlas"]
    }

    fn description(&self) -> &str {
        "Workspace directory atlas (status, inject preview, resolve)"
    }

    fn usage(&self) -> &str {
        "/tree [status|doctor|inject-preview|refresh|resolve <name>]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("[status|doctor|inject-preview|refresh|resolve <name>]")
    }

    fn suggest_args(&self, _ctx: &AppCtx, args_query: &str) -> Option<Vec<ArgItem>> {
        let q = args_query.trim().to_ascii_lowercase();
        let options = [
            ("status", "Freshness, stats, store path"),
            ("doctor", "Config + store diagnostics"),
            ("inject-preview", "Card the agent would see"),
            ("refresh", "Rebuild durable index"),
            ("resolve", "Resolve a basename / guessed path"),
            ("search", "Search basenames / path substrings"),
        ];
        let items: Vec<ArgItem> = options
            .into_iter()
            .filter(|(name, _)| q.is_empty() || name.starts_with(&q) || name.contains(&q))
            .map(|(name, desc)| ArgItem {
                display: name.into(),
                match_text: name.into(),
                insert_text: name.into(),
                description: desc.into(),
                locked: false,
                action_id: None,
                hidden: false,
            })
            .collect();
        (!items.is_empty()).then_some(items)
    }

    fn session_scoped(&self) -> bool {
        false
    }

    fn run(&self, ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        // RC13 P1 F11: prefer session workspace CWD over process cwd.
        let root = ctx
            .session_cwd
            .map(|p| p.to_path_buf())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        if root.as_os_str().is_empty() {
            return CommandResult::Error("cwd: session and process CWD unavailable".into());
        }
        let trimmed = args.trim();
        let (cmd, rest) = match trimmed.split_once(char::is_whitespace) {
            Some((c, r)) => (c.to_ascii_lowercase(), r.trim()),
            None => {
                if trimmed.is_empty() {
                    ("status".to_string(), "")
                } else {
                    (trimmed.to_ascii_lowercase(), "")
                }
            }
        };

        let tree_args = match cmd.as_str() {
            "status" | "" => crate::tree_cmd::TreeArgs {
                command: crate::tree_cmd::TreeCommand::Status {
                    root: Some(root),
                    json: false,
                },
            },
            "doctor" => crate::tree_cmd::TreeArgs {
                command: crate::tree_cmd::TreeCommand::Doctor { root: Some(root) },
            },
            "inject-preview" | "inject" | "preview" => crate::tree_cmd::TreeArgs {
                command: crate::tree_cmd::TreeCommand::InjectPreview {
                    root: Some(root),
                    mode: None,
                    subagent: false,
                },
            },
            "refresh" | "build" => crate::tree_cmd::TreeArgs {
                command: crate::tree_cmd::TreeCommand::Build { root: Some(root) },
            },
            "resolve" => {
                if rest.is_empty() {
                    return CommandResult::Error("Usage: /tree resolve <name>".into());
                }
                crate::tree_cmd::TreeArgs {
                    command: crate::tree_cmd::TreeCommand::Resolve {
                        name: rest.to_string(),
                        root: Some(root),
                        hint: None,
                        limit: 8,
                    },
                }
            }
            "search" => {
                if rest.is_empty() {
                    return CommandResult::Error("Usage: /tree search <query>".into());
                }
                crate::tree_cmd::TreeArgs {
                    command: crate::tree_cmd::TreeCommand::Search {
                        query: rest.to_string(),
                        root: Some(root),
                        limit: 20,
                    },
                }
            }
            other => {
                return CommandResult::Error(format!("unknown `/tree` subcommand `{other}`\n{USAGE}"));
            }
        };

        // Capture stdout of tree_cmd::run by running into a string via temporary
        // approach: call the same helpers by re-invoking run and reading... run
        // prints to stdout. For TUI we wrap via a buffer isn't available, so
        // re-implement thin Message strings via status/doctor functions.
        match capture_tree(tree_args) {
            Ok(msg) => CommandResult::Message(msg),
            Err(e) => CommandResult::Error(e.to_string()),
        }
    }
}

fn capture_tree(args: crate::tree_cmd::TreeArgs) -> anyhow::Result<String> {
    // tree_cmd::run writes to stdout; for slash UX we call a stringy path.
    use xai_workspace_tree::{
        inject_card, load_index_for_root, prune_store, resolve_path, search, summary,
        workspace_id_for_path, workspace_store_dir, WorkspaceTreeConfig,
    };

    let config = WorkspaceTreeConfig::from_env();
    match args.command {
        crate::tree_cmd::TreeCommand::Status { root, .. } => {
            let root = root.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            if !config.enabled {
                return Ok("workspace tree: disabled (GROK_WORKSPACE_TREE=0)".into());
            }
            let id = workspace_id_for_path(&root)?;
            let store = workspace_store_dir(&xai_workspace_tree::store_root(&config), &id);
            match load_index_for_root(&root, &config) {
                Ok(index) => {
                    let m = &index.meta;
                    let s = summary(&index, 16);
                    let mut lines = vec![
                        format!("Workspace tree · {:?}", m.freshness.state),
                        format!("Root: {}", m.canonical_root),
                        format!("Store: {}", store.display()),
                        format!(
                            "Stats: {} files · {} dirs · built {}ms",
                            m.stats.files, m.stats.dirs, m.build.duration_ms
                        ),
                    ];
                    if let Some(ref basis) = m.freshness.basis {
                        lines.push(format!("Basis: {basis}"));
                    }
                    lines.push("Top-level:".into());
                    for e in &s.top_level {
                        lines.push(format!("  {}/", e.name));
                    }
                    lines.push("Tools: workspace_tree, resolve_path · CLI: turbo tree status".into());
                    Ok(lines.join("\n"))
                }
                Err(e) => Ok(format!(
                    "No index yet for {} ({e}).\nRun: /tree refresh   or   turbo tree build",
                    root.display()
                )),
            }
        }
        crate::tree_cmd::TreeCommand::Doctor { root } => {
            let root = root.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            let mut out = Vec::new();
            out.push(format!("enabled={}", config.enabled));
            out.push(format!("inject={}", config.inject.mode.as_str()));
            out.push(format!(
                "store={}",
                xai_workspace_tree::store_root(&config).display()
            ));
            match load_index_for_root(&root, &config) {
                Ok(i) => {
                    out.push(format!("freshness={:?}", i.meta.freshness.state));
                    if let Some(b) = &i.meta.freshness.basis {
                        out.push(format!("basis={b}"));
                    }
                }
                Err(e) => out.push(format!("index={e}")),
            }
            Ok(out.join("\n"))
        }
        crate::tree_cmd::TreeCommand::InjectPreview {
            root, mode, subagent, ..
        } => {
            let root = root.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            let mut cfg = config.clone();
            if let Some(m) = mode {
                if let Some(parsed) = xai_workspace_tree::InjectMode::parse(&m) {
                    cfg.inject.mode = parsed;
                }
            } else {
                cfg.inject.mode = config.inject_mode_for_audience(subagent);
            }
            let index = xai_grok_tools::util::workspace_tree_get_or_load(&root, &cfg)
                .map_err(anyhow::Error::msg)?;
            Ok(inject_card(&index, &cfg))
        }
        crate::tree_cmd::TreeCommand::Build { root } => {
            let root = root.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            // RC13 P0 F3: replace process cache; do not get_or_load after build.
            let index = xai_grok_tools::util::workspace_tree_refresh(&root, &config)
                .map_err(anyhow::Error::msg)?;
            Ok(format!(
                "Rebuilt: {} files, {} dirs, {}ms",
                index.meta.stats.files, index.meta.stats.dirs, index.meta.build.duration_ms
            ))
        }
        crate::tree_cmd::TreeCommand::Resolve {
            name,
            root,
            hint,
            limit,
        } => {
            let root = root.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            let index = xai_grok_tools::util::workspace_tree_get_or_load(&root, &config)
                .map_err(anyhow::Error::msg)?;
            let result = resolve_path(&index, &name, hint.as_deref(), limit);
            if result.hits.is_empty() {
                Ok(format!("No matches for `{name}`"))
            } else {
                let mut lines = vec![format!("resolve_path `{name}`:")];
                for (i, h) in result.hits.iter().enumerate() {
                    lines.push(format!(
                        "  {}. {}  ({:.2}, {})",
                        i + 1,
                        h.rel_path,
                        h.score,
                        h.reason
                    ));
                }
                Ok(lines.join("\n"))
            }
        }
        crate::tree_cmd::TreeCommand::Search {
            query,
            root,
            limit,
        } => {
            let root = root.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            let index = xai_grok_tools::util::workspace_tree_get_or_load(&root, &config)
                .map_err(anyhow::Error::msg)?;
            let result = search(&index, &query, limit.clamp(1, 100));
            if result.hits.is_empty() {
                Ok(format!("No search hits for `{query}`"))
            } else {
                let mut lines = vec![format!("search `{query}`:")];
                for (i, h) in result.hits.iter().enumerate() {
                    lines.push(format!(
                        "  {}. {}  (score {:.2})",
                        i + 1,
                        h.rel_path,
                        h.score
                    ));
                }
                Ok(lines.join("\n"))
            }
        }
        crate::tree_cmd::TreeCommand::Prune {
            max_age_days,
            keep_newest,
            dry_run,
        } => {
            if dry_run {
                return Ok(format!(
                    "prune dry-run: would drop indexes older than {max_age_days} day(s) (keep_newest={keep_newest})"
                ));
            }
            let max_age =
                std::time::Duration::from_secs(max_age_days.saturating_mul(24 * 3600));
            let report =
                prune_store(&config, max_age, keep_newest).map_err(anyhow::Error::msg)?;
            Ok(format!(
                "prune: removed {} dirs · freed {} bytes · remaining {} dirs · {} bytes",
                report.removed_dirs,
                report.freed_bytes,
                report.remaining_dirs,
                report.remaining_bytes
            ))
        }
    }
}
