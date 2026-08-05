//! `turbo disk` — report workspace + product disk use; safe clean for RC dogfood.
//!
//! Focus: free space, `target/` bloat, subagent worktrees, tree store, session TMP.
//! Does **not** install/uninstall product binaries.

use anyhow::{Result, bail};
use clap::Subcommand;
use std::path::{Path, PathBuf};

#[derive(Debug, clap::Args, Clone)]
pub struct DiskArgs {
    #[command(subcommand)]
    pub command: DiskCommand,
}

#[derive(Debug, Subcommand, Clone)]
pub enum DiskCommand {
    /// Summarize free space and large Turbo / Rust build directories
    Report {
        /// Workspace root (default: cwd)
        #[arg(long)]
        root: Option<PathBuf>,
        /// Emit JSON
        #[arg(long)]
        json: bool,
    },
    /// Remove safe caches only (debug target artifacts, old worktrees, tree store)
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
        /// Also prune subagent worktrees older than this many hours (default 24)
        #[arg(long, default_value_t = 24)]
        worktree_hours: u64,
        /// Also prune workspace-tree indexes older than this many days (default 14)
        #[arg(long, default_value_t = 14)]
        tree_days: u64,
    },
}

pub fn run(args: DiskArgs) -> Result<()> {
    match args.command {
        DiskCommand::Report { root, json } => report(root, json),
        DiskCommand::Clean {
            root,
            dry_run,
            safe,
            worktree_hours,
            tree_days,
        } => {
            if !safe {
                bail!("refusing clean without --safe (try: turbo disk clean --safe --dry-run)");
            }
            clean(root, dry_run, worktree_hours, tree_days)
        }
    }
}

fn report(root: Option<PathBuf>, json: bool) -> Result<()> {
    let root = root
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let root = dunce::canonicalize(&root).unwrap_or(root);

    let free = free_bytes_for_path(&root);
    let target = root.join("target");
    let target_debug = target.join("debug");
    let target_release = target.join("release");
    let target_rd = target.join("release-dist");
    let target_sz = dir_size_capped(&target, 2_000_000);
    let debug_sz = dir_size_capped(&target_debug, 1_000_000);
    let release_sz = dir_size_capped(&target_release, 1_000_000);
    let rd_sz = dir_size_capped(&target_rd, 500_000);

    let worktrees = worktrees_base();
    let wt_sz = dir_size_capped(&worktrees, 500_000);
    let wt_count = count_dirs(&worktrees);

    let tree_cfg = xai_workspace_tree::WorkspaceTreeConfig::from_env();
    let tree_store = xai_workspace_tree::store_root(&tree_cfg);
    let (tree_dirs, tree_usage) = xai_workspace_tree::store_disk_usage(&tree_cfg);

    let temp_grok = std::env::temp_dir().join("grok");
    let temp_sz = dir_size_capped(&temp_grok, 500_000);

    if json {
        let v = serde_json::json!({
            "root": root.display().to_string(),
            "free_bytes": free,
            "target_bytes": target_sz.bytes,
            "target_truncated": target_sz.truncated,
            "target_debug_bytes": debug_sz.bytes,
            "target_release_bytes": release_sz.bytes,
            "target_release_dist_bytes": rd_sz.bytes,
            "worktrees_path": worktrees.display().to_string(),
            "worktrees_bytes": wt_sz.bytes,
            "worktrees_dirs": wt_count,
            "tree_store_path": tree_store.display().to_string(),
            "tree_store_dirs": tree_dirs,
            "tree_store_bytes": tree_usage,
            "temp_grok_path": temp_grok.display().to_string(),
            "temp_grok_bytes": temp_sz.bytes,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }

    println!("Turbo disk report");
    println!("  root:           {}", root.display());
    println!(
        "  free space:     {}",
        free.map(fmt_bytes).unwrap_or_else(|| "unknown".into())
    );
    println!(
        "  target/:        {}{}",
        fmt_bytes(target_sz.bytes),
        if target_sz.truncated { " (scan capped)" } else { "" }
    );
    println!("    debug/:       {}", fmt_bytes(debug_sz.bytes));
    println!("    release/:     {}", fmt_bytes(release_sz.bytes));
    println!("    release-dist/ {}", fmt_bytes(rd_sz.bytes));
    println!(
        "  worktrees:      {} ({} dirs) @ {}",
        fmt_bytes(wt_sz.bytes),
        wt_count,
        worktrees.display()
    );
    println!(
        "  tree store:     {} ({} workspace dirs) @ {}",
        fmt_bytes(tree_usage),
        tree_dirs,
        tree_store.display()
    );
    println!(
        "  %TEMP%/grok:    {} @ {}",
        fmt_bytes(temp_sz.bytes),
        temp_grok.display()
    );
    println!();
    println!("Safe clean (dry-run first):");
    println!("  turbo disk clean --safe --dry-run");
    println!("  turbo subagent prune --older-than 24h");
    println!("  turbo tree prune --max-age-days 14");
    if free.map(|b| b < 40u64 * 1024 * 1024 * 1024).unwrap_or(false) {
        println!();
        println!("WARNING: free space under ~40 GB — prefer package-scoped cargo tests;");
        println!("  consider cargo clean -p … or removing target/debug after release-dist builds.");
    }
    Ok(())
}

fn clean(
    root: Option<PathBuf>,
    dry_run: bool,
    worktree_hours: u64,
    tree_days: u64,
) -> Result<()> {
    let root = root
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let root = dunce::canonicalize(&root).unwrap_or(root);

    let mut actions: Vec<String> = Vec::new();
    let mut freed: u64 = 0;

    // 1) target/debug only — never touch release-dist by default (ship artifact).
    let debug = root.join("target").join("debug");
    if debug.is_dir() {
        let sz = dir_size_capped(&debug, 1_000_000).bytes;
        actions.push(format!(
            "{} target/debug ({})",
            if dry_run { "would remove" } else { "remove" },
            fmt_bytes(sz)
        ));
        if !dry_run {
            let _ = std::fs::remove_dir_all(&debug);
            freed = freed.saturating_add(sz);
        }
    }

    // 2) Incremental caches under target (if present)
    for name in ["incremental", ".fingerprint"] {
        let p = root.join("target").join(name);
        if p.is_dir() {
            let sz = dir_size_capped(&p, 200_000).bytes;
            actions.push(format!(
                "{} target/{} ({})",
                if dry_run { "would remove" } else { "remove" },
                name,
                fmt_bytes(sz)
            ));
            if !dry_run {
                let _ = std::fs::remove_dir_all(&p);
                freed = freed.saturating_add(sz);
            }
        }
    }

    // 3) Subagent worktrees older than N hours (directory age by mtime)
    let wt_base = worktrees_base();
    if wt_base.is_dir() {
        let cutoff = std::time::SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(worktree_hours.saturating_mul(3600)))
            .unwrap_or(std::time::UNIX_EPOCH);
        if let Ok(rd) = std::fs::read_dir(&wt_base) {
            for entry in rd.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                // Nested slug dirs: walk one level of subagent-*
                if let Ok(inner) = std::fs::read_dir(&path) {
                    for child in inner.flatten() {
                        let cp = child.path();
                        let name = child.file_name().to_string_lossy().into_owned();
                        if !name.starts_with("subagent-") || !cp.is_dir() {
                            continue;
                        }
                        let old = child
                            .metadata()
                            .and_then(|m| m.modified())
                            .map(|t| t < cutoff)
                            .unwrap_or(false);
                        if !old {
                            continue;
                        }
                        let sz = dir_size_capped(&cp, 100_000).bytes;
                        actions.push(format!(
                            "{} worktree {} ({})",
                            if dry_run { "would remove" } else { "remove" },
                            cp.display(),
                            fmt_bytes(sz)
                        ));
                        if !dry_run {
                            let _ = std::fs::remove_dir_all(&cp);
                            freed = freed.saturating_add(sz);
                        }
                    }
                }
            }
        }
    }

    // 4) Workspace tree store prune
    let tree_cfg = xai_workspace_tree::WorkspaceTreeConfig::from_env();
    let tree_store = xai_workspace_tree::store_root(&tree_cfg);
    if tree_store.is_dir() {
        if dry_run {
            actions.push(format!(
                "would prune tree store older than {tree_days}d @ {}",
                tree_store.display()
            ));
        } else {
            let max_age = std::time::Duration::from_secs(tree_days.saturating_mul(24 * 3600));
            match xai_workspace_tree::prune_store(&tree_cfg, max_age, 0) {
                Ok(report) => {
                    actions.push(format!(
                        "tree prune: removed {} dirs, freed {} @ {}",
                        report.removed_dirs,
                        fmt_bytes(report.freed_bytes),
                        tree_store.display()
                    ));
                    freed = freed.saturating_add(report.freed_bytes);
                }
                Err(e) => actions.push(format!("tree prune skipped: {e}")),
            }
        }
    }

    println!(
        "turbo disk clean --safe{}",
        if dry_run { " --dry-run" } else { "" }
    );
    if actions.is_empty() {
        println!("  (nothing to clean)");
    } else {
        for a in &actions {
            println!("  {a}");
        }
    }
    if !dry_run {
        println!("  approx freed: {}", fmt_bytes(freed));
    } else {
        println!("  (dry-run; re-run without --dry-run to apply)");
    }
    Ok(())
}

fn worktrees_base() -> PathBuf {
    // Prefer product home then ~/.grok/worktrees
    if let Ok(home) = std::env::var("GROK_HOME") {
        let p = PathBuf::from(home).join("worktrees");
        if p.exists() {
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

fn free_bytes_for_path(path: &Path) -> Option<u64> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut free_caller = 0u64;
        let mut total = 0u64;
        let mut free = 0u64;
        // GetDiskFreeSpaceExW
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
            Some(free_caller)
        } else {
            None
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
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
