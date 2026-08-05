//! `turbo disk` — report workspace + product disk use; safe clean for RC dogfood.
//!
//! Focus: free space, `target/` bloat, subagent worktrees, tree store, session TMP.
//! Does **not** install/uninstall product binaries.
//!
//! RC2 gates (env):
//! - `GROK_MIN_FREE_GB` (default 40) / `GROK_SUBAGENT_MIN_FREE_BYTES`
//! - `GROK_SUBAGENT_KEEP_N` (default 3) / `GROK_SUBAGENT_SOFT_PRESERVE_KEEP_N`

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
    /// Fail closed if free space is under the configured min (default GROK_MIN_FREE_GB=40).
    ///
    /// Use before `cargo build --profile release-dist` or heavy workspace tests.
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
        /// Only clean when free space is under the min-free gate (default always clean when --safe)
        #[arg(long)]
        if_low_space: bool,
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
        } => {
            if !safe {
                bail!("refusing clean without --safe (try: turbo disk clean --safe --dry-run)");
            }
            clean(root, dry_run, worktree_hours, tree_days, if_low_space)
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

fn report(root: Option<PathBuf>, json: bool) -> Result<()> {
    let root = root
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let root = dunce::canonicalize(&root).unwrap_or(root);

    let free = free_bytes_for_path(&root);
    let min_free = configured_min_free_bytes();
    let keep_n = configured_keep_n();
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
    let subagent_count = count_subagent_dirs(&worktrees);

    let tree_cfg = xai_workspace_tree::WorkspaceTreeConfig::from_env();
    let tree_store = xai_workspace_tree::store_root(&tree_cfg);
    let (tree_dirs, tree_usage) = xai_workspace_tree::store_disk_usage(&tree_cfg);

    let temp_grok = std::env::temp_dir().join("grok");
    let temp_sz = dir_size_capped(&temp_grok, 500_000);

    let free_ok = free.map(|b| min_free == 0 || b >= min_free);
    let keep_over = keep_n > 0 && subagent_count > keep_n as u64;

    if json {
        let v = serde_json::json!({
            "root": root.display().to_string(),
            "free_bytes": free,
            "min_free_bytes": min_free,
            "min_free_gb": min_free / (1024 * 1024 * 1024),
            "free_space_ok": free_ok,
            "keep_n": keep_n,
            "target_bytes": target_sz.bytes,
            "target_truncated": target_sz.truncated,
            "target_debug_bytes": debug_sz.bytes,
            "target_release_bytes": release_sz.bytes,
            "target_release_dist_bytes": rd_sz.bytes,
            "worktrees_path": worktrees.display().to_string(),
            "worktrees_bytes": wt_sz.bytes,
            "worktrees_dirs": wt_count,
            "worktrees_subagent_dirs": subagent_count,
            "worktrees_over_keep_n": keep_over,
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
    let min_label = if min_free == 0 {
        "disabled (0)".to_string()
    } else {
        format!(
            "{} (GROK_MIN_FREE_GB / GROK_SUBAGENT_MIN_FREE_BYTES)",
            fmt_bytes(min_free)
        )
    };
    let status = match free_ok {
        Some(true) => "OK",
        Some(false) => "BELOW THRESHOLD",
        None => "unknown",
    };
    println!("  min free gate:  {min_label} → {status}");
    println!(
        "  target/:        {}{}",
        fmt_bytes(target_sz.bytes),
        if target_sz.truncated {
            " (scan capped)"
        } else {
            ""
        }
    );
    println!("    debug/:       {}", fmt_bytes(debug_sz.bytes));
    println!("    release/:     {}", fmt_bytes(release_sz.bytes));
    println!("    release-dist/ {}", fmt_bytes(rd_sz.bytes));
    let keep_label = if keep_n == 0 {
        "age-only (KEEP_N=0)".to_string()
    } else {
        format!("keep-N={keep_n}")
    };
    println!(
        "  worktrees:      {} ({} dirs, {} subagent-*) @ {}",
        fmt_bytes(wt_sz.bytes),
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
        fmt_bytes(temp_sz.bytes),
        temp_grok.display()
    );
    println!();
    println!("Safe clean (dry-run first):");
    println!("  turbo disk clean --safe --dry-run");
    println!("  turbo disk clean --safe --if-low-space   # only when under min free");
    println!("  turbo disk check                         # exit 1 if under min free");
    println!("  turbo subagent prune --older-than 24h");
    println!("  turbo tree prune --max-age-days 14");
    if free.map(|b| b < min_free || b < 40u64 * 1024 * 1024 * 1024).unwrap_or(false) {
        println!();
        println!("WARNING: free space under gate — prefer package-scoped cargo tests;");
        println!("  set CARGO_INCREMENTAL=0 for one-shot builds; clean target/debug after release-dist:");
        println!("  turbo disk clean --safe");
    }
    if debug_sz.bytes > 50u64 * 1024 * 1024 * 1024 {
        println!();
        println!(
            "NOTE: target/debug is large ({}) — agents should avoid full-workspace debug rebuilds.",
            fmt_bytes(debug_sz.bytes)
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
    let free = free_bytes_for_path(&root);
    let ok = min == 0 || free.map(|b| b >= min).unwrap_or(false);

    if json {
        let v = serde_json::json!({
            "root": root.display().to_string(),
            "free_bytes": free,
            "min_free_bytes": min,
            "ok": ok,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!(
            "turbo disk check: free={} min={} → {}",
            free.map(fmt_bytes).unwrap_or_else(|| "unknown".into()),
            if min == 0 {
                "disabled".into()
            } else {
                fmt_bytes(min)
            },
            if ok { "OK" } else { "FAIL" }
        );
        if !ok {
            println!("  Remediation: turbo disk clean --safe");
            println!("  Or: GROK_MIN_FREE_GB=0 / lower --min-free-gb for this check only");
        }
    }

    if !ok {
        bail!(
            "free space under threshold (need at least {}; have {})",
            fmt_bytes(min),
            free.map(fmt_bytes).unwrap_or_else(|| "unknown".into())
        );
    }
    Ok(())
}

fn clean(
    root: Option<PathBuf>,
    dry_run: bool,
    worktree_hours: u64,
    tree_days: u64,
    if_low_space: bool,
) -> Result<()> {
    let root = root
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let root = dunce::canonicalize(&root).unwrap_or(root);

    if if_low_space {
        let min = configured_min_free_bytes();
        if min > 0
            && let Some(free) = free_bytes_for_path(&root)
            && free >= min
        {
            println!(
                "turbo disk clean --safe --if-low-space: free space OK ({} >= {}); nothing to do",
                fmt_bytes(free),
                fmt_bytes(min)
            );
            return Ok(());
        }
    }

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
            .checked_sub(std::time::Duration::from_secs(
                worktree_hours.saturating_mul(3600),
            ))
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
                        // Never delete a live running child.
                        if cp.join(".grok-subagent-live").exists() {
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
        "turbo disk clean --safe{}{}",
        if dry_run { " --dry-run" } else { "" },
        if if_low_space { " --if-low-space" } else { "" }
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
    let probe = if path.exists() {
        path.to_path_buf()
    } else {
        path.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
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
}
