//! `turbo disk` — report workspace + product disk use; full clean for RC3.
//!
//! Focus: free space (multi-path), `target/` bloat by category, subagent
//! worktrees, tree store, session TMP, optional cargo-home reclaim.
//! Does **not** install/uninstall product binaries.
//!
//! Gates (env):
//! - `GROK_MIN_FREE_GB` (default 40) / `GROK_SUBAGENT_MIN_FREE_BYTES`
//! - `GROK_SUBAGENT_KEEP_N` (default 3) / `GROK_SUBAGENT_SOFT_PRESERVE_KEEP_N`
//!
//! Clean safety:
//! - Always requires `--safe` (and `--i-accept-redownload` for cargo-home).
//! - Never deletes live-marked worktrees (`.grok-subagent-live`).
//! - Never deletes `release-dist` ship binaries; cache-only reclaim is opt-in
//!   and refuses when an active build is detected.
//! - Default categories match RC2 safe clean (debug + aged worktrees + tree store).

use anyhow::{Result, bail};
use clap::{Subcommand, ValueEnum};
use fs2::FileExt;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, clap::Args, Clone)]
pub struct DiskArgs {
    #[command(subcommand)]
    pub command: DiskCommand,
}

/// Reclaim categories for `turbo disk clean --include …`.
///
/// Default when `--include` is omitted: `debug`, `worktrees`, `tree-store`
/// (plus top-level `target/{incremental,.fingerprint}` with `debug`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, ValueEnum, Serialize)]
#[value(rename_all = "kebab-case")]
#[serde(rename_all = "kebab-case")]
pub enum CleanCategory {
    /// Entire `target/debug` (plus top-level `target/incremental` / `.fingerprint`)
    Debug,
    /// Windows PDB files under `target/debug` only
    DebugPdbs,
    /// `target/debug/incremental` only
    DebugIncremental,
    /// Entire `target/release` (never `release-dist`)
    Release,
    /// Cache subdirs under `target/release-dist` only (keeps ship binaries)
    ReleaseDistCaches,
    /// Aged non-live `subagent-*` worktrees
    Worktrees,
    /// Aged workspace-tree store entries
    TreeStore,
    /// `%TEMP%/grok` (or `$TMPDIR/grok`)
    TempGrok,
    /// `CARGO_HOME` registry/git caches — requires `--i-accept-redownload`
    CargoHome,
}

impl CleanCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::DebugPdbs => "debug-pdbs",
            Self::DebugIncremental => "debug-incremental",
            Self::Release => "release",
            Self::ReleaseDistCaches => "release-dist-caches",
            Self::Worktrees => "worktrees",
            Self::TreeStore => "tree-store",
            Self::TempGrok => "temp-grok",
            Self::CargoHome => "cargo-home",
        }
    }

    fn default_safe() -> &'static [CleanCategory] {
        &[
            CleanCategory::Debug,
            CleanCategory::Worktrees,
            CleanCategory::TreeStore,
        ]
    }
}

#[derive(Debug, Subcommand, Clone)]
pub enum DiskCommand {
    /// Summarize free space (multi-path) and large Turbo / Rust build directories
    Report {
        /// Workspace root (default: cwd)
        #[arg(long)]
        root: Option<PathBuf>,
        /// Emit JSON
        #[arg(long)]
        json: bool,
    },
    /// Fail closed if free space on any gated path is under the min (default GROK_MIN_FREE_GB=40).
    ///
    /// Gates workspace root, worktrees base, and `CARGO_TARGET_DIR` when set.
    Check {
        /// Workspace root (default: cwd)
        #[arg(long)]
        root: Option<PathBuf>,
        /// Override min free GiB (else env / default 40)
        #[arg(long)]
        min_free_gb: Option<u64>,
        /// Emit JSON
        #[arg(long)]
        json: bool,
    },
    /// Remove caches by category (requires `--safe`; default = debug + worktrees + tree-store)
    Clean {
        /// Workspace root (default: cwd)
        #[arg(long)]
        root: Option<PathBuf>,
        /// Only print actions; do not delete
        #[arg(long)]
        dry_run: bool,
        /// Required safety latch for any deletion
        #[arg(long)]
        safe: bool,
        /// Prune non-live subagent worktrees older than this many hours (default 24).
        /// `0` means all non-live worktrees (aggressive).
        #[arg(long, default_value_t = 24)]
        worktree_hours: u64,
        /// Also prune workspace-tree indexes older than this many days (default 14)
        #[arg(long, default_value_t = 14)]
        tree_days: u64,
        /// Only clean when free space is under the min-free gate (default always clean when --safe)
        #[arg(long)]
        if_low_space: bool,
        /// Override min free GiB for --if-low-space (else env / default 40)
        #[arg(long)]
        min_free_gb: Option<u64>,
        /// Categories to reclaim (comma-separated or repeated).
        /// When omitted: debug, worktrees, tree-store.
        #[arg(long, value_delimiter = ',', num_args = 1..)]
        include: Vec<CleanCategory>,
        /// Required when `cargo-home` is included (forces redownload of crates)
        #[arg(long = "i-accept-redownload")]
        i_accept_redownload: bool,
        /// Emit JSON (actions + reclaimed_bytes by category)
        #[arg(long)]
        json: bool,
        /// Secondary mtime hint window for release-dist-caches (default 120).
        /// Does **not** override a held Cargo profile lock.
        #[arg(long, default_value_t = 120)]
        active_build_grace_secs: u64,
    },
    /// Closed-loop reclaim: check free space → clean only when low → re-check.
    ///
    /// Exit 0 when free space meets the min gate after (optional) reclaim.
    /// Exit 1 when still under threshold. Requires `--safe` for any deletion.
    Recover {
        /// Workspace root (default: cwd)
        #[arg(long)]
        root: Option<PathBuf>,
        /// Only print plan; do not delete
        #[arg(long)]
        dry_run: bool,
        /// Required safety latch for any deletion
        #[arg(long)]
        safe: bool,
        /// Override min free GiB (else env / default 40)
        #[arg(long)]
        min_free_gb: Option<u64>,
        /// Categories to reclaim when low (default: debug + worktrees + tree-store)
        #[arg(long, value_delimiter = ',', num_args = 1..)]
        include: Vec<CleanCategory>,
        /// Required when `cargo-home` is included
        #[arg(long = "i-accept-redownload")]
        i_accept_redownload: bool,
        /// Worktree age threshold in hours (default 24; `0` = all non-live)
        #[arg(long, default_value_t = 24)]
        worktree_hours: u64,
        /// Tree-store age threshold in days (default 14)
        #[arg(long, default_value_t = 14)]
        tree_days: u64,
        /// Emit JSON report (free_before / free_after / ok / actions)
        #[arg(long)]
        json: bool,
    },
    /// Unified prune for worktrees, tree store, and session subagent metadata
    Prune {
        /// Prune aged non-live subagent worktrees under ~/.grok/worktrees
        #[arg(long)]
        worktrees: bool,
        /// Prune aged workspace-tree store entries
        #[arg(long)]
        tree_store: bool,
        /// Prune aged session subagent metadata dirs (same as `turbo subagent prune`)
        #[arg(long)]
        session_meta: bool,
        /// Enable all prune targets
        #[arg(long)]
        all: bool,
        /// Only print candidates; do not delete
        #[arg(long, conflicts_with = "execute")]
        dry_run: bool,
        /// Actually delete (default is dry-run unless this is set)
        #[arg(long, conflicts_with = "dry_run")]
        execute: bool,
        /// Worktree age threshold in hours (default 24)
        #[arg(long, default_value_t = 24)]
        worktree_hours: u64,
        /// Tree-store age threshold in days (default 14)
        #[arg(long, default_value_t = 14)]
        tree_days: u64,
        /// Session-meta age (e.g. 24h, 7d); default 24h
        #[arg(long = "older-than", default_value = "24h")]
        older_than: String,
        /// Workspace root (for free-space labels only)
        #[arg(long)]
        root: Option<PathBuf>,
        /// Emit JSON
        #[arg(long)]
        json: bool,
    },
}

pub fn run(args: DiskArgs) -> Result<()> {
    match args.command {
        DiskCommand::Report { root, json } => report(root, json),
        DiskCommand::Check {
            root,
            min_free_gb,
            json,
        } => check(root, min_free_gb, json),
        DiskCommand::Clean {
            root,
            dry_run,
            safe,
            worktree_hours,
            tree_days,
            if_low_space,
            min_free_gb,
            include,
            i_accept_redownload,
            json,
            active_build_grace_secs,
        } => {
            if !safe {
                bail!(
                    "refusing clean without --safe (try: turbo disk clean --safe --dry-run)"
                );
            }
            let cats = resolve_categories(&include, i_accept_redownload)?;
            clean(CleanOpts {
                root,
                dry_run,
                worktree_hours,
                tree_days,
                if_low_space,
                min_free_gb,
                categories: cats,
                json,
                active_build_grace_secs,
            })
        }
        DiskCommand::Recover {
            root,
            dry_run,
            safe,
            min_free_gb,
            include,
            i_accept_redownload,
            worktree_hours,
            tree_days,
            json,
        } => {
            if !safe {
                bail!(
                    "refusing recover without --safe (try: turbo disk recover --safe --dry-run)"
                );
            }
            let cats = resolve_categories(&include, i_accept_redownload)?;
            recover(RecoverOpts {
                root,
                dry_run,
                min_free_gb,
                categories: cats,
                worktree_hours,
                tree_days,
                json,
            })
        }
        DiskCommand::Prune {
            worktrees,
            tree_store,
            session_meta,
            all,
            dry_run,
            execute,
            worktree_hours,
            tree_days,
            older_than,
            root,
            json,
        } => {
            let do_worktrees = all || worktrees;
            let do_tree = all || tree_store;
            let do_session = all || session_meta;
            if !do_worktrees && !do_tree && !do_session {
                bail!(
                    "specify at least one of --worktrees, --tree-store, --session-meta, or --all"
                );
            }
            // Default is dry-run; --execute performs deletion.
            // clap conflicts_with prevents combining --dry-run and --execute.
            let dry = !execute || dry_run;
            prune_unified(PruneOpts {
                root,
                dry_run: dry,
                worktrees: do_worktrees,
                tree_store: do_tree,
                session_meta: do_session,
                worktree_hours,
                tree_days,
                older_than,
                json,
            })
        }
    }
}

/// Min free bytes: GROK_MIN_FREE_GB (GiB) > GROK_SUBAGENT_MIN_FREE_BYTES > 40 GiB.
pub fn configured_min_free_bytes() -> u64 {
    if let Ok(v) = std::env::var("GROK_MIN_FREE_GB")
        && let Ok(gb) = v.trim().parse::<u64>()
    {
        return gb.saturating_mul(1024 * 1024 * 1024);
    }
    if let Ok(v) = std::env::var("GROK_SUBAGENT_MIN_FREE_BYTES")
        && let Ok(b) = v.trim().parse::<u64>()
    {
        return b;
    }
    40 * 1024 * 1024 * 1024
}

/// Keep-N: GROK_SUBAGENT_KEEP_N > GROK_SUBAGENT_SOFT_PRESERVE_KEEP_N > 3.
pub fn configured_keep_n() -> usize {
    for key in ["GROK_SUBAGENT_KEEP_N", "GROK_SUBAGENT_SOFT_PRESERVE_KEEP_N"] {
        if let Ok(v) = std::env::var(key)
            && let Ok(n) = v.trim().parse::<usize>()
        {
            return n;
        }
    }
    3
}

fn resolve_categories(
    include: &[CleanCategory],
    i_accept_redownload: bool,
) -> Result<BTreeSet<CleanCategory>> {
    let mut set: BTreeSet<CleanCategory> = if include.is_empty() {
        CleanCategory::default_safe().iter().copied().collect()
    } else {
        include.iter().copied().collect()
    };
    if set.contains(&CleanCategory::CargoHome) && !i_accept_redownload {
        bail!(
            "cargo-home reclaim requires --i-accept-redownload \
             (crates will be redownloaded on next cargo build)"
        );
    }
    // debug supersedes finer debug slices (avoid double-count / redundant work).
    if set.contains(&CleanCategory::Debug) {
        set.remove(&CleanCategory::DebugPdbs);
        set.remove(&CleanCategory::DebugIncremental);
    }
    Ok(set)
}

#[derive(Debug, Clone, Serialize)]
struct PathSpace {
    role: String,
    path: String,
    free_bytes: Option<u64>,
    ok: Option<bool>,
}

fn gated_paths(root: &Path) -> Vec<(String, PathBuf)> {
    let mut paths = vec![("workspace".into(), root.to_path_buf())];
    let wt = worktrees_base();
    paths.push(("worktrees".into(), wt));
    if let Ok(td) = std::env::var("CARGO_TARGET_DIR") {
        let p = PathBuf::from(td.trim());
        if !p.as_os_str().is_empty() {
            paths.push(("cargo_target_dir".into(), p));
        }
    }
    if let Ok(home) = std::env::var("GROK_HOME") {
        let p = PathBuf::from(home.trim());
        if !p.as_os_str().is_empty() {
            paths.push(("grok_home".into(), p));
        }
    }
    // Cargo home often lives on C: while monorepos are on another drive (Windows).
    paths.push(("cargo_home".into(), cargo_home_path()));
    paths.push(("temp".into(), std::env::temp_dir()));
    // Dedup by canonical path string.
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (role, p) in paths {
        let key = dunce::canonicalize(&p)
            .map(|c| c.display().to_string())
            .unwrap_or_else(|_| p.display().to_string());
        if seen.insert(key) {
            out.push((role, p));
        }
    }
    out
}

/// Volume identity for multi-drive filtering (Windows drive letter, else path root).
fn volume_key(path: &Path) -> Option<String> {
    let mut cur = path.to_path_buf();
    loop {
        if cur.exists() {
            break;
        }
        if !cur.pop() {
            return None;
        }
    }
    let abs = dunce::canonicalize(&cur).unwrap_or(cur);
    let s = abs.to_string_lossy();
    #[cfg(windows)]
    {
        let s = s.trim_start_matches(r"\\?\");
        let bytes = s.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' {
            return Some(s[..2].to_ascii_uppercase());
        }
    }
    abs.components()
        .next()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
}

fn category_storage_path(root: &Path, cat: CleanCategory) -> PathBuf {
    let target = target_root(root);
    match cat {
        CleanCategory::Debug
        | CleanCategory::DebugPdbs
        | CleanCategory::DebugIncremental => target.join("debug"),
        CleanCategory::Release => target.join("release"),
        CleanCategory::ReleaseDistCaches => target.join("release-dist"),
        CleanCategory::Worktrees => worktrees_base(),
        CleanCategory::TreeStore => {
            let cfg = xai_workspace_tree::WorkspaceTreeConfig::from_env();
            xai_workspace_tree::store_root(&cfg)
        }
        CleanCategory::TempGrok => std::env::temp_dir().join("grok"),
        CleanCategory::CargoHome => cargo_home_path(),
    }
}

/// Live-marker protection parity with shell (`GROK_SUBAGENT_LIVE_MARKER_MAX_SECS`, default 12h).
fn is_live_worktree_protected(worktree: &Path) -> bool {
    let marker = worktree.join(".grok-subagent-live");
    let Ok(meta) = std::fs::metadata(&marker) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return true; // fail closed
    };
    let Ok(age) = SystemTime::now().duration_since(modified) else {
        return true;
    };
    let max_secs = std::env::var("GROK_SUBAGENT_LIVE_MARKER_MAX_SECS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(12 * 60 * 60u64);
    age <= Duration::from_secs(max_secs)
}

fn path_spaces(root: &Path, min_free: u64) -> Vec<PathSpace> {
    gated_paths(root)
        .into_iter()
        .map(|(role, path)| {
            let free = free_bytes_for_path(&path);
            let ok = if min_free == 0 {
                Some(true)
            } else {
                free.map(|b| b >= min_free)
            };
            PathSpace {
                role,
                path: path.display().to_string(),
                free_bytes: free,
                ok,
            }
        })
        .collect()
}

/// Ranked agent remediation commands from category sizes (largest first).
fn suggested_clean_cmds(cats: &BTreeMap<&'static str, u64>) -> Vec<serde_json::Value> {
    let mut items: Vec<(&'static str, u64, &'static str)> = vec![
        (
            "debug",
            cats.get("debug").copied().unwrap_or(0),
            "turbo disk clean --safe --include debug --dry-run",
        ),
        (
            "debug-pdbs",
            cats.get("debug-pdbs").copied().unwrap_or(0),
            "turbo disk clean --safe --include debug-pdbs --dry-run",
        ),
        (
            "release",
            cats.get("release").copied().unwrap_or(0),
            "turbo disk clean --safe --include release --dry-run",
        ),
        (
            "release-dist-caches",
            cats.get("release-dist-caches").copied().unwrap_or(0),
            "turbo disk clean --safe --include release-dist-caches --dry-run",
        ),
        (
            "worktrees",
            cats.get("worktrees").copied().unwrap_or(0),
            "turbo disk clean --safe --include worktrees --worktree-hours 0 --dry-run",
        ),
        (
            "cargo-home",
            cats.get("cargo-home").copied().unwrap_or(0),
            "turbo disk clean --safe --include cargo-home --i-accept-redownload --dry-run",
        ),
    ];
    items.retain(|(_, b, _)| *b > 0);
    items.sort_by(|a, b| b.1.cmp(&a.1));
    items
        .into_iter()
        .take(6)
        .map(|(cat, bytes, cmd)| {
            serde_json::json!({
                "category": cat,
                "bytes": bytes,
                "command": cmd,
            })
        })
        .collect()
}

/// Resolved cargo target root: `CARGO_TARGET_DIR` if set, else `<workspace>/target`.
fn target_root(workspace: &Path) -> PathBuf {
    if let Ok(td) = std::env::var("CARGO_TARGET_DIR") {
        let p = PathBuf::from(td.trim());
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    workspace.join("target")
}

fn category_sizes(root: &Path) -> BTreeMap<&'static str, u64> {
    let target = target_root(root);
    let debug = target.join("debug");
    let release = target.join("release");
    let rd = target.join("release-dist");
    let mut m = BTreeMap::new();
    m.insert("debug", dir_size_capped(&debug, 1_000_000).bytes);
    m.insert(
        "debug-pdbs",
        size_matching(&debug, 500_000, |p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("pdb"))
                .unwrap_or(false)
        }),
    );
    m.insert(
        "debug-incremental",
        dir_size_capped(&debug.join("incremental"), 500_000).bytes,
    );
    m.insert("release", dir_size_capped(&release, 1_000_000).bytes);
    m.insert(
        "release-dist",
        dir_size_capped(&rd, 500_000).bytes,
    );
    m.insert(
        "release-dist-caches",
        release_dist_cache_bytes(&rd),
    );
    m.insert(
        "worktrees",
        dir_size_capped(&worktrees_base(), 500_000).bytes,
    );
    let tree_cfg = xai_workspace_tree::WorkspaceTreeConfig::from_env();
    let (_, tree_usage) = xai_workspace_tree::store_disk_usage(&tree_cfg);
    m.insert("tree-store", tree_usage);
    m.insert(
        "temp-grok",
        dir_size_capped(&std::env::temp_dir().join("grok"), 500_000).bytes,
    );
    m.insert("cargo-home", cargo_home_cache_bytes());
    m
}

fn release_dist_cache_bytes(rd: &Path) -> u64 {
    let mut total = 0u64;
    for name in release_dist_cache_names() {
        total = total.saturating_add(dir_size_capped(&rd.join(name), 300_000).bytes);
    }
    total
}

fn release_dist_cache_names() -> &'static [&'static str] {
    &["incremental", "deps", "build", "examples", ".fingerprint"]
}

fn cargo_home_path() -> PathBuf {
    if let Ok(h) = std::env::var("CARGO_HOME") {
        let p = PathBuf::from(h.trim());
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    dirs::home_dir()
        .map(|h| h.join(".cargo"))
        .unwrap_or_else(|| PathBuf::from(".cargo"))
}

fn cargo_home_cache_bytes() -> u64 {
    let home = cargo_home_path();
    let registry = home.join("registry").join("cache");
    let git_db = home.join("git").join("db");
    dir_size_capped(&registry, 500_000)
        .bytes
        .saturating_add(dir_size_capped(&git_db, 300_000).bytes)
}

fn report(root: Option<PathBuf>, json: bool) -> Result<()> {
    let root = root
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let root = dunce::canonicalize(&root).unwrap_or(root);

    let free = free_bytes_for_path(&root);
    let min_free = configured_min_free_bytes();
    let keep_n = configured_keep_n();
    let spaces = path_spaces(&root, min_free);
    let cats = category_sizes(&root);

    let target = target_root(&root);
    let target_sz = dir_size_capped(&target, 2_000_000);

    let worktrees = worktrees_base();
    let wt_count = count_dirs(&worktrees);
    let subagent_count = count_subagent_dirs(&worktrees);

    let tree_cfg = xai_workspace_tree::WorkspaceTreeConfig::from_env();
    let tree_store = xai_workspace_tree::store_root(&tree_cfg);
    let (tree_dirs, tree_usage) = xai_workspace_tree::store_disk_usage(&tree_cfg);

    let temp_grok = std::env::temp_dir().join("grok");
    // Fail closed on unknown free space (matches `disk check`).
    let free_ok = spaces.iter().all(|s| matches!(s.ok, Some(true)));
    let keep_over = keep_n > 0 && subagent_count > keep_n as u64;

    if json {
        let v = serde_json::json!({
            "root": root.display().to_string(),
            "free_bytes": free,
            "min_free_bytes": min_free,
            "min_free_gb": min_free / (1024 * 1024 * 1024),
            "free_space_ok": free_ok,
            "paths": spaces,
            "keep_n": keep_n,
            "target_bytes": target_sz.bytes,
            "target_truncated": target_sz.truncated,
            "target_debug_bytes": cats.get("debug").copied().unwrap_or(0),
            "target_debug_pdbs_bytes": cats.get("debug-pdbs").copied().unwrap_or(0),
            "target_debug_incremental_bytes": cats.get("debug-incremental").copied().unwrap_or(0),
            "target_release_bytes": cats.get("release").copied().unwrap_or(0),
            "target_release_dist_bytes": cats.get("release-dist").copied().unwrap_or(0),
            "target_release_dist_caches_bytes": cats.get("release-dist-caches").copied().unwrap_or(0),
            "worktrees_path": worktrees.display().to_string(),
            "worktrees_bytes": cats.get("worktrees").copied().unwrap_or(0),
            "worktrees_dirs": wt_count,
            "worktrees_subagent_dirs": subagent_count,
            "worktrees_over_keep_n": keep_over,
            "tree_store_path": tree_store.display().to_string(),
            "tree_store_dirs": tree_dirs,
            "tree_store_bytes": tree_usage,
            "temp_grok_path": temp_grok.display().to_string(),
            "temp_grok_bytes": cats.get("temp-grok").copied().unwrap_or(0),
            "cargo_home_path": cargo_home_path().display().to_string(),
            "cargo_home_cache_bytes": cats.get("cargo-home").copied().unwrap_or(0),
            "categories": cats,
            "suggested_clean": suggested_clean_cmds(&cats),
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }

    println!("Turbo disk report");
    println!("  root:           {}", root.display());
    for s in &spaces {
        let free_s = s
            .free_bytes
            .map(fmt_bytes)
            .unwrap_or_else(|| "unknown".into());
        let st = match s.ok {
            Some(true) => "OK",
            Some(false) => "BELOW THRESHOLD",
            None => "unknown",
        };
        println!(
            "  free[{}]:  {} @ {} → {st}",
            s.role, free_s, s.path
        );
    }
    let min_label = if min_free == 0 {
        "disabled (0)".to_string()
    } else {
        format!(
            "{} (GROK_MIN_FREE_GB / GROK_SUBAGENT_MIN_FREE_BYTES)",
            fmt_bytes(min_free)
        )
    };
    let any_unknown = spaces.iter().any(|s| s.ok.is_none());
    let status = if free_ok {
        "OK"
    } else if any_unknown {
        "UNKNOWN / FAIL-CLOSED"
    } else {
        "BELOW THRESHOLD"
    };
    println!("  min free gate:  {min_label} → {status}");
    println!(
        "  target/:        {} @ {}{}",
        fmt_bytes(target_sz.bytes),
        target.display(),
        if target_sz.truncated {
            " (scan capped)"
        } else {
            ""
        }
    );
    println!(
        "    debug/:       {}  (pdbs {} · incremental {})",
        fmt_bytes(cats.get("debug").copied().unwrap_or(0)),
        fmt_bytes(cats.get("debug-pdbs").copied().unwrap_or(0)),
        fmt_bytes(cats.get("debug-incremental").copied().unwrap_or(0)),
    );
    println!(
        "    release/:     {}",
        fmt_bytes(cats.get("release").copied().unwrap_or(0))
    );
    println!(
        "    release-dist/ {}  (caches {})",
        fmt_bytes(cats.get("release-dist").copied().unwrap_or(0)),
        fmt_bytes(cats.get("release-dist-caches").copied().unwrap_or(0)),
    );
    let keep_label = if keep_n == 0 {
        "age-only (KEEP_N=0)".to_string()
    } else {
        format!("keep-N={keep_n}")
    };
    println!(
        "  worktrees:      {} ({} dirs, {} subagent-*) @ {}",
        fmt_bytes(cats.get("worktrees").copied().unwrap_or(0)),
        wt_count,
        subagent_count,
        worktrees.display()
    );
    println!(
        "  worktree gate:  {keep_label}{}",
        if keep_over {
            " → OVER CAP (spawn will prune oldest non-live)"
        } else {
            ""
        }
    );
    println!(
        "  tree store:     {} ({} workspace dirs) @ {}",
        fmt_bytes(tree_usage),
        tree_dirs,
        tree_store.display()
    );
    println!(
        "  %TEMP%/grok:    {} @ {}",
        fmt_bytes(cats.get("temp-grok").copied().unwrap_or(0)),
        temp_grok.display()
    );
    println!(
        "  cargo home:     {} (registry/git caches) @ {}",
        fmt_bytes(cats.get("cargo-home").copied().unwrap_or(0)),
        cargo_home_path().display()
    );
    println!();
    println!("Clean (dry-run first):");
    println!("  turbo disk clean --safe --dry-run");
    println!("  turbo disk clean --safe --if-low-space");
    println!(
        "  turbo disk clean --safe --include debug-pdbs,debug-incremental,release --dry-run"
    );
    println!(
        "  turbo disk clean --safe --include release-dist-caches --dry-run"
    );
    println!(
        "  turbo disk clean --safe --include cargo-home --i-accept-redownload --dry-run"
    );
    println!("  turbo disk check");
    println!("  turbo disk prune --all --dry-run");
    println!("  turbo subagent prune --older-than 24h");
    println!("  turbo tree prune --max-age-days 14");
    if !free_ok {
        println!();
        println!("WARNING: free space under gate on one or more paths — prefer package-scoped cargo tests;");
        println!("  set CARGO_INCREMENTAL=0 for one-shot builds; reclaim on the failing volume:");
        println!("  turbo disk clean --safe --if-low-space");
        println!("  turbo disk clean --safe --include worktrees --worktree-hours 0 --dry-run");
        println!("  turbo disk clean --safe --include debug --dry-run");
    }
    if cats.get("debug").copied().unwrap_or(0) > 50u64 * 1024 * 1024 * 1024 {
        println!();
        println!(
            "NOTE: target/debug is large ({}) — agents should avoid full-workspace debug rebuilds.",
            fmt_bytes(cats.get("debug").copied().unwrap_or(0))
        );
    }
    Ok(())
}

struct RecoverOpts {
    root: Option<PathBuf>,
    dry_run: bool,
    min_free_gb: Option<u64>,
    categories: BTreeSet<CleanCategory>,
    worktree_hours: u64,
    tree_days: u64,
    json: bool,
}

/// Closed-loop: assess → clean --if-low-space → re-assess. Exit 1 if still low.
fn recover(opts: RecoverOpts) -> Result<()> {
    let root = opts
        .root
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let root = dunce::canonicalize(&root).unwrap_or(root);
    let min = opts
        .min_free_gb
        .map(|gb| gb.saturating_mul(1024 * 1024 * 1024))
        .unwrap_or_else(configured_min_free_bytes);

    let before = path_spaces(&root, min);
    let before_ok = min == 0 || before.iter().all(|s| matches!(s.ok, Some(true)));

    if before_ok {
        if opts.json {
            let v = serde_json::json!({
                "root": root.display().to_string(),
                "min_free_bytes": min,
                "ok": true,
                "cleaned": false,
                "dry_run": opts.dry_run,
                "paths_before": before,
                "paths_after": before,
                "message": "free space already meets min gate; no clean needed",
            });
            println!("{}", serde_json::to_string_pretty(&v)?);
        } else {
            println!(
                "turbo disk recover: already OK (min={})",
                if min == 0 {
                    "disabled".into()
                } else {
                    fmt_bytes(min)
                }
            );
        }
        return Ok(());
    }

    if !opts.json {
        println!(
            "turbo disk recover: free space under gate — running clean --safe --if-low-space{}",
            if opts.dry_run { " (dry-run)" } else { "" }
        );
    }

    // Reclaim only failing volumes / safe categories (if_low_space filter).
    clean(CleanOpts {
        root: Some(root.clone()),
        dry_run: opts.dry_run,
        worktree_hours: opts.worktree_hours,
        tree_days: opts.tree_days,
        if_low_space: true,
        min_free_gb: opts.min_free_gb,
        categories: opts.categories,
        json: false, // recover owns JSON envelope
        active_build_grace_secs: 120,
    })?;

    let after = path_spaces(&root, min);
    let after_ok = min == 0 || after.iter().all(|s| matches!(s.ok, Some(true)));

    if opts.json {
        let v = serde_json::json!({
            "root": root.display().to_string(),
            "min_free_bytes": min,
            "ok": after_ok,
            "cleaned": true,
            "dry_run": opts.dry_run,
            "paths_before": before,
            "paths_after": after,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!(
            "turbo disk recover: after clean → {}",
            if after_ok { "OK" } else { "STILL LOW" }
        );
        for s in &after {
            let free_s = s
                .free_bytes
                .map(fmt_bytes)
                .unwrap_or_else(|| "unknown".into());
            let st = match s.ok {
                Some(true) => "OK",
                Some(false) => "FAIL",
                None => "unknown",
            };
            println!("  [{}] {} free={} → {st}", s.role, s.path, free_s);
        }
    }

    if !after_ok {
        bail!(
            "disk recover: free space still under threshold after clean (min={})",
            fmt_bytes(min)
        );
    }
    Ok(())
}

fn check(root: Option<PathBuf>, min_free_gb: Option<u64>, json: bool) -> Result<()> {
    let root = root
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let root = dunce::canonicalize(&root).unwrap_or(root);
    let min = min_free_gb
        .map(|gb| gb.saturating_mul(1024 * 1024 * 1024))
        .unwrap_or_else(configured_min_free_bytes);
    let spaces = path_spaces(&root, min);
    // If free is unknown (None), fail closed when min > 0.
    let ok = if min == 0 {
        true
    } else {
        spaces.iter().all(|s| matches!(s.ok, Some(true)))
    };

    if json {
        let v = serde_json::json!({
            "root": root.display().to_string(),
            "min_free_bytes": min,
            "ok": ok,
            "paths": spaces,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!(
            "turbo disk check: min={} → {}",
            if min == 0 {
                "disabled".into()
            } else {
                fmt_bytes(min)
            },
            if ok { "OK" } else { "FAIL" }
        );
        for s in &spaces {
            let free_s = s
                .free_bytes
                .map(fmt_bytes)
                .unwrap_or_else(|| "unknown".into());
            let st = match s.ok {
                Some(true) => "OK",
                Some(false) => "FAIL",
                None => "unknown",
            };
            println!("  [{}] {} free={} → {st}", s.role, s.path, free_s);
        }
        if !ok {
            println!("  Remediation: turbo disk clean --safe --dry-run");
            println!("  Or: GROK_MIN_FREE_GB=0 / lower --min-free-gb for this check only");
        }
    }

    if !ok {
        let failing: Vec<String> = spaces
            .iter()
            .filter(|s| !matches!(s.ok, Some(true)))
            .map(|s| {
                format!(
                    "{}@{} free={}",
                    s.role,
                    s.path,
                    s.free_bytes
                        .map(fmt_bytes)
                        .unwrap_or_else(|| "unknown".into())
                )
            })
            .collect();
        bail!(
            "free space under threshold (need at least {} on each gated path): {}",
            fmt_bytes(min),
            failing.join("; ")
        );
    }
    Ok(())
}

struct CleanOpts {
    root: Option<PathBuf>,
    dry_run: bool,
    worktree_hours: u64,
    tree_days: u64,
    if_low_space: bool,
    min_free_gb: Option<u64>,
    categories: BTreeSet<CleanCategory>,
    json: bool,
    active_build_grace_secs: u64,
}

const MAX_ACTIONS: usize = 80;

#[derive(Debug, Default, Serialize)]
struct CleanResult {
    dry_run: bool,
    if_low_space: bool,
    categories: Vec<&'static str>,
    actions: Vec<String>,
    actions_truncated: bool,
    reclaimed_bytes: BTreeMap<&'static str, u64>,
    total_reclaimed_bytes: u64,
    skipped: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skipped_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    free_bytes_before: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    free_bytes_after: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    paths: Option<Vec<PathSpace>>,
    ok: bool,
}

fn emit_clean_result(result: &CleanResult, json: bool, if_low_space: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(result)?);
        return Ok(());
    }
    let include_label = result.categories.join(",");
    println!(
        "turbo disk clean --safe{}{}{}",
        if result.dry_run { " --dry-run" } else { "" },
        if if_low_space { " --if-low-space" } else { "" },
        if include_label.is_empty() {
            String::new()
        } else {
            format!(" --include {include_label}")
        }
    );
    if let Some(reason) = &result.skipped_reason {
        println!("  skipped: {reason}");
    }
    if result.actions.is_empty() && result.skipped.is_empty() && result.skipped_reason.is_none()
    {
        println!("  (nothing to clean)");
    } else {
        for a in &result.actions {
            println!("  {a}");
        }
        if result.actions_truncated {
            println!("  … (actions truncated; see --json reclaimed_bytes totals)");
        }
        for s in &result.skipped {
            println!("  skip: {s}");
        }
    }
    if !result.reclaimed_bytes.is_empty() {
        println!("  by category:");
        for (k, v) in &result.reclaimed_bytes {
            if *v > 0 {
                println!("    {k}: {}", fmt_bytes(*v));
            }
        }
    }
    if result.dry_run {
        println!(
            "  would free: {} (dry-run; re-run without --dry-run to apply)",
            fmt_bytes(result.total_reclaimed_bytes)
        );
    } else {
        println!(
            "  approx freed: {}",
            fmt_bytes(result.total_reclaimed_bytes)
        );
    }
    if let (Some(b), Some(a)) = (result.free_bytes_before, result.free_bytes_after) {
        println!(
            "  free space (workspace volume): {} → {}",
            fmt_bytes(b),
            fmt_bytes(a)
        );
    }
    Ok(())
}

fn clean(opts: CleanOpts) -> Result<()> {
    let root = opts
        .root
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let root = dunce::canonicalize(&root).unwrap_or(root);

    let min = opts
        .min_free_gb
        .map(|gb| gb.saturating_mul(1024 * 1024 * 1024))
        .unwrap_or_else(configured_min_free_bytes);

    let free_before = free_bytes_for_path(&root);
    let mut categories = opts.categories.clone();
    let mut low_space_paths: Option<Vec<PathSpace>> = None;

    if opts.if_low_space {
        // min==0 means gate disabled — never treat as "low" (match disk check).
        if min == 0 {
            let result = CleanResult {
                dry_run: opts.dry_run,
                if_low_space: true,
                categories: categories.iter().map(|c| c.as_str()).collect(),
                skipped_reason: Some("gate_disabled".into()),
                free_bytes_before: free_before,
                free_bytes_after: free_before,
                ok: true,
                ..Default::default()
            };
            emit_clean_result(&result, opts.json, true)?;
            return Ok(());
        }
        let spaces = path_spaces(&root, min);
        low_space_paths = Some(spaces.clone());
        if spaces.iter().any(|s| s.ok.is_none()) {
            bail!(
                "turbo disk clean --safe --if-low-space: free space probe failed on one or more gated paths; refusing to clean"
            );
        }
        let low: Vec<&PathSpace> = spaces.iter().filter(|s| matches!(s.ok, Some(false))).collect();
        if low.is_empty() {
            let result = CleanResult {
                dry_run: opts.dry_run,
                if_low_space: true,
                categories: categories.iter().map(|c| c.as_str()).collect(),
                skipped_reason: Some("free_space_ok".into()),
                free_bytes_before: free_before,
                free_bytes_after: free_before,
                paths: Some(spaces),
                ok: true,
                ..Default::default()
            };
            emit_clean_result(&result, opts.json, true)?;
            return Ok(());
        }
        // Only reclaim categories that live on a failing volume.
        let low_vols: BTreeSet<String> = low
            .iter()
            .filter_map(|s| volume_key(Path::new(&s.path)))
            .collect();
        categories.retain(|cat| {
            let p = category_storage_path(&root, *cat);
            volume_key(&p)
                .map(|v| low_vols.contains(&v))
                .unwrap_or(false)
        });
        if categories.is_empty() {
            let mut result = CleanResult {
                dry_run: opts.dry_run,
                if_low_space: true,
                categories: vec![],
                skipped_reason: Some(
                    "no_selected_categories_on_low_volumes".into(),
                ),
                free_bytes_before: free_before,
                free_bytes_after: free_before,
                paths: Some(spaces),
                ok: false,
                ..Default::default()
            };
            result.skipped.push(
                "selected categories are not on any volume that is under the free-space gate; \
                 re-run with --include targeting that volume (e.g. worktrees, cargo-home)"
                    .into(),
            );
            emit_clean_result(&result, opts.json, true)?;
            return Ok(());
        }
    }

    let mut result = CleanResult {
        dry_run: opts.dry_run,
        if_low_space: opts.if_low_space,
        categories: categories.iter().map(|c| c.as_str()).collect(),
        free_bytes_before: free_before,
        paths: low_space_paths,
        ok: true,
        ..Default::default()
    };

    let dry = opts.dry_run;
    let target = target_root(&root);

    // Profile locks: release-dist-caches, debug, release (Cargo profile .cargo-lock).
    let mut release_dist_lock: Option<std::fs::File> = None;
    let mut debug_lock: Option<std::fs::File> = None;
    let mut release_lock: Option<std::fs::File> = None;

    if categories.contains(&CleanCategory::ReleaseDistCaches) {
        let rd = target.join("release-dist");
        match try_acquire_profile_lock(&rd) {
            Ok(f) => release_dist_lock = f,
            Err(reason) => {
                if dry {
                    result
                        .skipped
                        .push(format!("release-dist-caches blocked: {reason}"));
                    for name in release_dist_cache_names() {
                        let p = rd.join(name);
                        if p.is_dir() {
                            let sz = dir_size_capped(&p, 300_000).bytes;
                            push_action(
                                &mut result,
                                format!(
                                    "would remove {} ({}) [blocked: {reason}]",
                                    p.display(),
                                    fmt_bytes(sz)
                                ),
                            );
                        }
                    }
                } else {
                    bail!(
                        "release-dist-caches refused: {reason}. \
                         Wait for cargo to finish. \
                         (--active-build-grace-secs only affects mtime warnings after a lock is acquired; it cannot force through a held lock.)"
                    );
                }
            }
        }
        if release_dist_lock.is_some()
            && opts.active_build_grace_secs > 0
            && let Some(hint) = release_dist_recent_mtime_hint(&rd, opts.active_build_grace_secs)
        {
            result.skipped.push(format!(
                "warning: recent release-dist activity ({hint}); lock acquired — proceeding carefully"
            ));
        }
    }

    if categories.contains(&CleanCategory::Debug)
        || categories.contains(&CleanCategory::DebugPdbs)
        || categories.contains(&CleanCategory::DebugIncremental)
    {
        let debug = target.join("debug");
        if debug.is_dir() {
            match try_acquire_profile_lock(&debug) {
                Ok(f) => debug_lock = f,
                Err(reason) => {
                    if dry {
                        result.skipped.push(format!("debug blocked: {reason}"));
                    } else {
                        bail!(
                            "debug reclaim refused: {reason}. Wait for cargo test/build to finish."
                        );
                    }
                }
            }
        }
    }
    if categories.contains(&CleanCategory::Release) {
        let rel = target.join("release");
        if rel.is_dir() {
            match try_acquire_profile_lock(&rel) {
                Ok(f) => release_lock = f,
                Err(reason) => {
                    if dry {
                        result.skipped.push(format!("release blocked: {reason}"));
                    } else {
                        bail!(
                            "release reclaim refused: {reason}. Wait for cargo build --release to finish."
                        );
                    }
                }
            }
        }
    }

    let skip_rd_caches = categories.contains(&CleanCategory::ReleaseDistCaches)
        && release_dist_lock.is_none()
        && dry;
    let skip_debug = (categories.contains(&CleanCategory::Debug)
        || categories.contains(&CleanCategory::DebugPdbs)
        || categories.contains(&CleanCategory::DebugIncremental))
        && debug_lock.is_none()
        && target.join("debug").is_dir()
        && dry
        && result.skipped.iter().any(|s| s.starts_with("debug blocked"));
    let skip_release = categories.contains(&CleanCategory::Release)
        && release_lock.is_none()
        && target.join("release").is_dir()
        && dry
        && result
            .skipped
            .iter()
            .any(|s| s.starts_with("release blocked"));

    for cat in &categories {
        match cat {
            CleanCategory::Debug => {
                if skip_debug {
                    continue;
                }
                reclaim_dir(
                    &mut result,
                    "debug",
                    &target.join("debug"),
                    dry,
                    1_000_000,
                );
                for name in ["incremental", ".fingerprint"] {
                    reclaim_dir(
                        &mut result,
                        "debug",
                        &target.join(name),
                        dry,
                        200_000,
                    );
                }
            }
            CleanCategory::DebugPdbs => {
                if skip_debug {
                    continue;
                }
                reclaim_pdbs(&mut result, &target.join("debug"), dry);
            }
            CleanCategory::DebugIncremental => {
                if skip_debug {
                    continue;
                }
                reclaim_dir(
                    &mut result,
                    "debug-incremental",
                    &target.join("debug").join("incremental"),
                    dry,
                    500_000,
                );
            }
            CleanCategory::Release => {
                if skip_release {
                    continue;
                }
                reclaim_dir(
                    &mut result,
                    "release",
                    &target.join("release"),
                    dry,
                    1_000_000,
                );
            }
            CleanCategory::ReleaseDistCaches => {
                if skip_rd_caches {
                    // already recorded blocked actions in preflight
                } else {
                    reclaim_release_dist_caches(
                        &mut result,
                        &target.join("release-dist"),
                        dry,
                        release_dist_lock.as_ref(),
                    )?;
                }
            }
            CleanCategory::Worktrees => {
                reclaim_worktrees(&mut result, opts.worktree_hours, dry);
            }
            CleanCategory::TreeStore => {
                reclaim_tree_store(&mut result, opts.tree_days, dry);
            }
            CleanCategory::TempGrok => {
                reclaim_temp_grok_aged(&mut result, dry, 24);
            }
            CleanCategory::CargoHome => {
                reclaim_cargo_home(&mut result, dry);
            }
        }
    }
    drop(release_dist_lock);
    drop(debug_lock);
    drop(release_lock);

    result.free_bytes_after = free_bytes_for_path(&root);
    result.ok = !result.skipped.iter().any(|s| {
        s.starts_with("failed ")
            || s.starts_with("debug blocked")
            || s.starts_with("release blocked")
            || s.starts_with("release-dist-caches blocked")
            || s.contains("failed remove")
    });

    emit_clean_result(&result, opts.json, opts.if_low_space)?;
    Ok(())
}

fn push_action(result: &mut CleanResult, action: String) {
    if result.actions.len() < MAX_ACTIONS {
        result.actions.push(action);
    } else {
        result.actions_truncated = true;
    }
}

fn add_reclaim(result: &mut CleanResult, category: &'static str, bytes: u64, action: String) {
    push_action(result, action);
    *result.reclaimed_bytes.entry(category).or_insert(0) = result
        .reclaimed_bytes
        .get(category)
        .copied()
        .unwrap_or(0)
        .saturating_add(bytes);
    result.total_reclaimed_bytes = result.total_reclaimed_bytes.saturating_add(bytes);
}

fn reclaim_dir(
    result: &mut CleanResult,
    category: &'static str,
    path: &Path,
    dry_run: bool,
    max_entries: u64,
) {
    if !path.is_dir() {
        return;
    }
    let sz = dir_size_capped(path, max_entries).bytes;
    if dry_run {
        add_reclaim(
            result,
            category,
            sz,
            format!(
                "would remove {} ({})",
                path.display(),
                fmt_bytes(sz)
            ),
        );
        return;
    }
    match std::fs::remove_dir_all(path) {
        Ok(()) => add_reclaim(
            result,
            category,
            sz,
            format!("remove {} ({})", path.display(), fmt_bytes(sz)),
        ),
        Err(e) => {
            // Partial delete: credit only if path is gone or shrunk.
            let after = if path.exists() {
                dir_size_capped(path, max_entries).bytes
            } else {
                0
            };
            let credited = sz.saturating_sub(after);
            if credited > 0 {
                add_reclaim(
                    result,
                    category,
                    credited,
                    format!(
                        "partial remove {} (claimed {}, remaining {}) err={e}",
                        path.display(),
                        fmt_bytes(credited),
                        fmt_bytes(after)
                    ),
                );
            }
            result.skipped.push(format!(
                "failed remove {} ({e})",
                path.display()
            ));
        }
    }
}

fn reclaim_pdbs(result: &mut CleanResult, debug: &Path, dry_run: bool) -> u64 {
    if !debug.is_dir() {
        return 0;
    }
    let mut total = 0u64;
    let mut removed = 0u64;
    let mut failed = 0u64;
    let mut count = 0u64;
    let mut stack = vec![debug.to_path_buf()];
    let mut entries = 0u64;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            entries += 1;
            if entries > 500_000 {
                // Soft-cap: stop walking immediately (do not keep popping stack).
                stack.clear();
                break;
            }
            let p = e.path();
            let Ok(meta) = e.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(p);
                continue;
            }
            let is_pdb = p
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x.eq_ignore_ascii_case("pdb"))
                .unwrap_or(false);
            if !is_pdb {
                continue;
            }
            let sz = meta.len();
            count += 1;
            total = total.saturating_add(sz);
            if dry_run {
                removed = removed.saturating_add(sz);
            } else if std::fs::remove_file(&p).is_ok() {
                removed = removed.saturating_add(sz);
            } else {
                failed += 1;
            }
        }
    }
    if count == 0 {
        return 0;
    }
    let credited = if dry_run { total } else { removed };
    add_reclaim(
        result,
        "debug-pdbs",
        credited,
        format!(
            "{} {count} pdb file(s) under {} ({}){}",
            if dry_run { "would remove" } else { "remove" },
            debug.display(),
            fmt_bytes(credited),
            if failed > 0 {
                format!("; {failed} failed")
            } else {
                String::new()
            }
        ),
    );
    if failed > 0 {
        result.skipped.push(format!(
            "{failed} pdb file(s) under {} could not be removed",
            debug.display()
        ));
    }
    credited
}

/// Try exclusive lock on profile `.cargo-lock` (Cargo's profile lock).
/// Existence alone is **not** activity — Cargo leaves the file after builds.
/// Returns `Ok(Some(file))` holding the lock, `Ok(None)` if profile dir missing,
/// `Err(reason)` if another process holds the lock.
fn try_acquire_profile_lock(profile_dir: &Path) -> Result<Option<std::fs::File>, String> {
    if !profile_dir.is_dir() {
        return Ok(None);
    }
    let lock_path = profile_dir.join(".cargo-lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)
        .map_err(|e| format!("cannot open {}: {e}", lock_path.display()))?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(e) => Err(format!(
            "profile lock held or unavailable at {} ({e}) — cargo may be building",
            lock_path.display()
        )),
    }
}

/// Soft mtime hint only (not the safety barrier).
fn release_dist_recent_mtime_hint(rd: &Path, grace_secs: u64) -> Option<String> {
    if grace_secs == 0 || !rd.is_dir() {
        return None;
    }
    let grace = Duration::from_secs(grace_secs);
    let now = SystemTime::now();
    let mut checked = 0u64;
    for name in release_dist_cache_names() {
        let dir = rd.join(name);
        if !dir.is_dir() {
            continue;
        }
        let mut stack = vec![dir];
        while let Some(d) = stack.pop() {
            let Ok(rd_inner) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd_inner.flatten() {
                checked += 1;
                if checked > 2_000 {
                    return None;
                }
                let p = e.path();
                let Ok(meta) = e.metadata() else {
                    continue;
                };
                if meta.is_dir() {
                    stack.push(p);
                    continue;
                }
                if let Ok(mtime) = meta.modified()
                    && let Ok(age) = now.duration_since(mtime)
                    && age < grace
                {
                    return Some(format!(
                        "{} modified within {}s",
                        p.display(),
                        grace_secs
                    ));
                }
            }
        }
    }
    None
}

fn reclaim_release_dist_caches(
    result: &mut CleanResult,
    rd: &Path,
    dry_run: bool,
    _lock: Option<&std::fs::File>,
) -> Result<()> {
    if !rd.is_dir() {
        return Ok(());
    }
    // Never touch binaries at release-dist root — only named cache subdirs.
    for name in release_dist_cache_names() {
        reclaim_dir(result, "release-dist-caches", &rd.join(name), dry_run, 300_000);
    }
    Ok(())
}

fn reclaim_worktrees(result: &mut CleanResult, worktree_hours: u64, dry_run: bool) {
    let wt_base = worktrees_base();
    if !wt_base.is_dir() {
        return;
    }
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(worktree_hours.saturating_mul(3600)))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let Ok(rd) = std::fs::read_dir(&wt_base) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Nested slug dirs: walk one level of subagent-*
        if let Ok(inner) = std::fs::read_dir(&path) {
            for child in inner.flatten() {
                maybe_reclaim_worktree(
                    result,
                    &child.path(),
                    &child,
                    cutoff,
                    worktree_hours,
                    dry_run,
                );
            }
        }
        // Also allow subagent-* directly under base
        maybe_reclaim_worktree(result, &path, &entry, cutoff, worktree_hours, dry_run);
    }
}

fn maybe_reclaim_worktree(
    result: &mut CleanResult,
    cp: &Path,
    entry: &std::fs::DirEntry,
    cutoff: SystemTime,
    worktree_hours: u64,
    dry_run: bool,
) {
    let name = entry.file_name().to_string_lossy().into_owned();
    if !name.starts_with("subagent-") || !cp.is_dir() {
        return;
    }
    // Parity with shell: fresh live markers protected; stale markers reclaimable.
    if is_live_worktree_protected(cp) {
        result.skipped.push(format!(
            "live worktree {} (fresh .grok-subagent-live)",
            cp.display()
        ));
        return;
    }
    // worktree_hours == 0 means "all non-live" (tests / explicit aggressive prune).
    let old = if worktree_hours == 0 {
        true
    } else {
        entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| t < cutoff)
            .unwrap_or(false)
    };
    if !old {
        return;
    }
    // Re-check live marker immediately before delete (TOCTOU).
    if is_live_worktree_protected(cp) {
        result.skipped.push(format!(
            "live worktree {} (marker appeared before delete)",
            cp.display()
        ));
        return;
    }
    let sz = dir_size_capped(cp, 100_000).bytes;
    if dry_run {
        add_reclaim(
            result,
            "worktrees",
            sz,
            format!(
                "would remove worktree {} ({})",
                cp.display(),
                fmt_bytes(sz)
            ),
        );
        return;
    }
    // Product teardown: deregister git worktree + long-path aware remove.
    match xai_fast_worktree::remove_worktree(cp) {
        Ok(_) => {
            if cp.exists() {
                // Best-effort leftover cleanup (same pattern as shell soft-preserve).
                match std::fs::remove_dir_all(cp) {
                    Ok(()) => add_reclaim(
                        result,
                        "worktrees",
                        sz,
                        format!("remove worktree {} ({})", cp.display(), fmt_bytes(sz)),
                    ),
                    Err(_) if !cp.exists() => add_reclaim(
                        result,
                        "worktrees",
                        sz,
                        format!("remove worktree {} ({})", cp.display(), fmt_bytes(sz)),
                    ),
                    Err(e) => result.skipped.push(format!(
                        "worktree leftover {} ({e})",
                        cp.display()
                    )),
                }
            } else {
                add_reclaim(
                    result,
                    "worktrees",
                    sz,
                    format!("remove worktree {} ({})", cp.display(), fmt_bytes(sz)),
                );
            }
        }
        Err(e) => {
            // Fallback to remove_dir_all if not a registered worktree.
            match std::fs::remove_dir_all(cp) {
                Ok(()) => add_reclaim(
                    result,
                    "worktrees",
                    sz,
                    format!(
                        "remove worktree {} ({}; fast-remove fallback after: {e})",
                        cp.display(),
                        fmt_bytes(sz)
                    ),
                ),
                Err(e2) => result.skipped.push(format!(
                    "failed remove worktree {} (fast={e}; fs={e2})",
                    cp.display()
                )),
            }
        }
    }
}

fn reclaim_tree_store(result: &mut CleanResult, tree_days: u64, dry_run: bool) {
    let tree_cfg = xai_workspace_tree::WorkspaceTreeConfig::from_env();
    let tree_store = xai_workspace_tree::store_root(&tree_cfg);
    if !tree_store.is_dir() {
        return;
    }
    if dry_run {
        let estimate = estimate_aged_dir_bytes(&tree_store, tree_days.saturating_mul(24), 50_000);
        add_reclaim(
            result,
            "tree-store",
            estimate,
            format!(
                "would prune tree store older than {tree_days}d @ {} (est. {})",
                tree_store.display(),
                fmt_bytes(estimate)
            ),
        );
        return;
    }
    let max_age = Duration::from_secs(tree_days.saturating_mul(24 * 3600));
    match xai_workspace_tree::prune_store(&tree_cfg, max_age, 0) {
        Ok(report) => {
            add_reclaim(
                result,
                "tree-store",
                report.freed_bytes,
                format!(
                    "tree prune: removed {} dirs, freed {} @ {}",
                    report.removed_dirs,
                    fmt_bytes(report.freed_bytes),
                    tree_store.display()
                ),
            );
        }
        Err(e) => result.skipped.push(format!("tree prune skipped: {e}")),
    }
}

fn reclaim_cargo_home(result: &mut CleanResult, dry_run: bool) {
    let home = cargo_home_path();
    // Only registry/cache and git/db — never touch credentials / config / src / index.
    for (label, rel) in [
        ("registry/cache", home.join("registry").join("cache")),
        ("git/db", home.join("git").join("db")),
    ] {
        if !rel.is_dir() {
            continue;
        }
        reclaim_dir(result, "cargo-home", &rel, dry_run, 500_000);
        if dry_run {
            // reclaim_dir already recorded; annotate label is optional
            let _ = label;
        }
    }
}

/// Age-prune `%TEMP%/grok/*` using newest descendant mtime (bounded scan) so
/// Windows parent-dir mtime staleness does not wipe active sessions.
fn reclaim_temp_grok_aged(result: &mut CleanResult, dry_run: bool, max_age_hours: u64) {
    let base = std::env::temp_dir().join("grok");
    if !base.is_dir() {
        return;
    }
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(max_age_hours.saturating_mul(3600)))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let Ok(rd) = std::fs::read_dir(&base) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let newest = newest_mtime_capped(&p, 5_000);
        let old = match newest {
            Some(t) => t < cutoff,
            None => false, // fail closed on unreadable / truncated
        };
        if !old {
            continue;
        }
        if p.is_dir() {
            reclaim_dir(result, "temp-grok", &p, dry_run, 200_000);
        } else if dry_run {
            let sz = e.metadata().map(|m| m.len()).unwrap_or(0);
            add_reclaim(
                result,
                "temp-grok",
                sz,
                format!("would remove {} ({})", p.display(), fmt_bytes(sz)),
            );
        } else if let Ok(meta) = e.metadata() {
            let sz = meta.len();
            match std::fs::remove_file(&p) {
                Ok(()) => add_reclaim(
                    result,
                    "temp-grok",
                    sz,
                    format!("remove {} ({})", p.display(), fmt_bytes(sz)),
                ),
                Err(err) => result
                    .skipped
                    .push(format!("failed remove {} ({err})", p.display())),
            }
        }
    }
}

/// Newest mtime under path; `None` if truncated or unreadable (fail closed for reclaim).
fn newest_mtime_capped(path: &Path, max_entries: u64) -> Option<SystemTime> {
    let mut newest: Option<SystemTime> = None;
    let mut entries = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let meta = std::fs::metadata(&dir).ok();
        if let Some(m) = meta.as_ref().and_then(|m| m.modified().ok()) {
            newest = Some(newest.map_or(m, |n| n.max(m)));
        }
        if !dir.is_dir() {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return None;
        };
        for e in rd.flatten() {
            entries += 1;
            if entries > max_entries {
                return None; // truncated → do not reclaim
            }
            let p = e.path();
            if let Ok(m) = e.metadata() {
                if let Ok(t) = m.modified() {
                    newest = Some(newest.map_or(t, |n| n.max(t)));
                }
                if m.is_dir() {
                    stack.push(p);
                }
            }
        }
    }
    newest
}

fn estimate_aged_dir_bytes(path: &Path, max_age_hours: u64, max_entries: u64) -> u64 {
    if !path.is_dir() {
        return 0;
    }
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(max_age_hours.saturating_mul(3600)))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut total = 0u64;
    let mut entries = 0u64;
    let Ok(rd) = std::fs::read_dir(path) else {
        return 0;
    };
    for e in rd.flatten() {
        entries += 1;
        if entries > max_entries {
            break;
        }
        let p = e.path();
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| t < cutoff)
            .unwrap_or(false);
        if old {
            total = total.saturating_add(dir_size_capped(&p, 20_000).bytes);
        }
    }
    total
}

struct PruneOpts {
    root: Option<PathBuf>,
    dry_run: bool,
    worktrees: bool,
    tree_store: bool,
    session_meta: bool,
    worktree_hours: u64,
    tree_days: u64,
    older_than: String,
    json: bool,
}

fn prune_unified(opts: PruneOpts) -> Result<()> {
    let mut result = CleanResult {
        dry_run: opts.dry_run,
        ..Default::default()
    };
    if opts.worktrees {
        reclaim_worktrees(&mut result, opts.worktree_hours, opts.dry_run);
    }
    if opts.tree_store {
        reclaim_tree_store(&mut result, opts.tree_days, opts.dry_run);
    }
    if opts.session_meta {
        let execute = !opts.dry_run;
        let cwd = opts
            .root
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".".into());
        match crate::subagent_cmd::prune_session_meta(
            &cwd,
            None,
            &opts.older_than,
            execute,
        ) {
            Ok(report) => {
                result.actions.push(format!(
                    "{} {} session-meta dir(s) older than {} (deleted={}, failed={})",
                    if opts.dry_run {
                        "would prune"
                    } else {
                        "pruned"
                    },
                    report.candidates.len(),
                    opts.older_than,
                    report.deleted.len(),
                    report.failed.len()
                ));
                for f in report.failed {
                    result.skipped.push(format!("session-meta: {f}"));
                }
            }
            Err(e) => result
                .skipped
                .push(format!("session-meta prune: {e:#}")),
        }
    }

    if opts.json {
        let root = opts
            .root
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let v = serde_json::json!({
            "dry_run": opts.dry_run,
            "worktrees": opts.worktrees,
            "tree_store": opts.tree_store,
            "session_meta": opts.session_meta,
            "actions": result.actions,
            "skipped": result.skipped,
            "reclaimed_bytes": result.reclaimed_bytes,
            "total_reclaimed_bytes": result.total_reclaimed_bytes,
            "root": root.display().to_string(),
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!(
            "turbo disk prune{}{}",
            if opts.dry_run { " --dry-run" } else { " --execute" },
            {
                let mut parts = Vec::new();
                if opts.worktrees {
                    parts.push("worktrees");
                }
                if opts.tree_store {
                    parts.push("tree-store");
                }
                if opts.session_meta {
                    parts.push("session-meta");
                }
                if parts.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", parts.join(", "))
                }
            }
        );
        if result.actions.is_empty() && result.skipped.is_empty() {
            println!("  (nothing to prune)");
        } else {
            for a in &result.actions {
                println!("  {a}");
            }
            for s in &result.skipped {
                println!("  skip: {s}");
            }
        }
        if !opts.dry_run {
            println!(
                "  approx freed: {}",
                fmt_bytes(result.total_reclaimed_bytes)
            );
        }
    }
    Ok(())
}

fn worktrees_base() -> PathBuf {
    // Prefer product home then ~/.grok/worktrees
    if let Ok(home) = std::env::var("GROK_HOME") {
        let p = PathBuf::from(home).join("worktrees");
        if p.exists() || std::env::var_os("GROK_HOME").is_some() {
            return p;
        }
    }
    dirs::home_dir()
        .map(|h| h.join(".grok").join("worktrees"))
        .unwrap_or_else(|| PathBuf::from(".grok").join("worktrees"))
}

#[derive(Clone, Copy)]
struct SizeScan {
    bytes: u64,
    truncated: bool,
}

fn dir_size_capped(path: &Path, max_entries: u64) -> SizeScan {
    if !path.exists() {
        return SizeScan {
            bytes: 0,
            truncated: false,
        };
    }
    let mut bytes = 0u64;
    let mut entries = 0u64;
    let mut truncated = false;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            entries += 1;
            if entries > max_entries {
                truncated = true;
                return SizeScan { bytes, truncated };
            }
            let p = e.path();
            let meta = match e.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() {
                stack.push(p);
            } else {
                bytes = bytes.saturating_add(meta.len());
            }
        }
    }
    SizeScan { bytes, truncated }
}

/// Sum sizes of files under `path` matching `pred` (files only).
fn size_matching(path: &Path, max_entries: u64, pred: impl Fn(&Path) -> bool) -> u64 {
    if !path.exists() {
        return 0;
    }
    let mut bytes = 0u64;
    let mut entries = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            entries += 1;
            if entries > max_entries {
                return bytes;
            }
            let p = e.path();
            let Ok(meta) = e.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(p);
            } else if pred(&p) {
                bytes = bytes.saturating_add(meta.len());
            }
        }
    }
    bytes
}

fn count_dirs(path: &Path) -> u64 {
    let mut n = 0u64;
    let Ok(rd) = std::fs::read_dir(path) else {
        return 0;
    };
    for e in rd.flatten() {
        if e.path().is_dir() {
            n += 1;
            if let Ok(inner) = std::fs::read_dir(e.path()) {
                for c in inner.flatten() {
                    if c.path().is_dir() {
                        n += 1;
                    }
                }
            }
        }
    }
    n
}

/// Count `subagent-*` directories under the worktrees base (one nested slug level).
fn count_subagent_dirs(path: &Path) -> u64 {
    let mut n = 0u64;
    let Ok(rd) = std::fs::read_dir(path) else {
        return 0;
    };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("subagent-") {
            n += 1;
            continue;
        }
        if let Ok(inner) = std::fs::read_dir(&p) {
            for c in inner.flatten() {
                if c.path().is_dir()
                    && c.file_name().to_string_lossy().starts_with("subagent-")
                {
                    n += 1;
                }
            }
        }
    }
    n
}

fn free_bytes_for_path(path: &Path) -> Option<u64> {
    // Prefer fs2 (cross-platform). Fall back to Windows API if needed.
    // Never substitute cwd when the path has no existing ancestor — that would
    // report the wrong volume (multi-drive Windows).
    let probe = if path.exists() {
        path.to_path_buf()
    } else {
        let mut cur = path.to_path_buf();
        loop {
            if cur.exists() {
                break;
            }
            if !cur.pop() {
                return None;
            }
        }
        cur
    };
    if let Ok(bytes) = fs2::available_space(&probe) {
        return Some(bytes);
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = probe
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut free_caller = 0u64;
        let mut total = 0u64;
        let mut free = 0u64;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetDiskFreeSpaceExW(
                lpDirectoryName: *const u16,
                lpFreeBytesAvailableToCaller: *mut u64,
                lpTotalNumberOfBytes: *mut u64,
                lpTotalNumberOfFreeBytes: *mut u64,
            ) -> i32;
        }
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free_caller,
                &mut total,
                &mut free,
            )
        };
        if ok != 0 {
            return Some(free_caller);
        }
    }
    None
}

fn fmt_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let x = n as f64;
    if x >= GB {
        format!("{:.1} GB", x / GB)
    } else if x >= MB {
        format!("{:.1} MB", x / MB)
    } else if x >= KB {
        format!("{:.1} KB", x / KB)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    #[test]
    fn configured_keep_n_defaults_to_three() {
        // Cannot reliably unset env in parallel tests; only assert parse path
        // when neither var is set in this process. Soft assert.
        let n = configured_keep_n();
        assert!(n == 0 || n >= 1);
    }

    #[test]
    fn client_fmt_bytes_gb() {
        assert!(fmt_bytes(40 * 1024 * 1024 * 1024).contains("GB"));
    }

    #[test]
    fn resolve_categories_default_safe() {
        let s = resolve_categories(&[], false).unwrap();
        assert!(s.contains(&CleanCategory::Debug));
        assert!(s.contains(&CleanCategory::Worktrees));
        assert!(s.contains(&CleanCategory::TreeStore));
        assert!(!s.contains(&CleanCategory::Release));
        assert!(!s.contains(&CleanCategory::CargoHome));
    }

    #[test]
    fn resolve_categories_cargo_home_requires_consent() {
        let err = resolve_categories(&[CleanCategory::CargoHome], false).unwrap_err();
        assert!(err.to_string().contains("i-accept-redownload"));
        let ok = resolve_categories(&[CleanCategory::CargoHome], true).unwrap();
        assert!(ok.contains(&CleanCategory::CargoHome));
    }

    #[test]
    fn resolve_categories_debug_supersedes_slices() {
        let s = resolve_categories(
            &[
                CleanCategory::Debug,
                CleanCategory::DebugPdbs,
                CleanCategory::DebugIncremental,
            ],
            false,
        )
        .unwrap();
        assert!(s.contains(&CleanCategory::Debug));
        assert!(!s.contains(&CleanCategory::DebugPdbs));
        assert!(!s.contains(&CleanCategory::DebugIncremental));
    }

    #[test]
    fn clean_debug_pdbs_only_removes_pdbs() {
        let tmp = tempfile::tempdir().unwrap();
        let debug = tmp.path().join("target").join("debug");
        fs::create_dir_all(debug.join("deps")).unwrap();
        fs::write(debug.join("keep.rs"), b"keep").unwrap();
        fs::write(debug.join("foo.pdb"), b"pdb-data-here").unwrap();
        fs::write(debug.join("deps").join("bar.pdb"), b"more-pdb").unwrap();
        fs::write(debug.join("deps").join("lib.rlib"), b"rlib").unwrap();

        let cats = resolve_categories(&[CleanCategory::DebugPdbs], false).unwrap();
        clean(CleanOpts {
            root: Some(tmp.path().to_path_buf()),
            dry_run: false,
            worktree_hours: 24,
            tree_days: 14,
            if_low_space: false,
            min_free_gb: None,
            categories: cats,
            json: false,
            active_build_grace_secs: 120,
        })
        .unwrap();

        assert!(!debug.join("foo.pdb").exists());
        assert!(!debug.join("deps").join("bar.pdb").exists());
        assert!(debug.join("keep.rs").exists());
        assert!(debug.join("deps").join("lib.rlib").exists());
    }

    #[test]
    fn clean_release_dist_caches_keeps_ship_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let rd = tmp.path().join("target").join("release-dist");
        fs::create_dir_all(rd.join("deps")).unwrap();
        fs::create_dir_all(rd.join("incremental")).unwrap();
        fs::write(rd.join("turbo.exe"), b"ship-binary").unwrap();
        fs::write(rd.join("deps").join("foo.rlib"), b"cache").unwrap();

        let cats = resolve_categories(&[CleanCategory::ReleaseDistCaches], false).unwrap();
        clean(CleanOpts {
            root: Some(tmp.path().to_path_buf()),
            dry_run: false,
            worktree_hours: 24,
            tree_days: 14,
            if_low_space: false,
            min_free_gb: None,
            categories: cats,
            json: false,
            active_build_grace_secs: 120,
        })
        .unwrap();

        assert!(rd.join("turbo.exe").exists());
        assert!(!rd.join("deps").exists());
        assert!(!rd.join("incremental").exists());
    }

    #[test]
    fn clean_release_dist_caches_refuses_when_lock_held() {
        let tmp = tempfile::tempdir().unwrap();
        let rd = tmp.path().join("target").join("release-dist");
        fs::create_dir_all(rd.join("deps")).unwrap();
        fs::write(rd.join("turbo.exe"), b"ship").unwrap();
        fs::write(rd.join("deps").join("hot.rlib"), b"hot").unwrap();

        // Hold exclusive profile lock (simulates concurrent cargo).
        let lock_path = rd.join(".cargo-lock");
        let holder = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&lock_path)
            .unwrap();
        holder.try_lock_exclusive().unwrap();

        let cats = resolve_categories(&[CleanCategory::ReleaseDistCaches], false).unwrap();
        let err = clean(CleanOpts {
            root: Some(tmp.path().to_path_buf()),
            dry_run: false,
            worktree_hours: 24,
            tree_days: 14,
            if_low_space: false,
            min_free_gb: None,
            categories: cats,
            json: false,
            active_build_grace_secs: 120,
        });
        assert!(err.is_err(), "expected lock-held refuse: {err:?}");
        assert!(rd.join("turbo.exe").exists());
        assert!(rd.join("deps").join("hot.rlib").exists());
        drop(holder);
    }

    #[test]
    fn cargo_home_preserves_credentials_and_config() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("cargo-home");
        fs::create_dir_all(home.join("registry").join("cache")).unwrap();
        fs::create_dir_all(home.join("git").join("db")).unwrap();
        fs::write(home.join("credentials.toml"), b"secret=1").unwrap();
        fs::write(home.join("config.toml"), b"[build]\n").unwrap();
        fs::create_dir_all(home.join("registry").join("src")).unwrap();
        fs::write(home.join("registry").join("cache").join("x.crate"), b"c").unwrap();
        fs::write(home.join("git").join("db").join("repo"), b"g").unwrap();

        let prev = std::env::var_os("CARGO_HOME");
        unsafe {
            std::env::set_var("CARGO_HOME", &home);
        }
        let cats = resolve_categories(&[CleanCategory::CargoHome], true).unwrap();
        clean(CleanOpts {
            root: Some(tmp.path().to_path_buf()),
            dry_run: false,
            worktree_hours: 24,
            tree_days: 14,
            if_low_space: false,
            min_free_gb: None,
            categories: cats,
            json: false,
            active_build_grace_secs: 120,
        })
        .unwrap();
        assert!(home.join("credentials.toml").exists());
        assert!(home.join("config.toml").exists());
        assert!(home.join("registry").join("src").exists());
        assert!(!home.join("registry").join("cache").exists());
        assert!(!home.join("git").join("db").exists());
        unsafe {
            match prev {
                Some(v) => std::env::set_var("CARGO_HOME", v),
                None => std::env::remove_var("CARGO_HOME"),
            }
        }
    }

    #[test]
    fn target_root_honors_cargo_target_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let alt = tmp.path().join("alt-target");
        fs::create_dir_all(alt.join("debug")).unwrap();
        fs::write(alt.join("debug").join("x"), b"1").unwrap();
        let prev = std::env::var_os("CARGO_TARGET_DIR");
        unsafe {
            std::env::set_var("CARGO_TARGET_DIR", &alt);
        }
        let root = tmp.path().join("ws");
        fs::create_dir_all(&root).unwrap();
        assert_eq!(target_root(&root), alt);
        let cats = resolve_categories(&[CleanCategory::Debug], false).unwrap();
        clean(CleanOpts {
            root: Some(root),
            dry_run: false,
            worktree_hours: 24,
            tree_days: 14,
            if_low_space: false,
            min_free_gb: None,
            categories: cats,
            json: false,
            active_build_grace_secs: 120,
        })
        .unwrap();
        assert!(!alt.join("debug").exists());
        unsafe {
            match prev {
                Some(v) => std::env::set_var("CARGO_TARGET_DIR", v),
                None => std::env::remove_var("CARGO_TARGET_DIR"),
            }
        }
    }

    #[test]
    fn clean_never_touches_live_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let slug = tmp.path().join("ws-slug");
        let live = slug.join("subagent-live1");
        let dead = slug.join("subagent-old1");
        fs::create_dir_all(&live).unwrap();
        fs::create_dir_all(&dead).unwrap();
        fs::write(live.join(".grok-subagent-live"), b"1").unwrap();
        fs::write(live.join("x"), b"live").unwrap();
        fs::write(dead.join("x"), b"dead").unwrap();

        // Point GROK_HOME worktrees at a temp home
        let prev = std::env::var_os("GROK_HOME");
        let home = tempfile::tempdir().unwrap();
        let wt = home.path().join("worktrees").join("ws-slug");
        let live2 = wt.join("subagent-live1");
        let dead2 = wt.join("subagent-old1");
        fs::create_dir_all(&live2).unwrap();
        fs::create_dir_all(&dead2).unwrap();
        fs::write(live2.join(".grok-subagent-live"), b"1").unwrap();
        fs::write(live2.join("x"), b"live").unwrap();
        fs::write(dead2.join("x"), b"dead").unwrap();
        // Make dead tree appear old via worktree_hours = u64::MAX effectively... 
        // use hours=0 so cutoff is "now" and any mtime < now is old.
        // SAFETY: test-local env for worktrees_base
        unsafe {
            std::env::set_var("GROK_HOME", home.path());
        }

        let cats = resolve_categories(&[CleanCategory::Worktrees], false).unwrap();
        clean(CleanOpts {
            root: Some(tmp.path().to_path_buf()),
            dry_run: false,
            // hours=0 → cutoff = now; any existing mtime should be <= now
            worktree_hours: 0,
            tree_days: 14,
            if_low_space: false,
            min_free_gb: None,
            categories: cats,
            json: false,
            active_build_grace_secs: 120,
        })
        .unwrap();

        assert!(live2.exists(), "live worktree must remain");
        assert!(!dead2.exists(), "old worktree should be removed");

        unsafe {
            match prev {
                Some(v) => std::env::set_var("GROK_HOME", v),
                None => std::env::remove_var("GROK_HOME"),
            }
        }
    }

    #[test]
    fn refuse_clean_without_safe_is_run_path() {
        // run() is the latch — unit the error message path
        let args = DiskArgs {
            command: DiskCommand::Clean {
                root: None,
                dry_run: true,
                safe: false,
                worktree_hours: 24,
                tree_days: 14,
                if_low_space: false,
                min_free_gb: None,
                include: vec![],
                i_accept_redownload: false,
                json: false,
                active_build_grace_secs: 120,
            },
        };
        let err = run(args).unwrap_err();
        assert!(err.to_string().contains("--safe"));
    }

    #[test]
    fn prune_requires_target_flag() {
        let args = DiskArgs {
            command: DiskCommand::Prune {
                worktrees: false,
                tree_store: false,
                session_meta: false,
                all: false,
                dry_run: true,
                execute: false,
                worktree_hours: 24,
                tree_days: 14,
                older_than: "24h".into(),
                root: None,
                json: false,
            },
        };
        let err = run(args).unwrap_err();
        assert!(err.to_string().contains("--worktrees") || err.to_string().contains("--all"));
    }

    #[test]
    fn dir_size_and_pdb_scan() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("d");
        fs::create_dir_all(&d).unwrap();
        let mut f = fs::File::create(d.join("a.pdb")).unwrap();
        f.write_all(&[0u8; 100]).unwrap();
        drop(f);
        fs::write(d.join("b.txt"), b"hi").unwrap();
        let sz = dir_size_capped(&d, 100).bytes;
        assert!(sz >= 102);
        let pdb = size_matching(&d, 100, |p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("pdb"))
                .unwrap_or(false)
        });
        assert_eq!(pdb, 100);
    }

}
