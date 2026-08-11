//! `turbo tree` — workspace directory atlas status / doctor / inject-preview.

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use std::path::PathBuf;
use xai_workspace_tree::{
    inject_building_notice, inject_card, inject_disabled_notice, load_index_for_root, prune_store,
    store_disk_usage, summary, workspace_id_for_path, workspace_store_dir, InjectMode,
    WorkspaceTreeConfig,
};

#[derive(Debug, clap::Args, Clone)]
pub struct TreeArgs {
    #[command(subcommand)]
    pub command: TreeCommand,
}

#[derive(Debug, Subcommand, Clone)]
pub enum TreeCommand {
    /// Show index freshness, stats, and store path for the current workspace
    Status {
        /// Workspace root (default: process cwd)
        #[arg(long)]
        root: Option<PathBuf>,
        /// Emit JSON
        #[arg(long)]
        json: bool,
    },
    /// Diagnose store layout, config, and last build basis
    Doctor {
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// Print the exact inject card the agent would see
    #[command(name = "inject-preview")]
    InjectPreview {
        #[arg(long)]
        root: Option<PathBuf>,
        /// Force inject mode: off|minimal|standard|rich
        #[arg(long)]
        mode: Option<String>,
        /// Render the subagent (minimal-preferring) card
        #[arg(long)]
        subagent: bool,
    },
    /// Rebuild the durable index for a workspace
    Build {
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// Resolve a free-form name (same as resolve_path tool)
    Resolve {
        name: String,
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long)]
        hint: Option<String>,
        #[arg(long, default_value_t = 8)]
        limit: usize,
    },
    /// Search basenames / path substrings in the atlas
    Search {
        query: String,
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Prune old durable indexes under the store root (disk hygiene)
    ///
    /// Default is dry-run (same mental model as `turbo subagent prune` /
    /// `turbo disk prune`). Pass `--execute` to apply deletions.
    Prune {
        /// Delete workspace indexes older than this many days (default 30)
        #[arg(long, default_value_t = 30)]
        max_age_days: u64,
        /// Keep at most N newest indexes after age filter (0 = unlimited)
        #[arg(long, default_value_t = 0)]
        keep_newest: usize,
        /// Print what would be removed without deleting
        #[arg(long, conflicts_with = "execute")]
        dry_run: bool,
        /// Actually delete (default is dry-run unless this is set)
        #[arg(long, conflicts_with = "dry_run")]
        execute: bool,
    },
}

pub fn run(args: TreeArgs) -> Result<()> {
    let config = WorkspaceTreeConfig::from_env();
    match args.command {
        TreeCommand::Status { root, json } => {
            let root = resolve_root(root)?;
            status(&root, &config, json)
        }
        TreeCommand::Doctor { root } => {
            let root = resolve_root(root)?;
            doctor(&root, &config)
        }
        TreeCommand::InjectPreview {
            root,
            mode,
            subagent,
        } => {
            let root = resolve_root(root)?;
            inject_preview(&root, &config, mode.as_deref(), subagent)
        }
        TreeCommand::Build { root } => {
            let root = resolve_root(root)?;
            build(&root, &config)
        }
        TreeCommand::Resolve {
            name,
            root,
            hint,
            limit,
        } => {
            let root = resolve_root(root)?;
            resolve_cmd(&root, &config, &name, hint.as_deref(), limit)
        }
        TreeCommand::Search {
            query,
            root,
            limit,
        } => {
            let root = resolve_root(root)?;
            search_cmd(&root, &config, &query, limit)
        }
        TreeCommand::Prune {
            max_age_days,
            keep_newest,
            dry_run,
            execute,
        } => {
            // Default dry-run: only delete when --execute is set (parity with
            // `turbo subagent prune` / `turbo disk prune`).
            let dry = dry_run || !execute;
            prune_cmd(&config, max_age_days, keep_newest, dry)
        }
    }
}

fn resolve_root(root: Option<PathBuf>) -> Result<PathBuf> {
    match root {
        Some(p) => Ok(p),
        None => std::env::current_dir().context("current_dir"),
    }
}

fn status(root: &std::path::Path, config: &WorkspaceTreeConfig, json: bool) -> Result<()> {
    if !config.enabled {
        println!("workspace tree: disabled (GROK_WORKSPACE_TREE=0 / TURBO_TREE=0)");
        return Ok(());
    }
    let id = workspace_id_for_path(root).context("workspace_id")?;
    let store = workspace_store_dir(&xai_workspace_tree::store_root(config), &id);

    match load_index_for_root(root, config) {
        Ok(index) => {
            if json {
                let s = summary(&index, 24);
                println!("{}", serde_json::to_string_pretty(&s)?);
            } else {
                let m = &index.meta;
                println!("Workspace tree status");
                println!("  root:        {}", m.canonical_root);
                println!("  workspace:   {}", m.workspace_id);
                println!("  store:       {}", store.display());
                println!("  freshness:   {:?}", m.freshness.state);
                if let Some(ref basis) = m.freshness.basis {
                    println!("  basis:       {basis}");
                }
                println!(
                    "  built_at:    {}  (updated_at; Phase 1 build stamp)",
                    m.updated_at
                );
                println!(
                    "  stats:       {} files · {} dirs · {} collapsed · truncated={}",
                    m.stats.files, m.stats.dirs, m.stats.collapsed_dirs, m.stats.truncated
                );
                println!(
                    "  build:       {} in {}ms ({})",
                    m.build.mode, m.build.duration_ms, m.build.walker
                );
                if m.git.present {
                    let branch = m.git.branch.as_deref().unwrap_or("?");
                    let head = m
                        .git
                        .head
                        .as_deref()
                        .map(|h| if h.len() > 8 { &h[..8] } else { h })
                        .unwrap_or("?");
                    println!("  git:         {branch}@{head}");
                }
                if !m.workspace_profile.is_empty() {
                    println!("  profile:     {}", m.workspace_profile.join(", "));
                }
                println!("  inject:      {}", config.inject.mode.as_str());
            }
        }
        Err(e) => {
            println!("Workspace tree status");
            println!("  root:        {}", root.display());
            println!("  workspace:   {id}");
            println!("  store:       {}", store.display());
            println!("  index:       missing ({e})");
            println!("  tip:         turbo tree build");
        }
    }
    Ok(())
}

fn doctor(root: &std::path::Path, config: &WorkspaceTreeConfig) -> Result<()> {
    println!("Workspace tree doctor");
    println!(
        "  enabled:     {} (GROK_WORKSPACE_TREE / TURBO_TREE)",
        config.enabled
    );
    println!("  inject:      {}", config.inject.mode.as_str());
    println!(
        "  inject env:  GROK_WORKSPACE_TREE_INJECT / TURBO_TREE_INJECT = off|minimal|standard|rich"
    );
    println!(
        "  store root:  {}",
        xai_workspace_tree::store_root(config).display()
    );
    println!("  walk max_files: {}", config.walk.max_files);
    println!("  walk max_depth: {}", config.walk.max_depth);
    println!("  use_gitignore:  {}", config.walk.use_gitignore);
    println!(
        "  collapse names: {}",
        config.collapse.names.join(", ")
    );

    if !config.enabled {
        println!("  note: master switch is off — tools return disabled.");
        return Ok(());
    }

    let id = workspace_id_for_path(root).context("workspace_id")?;
    let store = workspace_store_dir(&xai_workspace_tree::store_root(config), &id);
    println!("  workspace:   {id}");
    println!("  store dir:   {}", store.display());
    println!("  meta.json:   {}", store.join("meta.json").exists());
    println!("  tree.v1.json:{}", store.join("tree.v1.json").exists());
    if store.is_dir() {
        let sz = xai_workspace_tree::dir_size_bytes(&store);
        println!("  this index:  {} bytes", sz);
    }
    let (n_dirs, n_bytes) = store_disk_usage(config);
    println!(
        "  store total: {n_dirs} workspace dirs · {n_bytes} bytes (use `turbo tree prune` to reclaim)"
    );

    match load_index_for_root(root, config) {
        Ok(index) => {
            println!("  load:        ok");
            println!("  freshness:   {:?}", index.meta.freshness.state);
            if let Some(ref basis) = index.meta.freshness.basis {
                println!("  basis:       {basis}");
            }
            println!(
                "  note: load reassesses git HEAD; stale indexes rebuild automatically."
            );
            if index.meta.git.present && index.meta.git.head.is_none() {
                println!("  warn: git present but HEAD sha not read (permissions / worktree?)");
            }
        }
        Err(e) => {
            println!("  load:        {e}");
            println!("  fix:         turbo tree build");
        }
    }

    // Process cache probe
    if xai_grok_tools::util::workspace_tree_try_get(root).is_some() {
        println!("  process cache: warm");
    } else {
        println!("  process cache: cold (kickoff or first tool call loads)");
    }
    Ok(())
}

fn inject_preview(
    root: &std::path::Path,
    config: &WorkspaceTreeConfig,
    mode: Option<&str>,
    subagent: bool,
) -> Result<()> {
    if !config.enabled {
        println!("workspace tree disabled");
        return Ok(());
    }
    let mut cfg = config.clone();
    if let Some(m) = mode {
        cfg.inject.mode = InjectMode::parse(m).with_context(|| format!("invalid mode `{m}`"))?;
    } else {
        cfg.inject.mode = config.inject_mode_for_audience(subagent);
    }

    // RC13 P2 F20: mode off is not "building".
    if matches!(cfg.inject.mode, InjectMode::Off) {
        println!("{}", inject_disabled_notice());
        return Ok(());
    }

    let index = match xai_grok_tools::util::workspace_tree_try_get(root)
        .or_else(|| xai_grok_tools::util::workspace_tree_try_load_cached(root, &cfg))
    {
        Some(idx) => idx,
        None => {
            // Avoid implying inject-off when cold; build on demand for preview.
            match xai_grok_tools::util::workspace_tree_get_or_load(root, &cfg) {
                Ok(idx) => idx,
                Err(_) => {
                    println!("{}", inject_building_notice());
                    return Ok(());
                }
            }
        }
    };
    let card = inject_card(&index, &cfg);
    if card.is_empty() {
        // Mode non-off but empty card (budget zero / empty tree).
        println!("(empty inject card)");
    } else {
        println!("{card}");
    }
    Ok(())
}

fn search_cmd(
    root: &std::path::Path,
    config: &WorkspaceTreeConfig,
    query: &str,
    limit: usize,
) -> Result<()> {
    if !config.enabled {
        bail!("workspace tree disabled");
    }
    let index = xai_grok_tools::util::workspace_tree_get_or_load(root, config)
        .map_err(|e| anyhow::anyhow!(e))?;
    let result = xai_workspace_tree::search(&index, query, limit.clamp(1, 100));
    if result.hits.is_empty() {
        println!("No hits for `{query}`");
    } else {
        for (i, h) in result.hits.iter().enumerate() {
            println!(
                "{}. {}  (score {:.2})",
                i + 1,
                h.rel_path,
                h.score
            );
        }
    }
    Ok(())
}

fn prune_cmd(
    config: &WorkspaceTreeConfig,
    max_age_days: u64,
    keep_newest: usize,
    dry_run: bool,
) -> Result<()> {
    let (before_dirs, before_bytes) = store_disk_usage(config);
    println!(
        "Store before: {before_dirs} workspace dirs · {before_bytes} bytes under {}",
        xai_workspace_tree::store_root(config).display()
    );
    if dry_run {
        println!(
            "dry-run: would prune indexes older than {max_age_days} day(s) (keep_newest={keep_newest})"
        );
        println!("Dry-run only. Re-run with --execute to delete.");
        return Ok(());
    }
    let max_age = std::time::Duration::from_secs(max_age_days.saturating_mul(24 * 3600));
    let report = prune_store(config, max_age, keep_newest).context("prune_store")?;
    println!(
        "Pruned: removed {} dirs, freed {} bytes; remaining {} dirs · {} bytes",
        report.removed_dirs, report.freed_bytes, report.remaining_dirs, report.remaining_bytes
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct TreeCli {
        #[command(subcommand)]
        command: TreeCommand,
    }

    #[test]
    fn prune_accepts_execute_flag() {
        let cli = TreeCli::try_parse_from(["tree", "prune", "--execute"]).expect("parse");
        match cli.command {
            TreeCommand::Prune {
                execute,
                dry_run,
                ..
            } => {
                assert!(execute);
                assert!(!dry_run);
            }
            other => panic!("expected Prune, got {other:?}"),
        }
    }

    #[test]
    fn prune_default_is_not_execute() {
        let cli = TreeCli::try_parse_from(["tree", "prune"]).expect("parse");
        match cli.command {
            TreeCommand::Prune { execute, dry_run, .. } => {
                assert!(!execute);
                assert!(!dry_run);
            }
            other => panic!("expected Prune, got {other:?}"),
        }
    }

    #[test]
    fn prune_execute_conflicts_with_dry_run() {
        let err = TreeCli::try_parse_from(["tree", "prune", "--execute", "--dry-run"]);
        assert!(err.is_err());
    }
}

fn build(root: &std::path::Path, config: &WorkspaceTreeConfig) -> Result<()> {
    if !config.enabled {
        bail!("workspace tree disabled");
    }
    // RC13 P0 F3: must replace process cache (get_or_load would return stale Arc).
    let index = xai_grok_tools::util::workspace_tree_refresh(root, config)
        .map_err(|e| anyhow::anyhow!(e))
        .context("workspace_tree_refresh")?;
    println!(
        "Built workspace tree: {} files, {} dirs, {}ms → store id {}",
        index.meta.stats.files,
        index.meta.stats.dirs,
        index.meta.build.duration_ms,
        index.meta.workspace_id
    );
    Ok(())
}

fn resolve_cmd(
    root: &std::path::Path,
    config: &WorkspaceTreeConfig,
    name: &str,
    hint: Option<&str>,
    limit: usize,
) -> Result<()> {
    if !config.enabled {
        bail!("workspace tree disabled");
    }
    let index = xai_grok_tools::util::workspace_tree_get_or_load(root, config)
        .map_err(|e| anyhow::anyhow!(e))?;
    let result = xai_workspace_tree::resolve_path(&index, name, hint, limit.clamp(1, 32));
    if result.hits.is_empty() {
        println!("No matches for `{name}`");
    } else {
        for (i, h) in result.hits.iter().enumerate() {
            println!(
                "{}. {}  (score {:.2}, {})",
                i + 1,
                h.rel_path,
                h.score,
                h.reason
            );
        }
    }
    Ok(())
}
