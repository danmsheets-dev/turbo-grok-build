//! `hyper subagent` — list / open / diff / land / discard / prune session subagents.
//!
//! Offline CLI over `~/.grok/sessions/<cwd-hash>/<session>/subagents/<id>/`.
//! Parity with the `diff_subagent` / `land_subagent` / `discard_subagent` tools
//! for operators who prefer the shell over tool calls.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use xai_grok_config::{encode_cwd_dirname, sessions_cwd_dir};

#[derive(Debug, clap::Args, Clone)]
pub struct SubagentArgs {
    #[command(subcommand)]
    pub command: SubagentCommand,
}

#[derive(Debug, Subcommand, Clone)]
pub enum SubagentCommand {
    /// List subagents for the current workspace (or a session id)
    List {
        /// Restrict to a single session id
        #[arg(long)]
        session: Option<String>,
        /// Emit JSON
        #[arg(long)]
        json: bool,
    },
    /// Print meta.json and recovery paths for a subagent
    Open {
        /// Subagent id
        id: String,
        /// Session id (required if the id is ambiguous across sessions)
        #[arg(long)]
        session: Option<String>,
    },
    /// Show unified diff of a subagent's work vs parent HEAD
    Diff {
        id: String,
        #[arg(long)]
        session: Option<String>,
    },
    /// Apply a subagent's work into the current git repo (merge fails closed)
    Land {
        id: String,
        #[arg(long)]
        session: Option<String>,
        /// `merge` (default) or `overwrite`
        #[arg(long, default_value = "merge")]
        mode: String,
    },
    /// Remove a live worktree; keep snapshot ref unless --drop-snapshot
    Discard {
        id: String,
        #[arg(long)]
        session: Option<String>,
        /// Also delete the durable snapshot ref from the parent repo
        #[arg(long)]
        drop_snapshot: bool,
    },
    /// Delete local subagent dirs older than the given duration (default 24h)
    Prune {
        /// Duration like `24h`, `7d`, or hours as a bare number
        #[arg(long = "older-than", default_value = "24h")]
        older_than: String,
        /// Actually delete (default is dry-run)
        #[arg(long)]
        execute: bool,
        /// Restrict to one session
        #[arg(long)]
        session: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MetaView {
    #[serde(default)]
    subagent_id: Option<String>,
    #[serde(default)]
    parent_session_id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    worktree_path: Option<String>,
    #[serde(default)]
    snapshot_ref: Option<String>,
    #[serde(default)]
    patch_path: Option<String>,
    #[serde(default)]
    worktree_state: Option<String>,
    #[serde(default)]
    land_status: Option<String>,
    #[serde(default)]
    child_cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ListedSubagent {
    id: String,
    session_id: String,
    status: Option<String>,
    worktree_state: Option<String>,
    land_status: Option<String>,
    snapshot_ref: Option<String>,
    patch_path: Option<String>,
    worktree_path: Option<String>,
    meta_path: String,
}

struct Resolved {
    id: String,
    session_id: String,
    dir: PathBuf,
    meta_path: PathBuf,
    meta: MetaView,
}

pub fn run(args: SubagentArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("current_dir")?;
    let cwd_str = cwd.to_string_lossy().into_owned();
    match args.command {
        SubagentCommand::List { session, json } => cmd_list(&cwd_str, session.as_deref(), json),
        SubagentCommand::Open { id, session } => {
            let r = resolve(&cwd_str, &id, session.as_deref())?;
            cmd_open(&r)
        }
        SubagentCommand::Diff { id, session } => {
            let r = resolve(&cwd_str, &id, session.as_deref())?;
            cmd_diff(&cwd, &r)
        }
        SubagentCommand::Land { id, session, mode } => {
            let r = resolve(&cwd_str, &id, session.as_deref())?;
            cmd_land(&cwd, &r, &mode)
        }
        SubagentCommand::Discard {
            id,
            session,
            drop_snapshot,
        } => {
            let r = resolve(&cwd_str, &id, session.as_deref())?;
            cmd_discard(&cwd, &r, drop_snapshot)
        }
        SubagentCommand::Prune {
            older_than,
            execute,
            session,
        } => cmd_prune(&cwd_str, session.as_deref(), &older_than, execute),
    }
}

fn sessions_root_for_cwd(cwd: &str) -> PathBuf {
    sessions_cwd_dir(cwd)
}

fn resolve(cwd: &str, id: &str, session: Option<&str>) -> Result<Resolved> {
    let id = id.trim();
    if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") {
        bail!("invalid subagent id `{id}`");
    }
    let root = sessions_root_for_cwd(cwd);
    if !root.is_dir() {
        bail!(
            "no sessions directory for this cwd ({}); has hyper ever been run here?",
            root.display()
        );
    }

    let mut hits: Vec<Resolved> = Vec::new();
    let session_dirs: Vec<PathBuf> = if let Some(sid) = session {
        vec![root.join(sid)]
    } else {
        fs::read_dir(&root)
            .with_context(|| format!("read {}", root.display()))?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.path())
            .collect()
    };

    for session_dir in session_dirs {
        let sid = session_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let meta_path = session_dir.join("subagents").join(id).join("meta.json");
        if !meta_path.is_file() {
            continue;
        }
        let raw = fs::read_to_string(&meta_path)
            .with_context(|| format!("read {}", meta_path.display()))?;
        let meta: MetaView = serde_json::from_str(&raw)
            .with_context(|| format!("parse {}", meta_path.display()))?;
        hits.push(Resolved {
            id: id.to_string(),
            session_id: sid,
            dir: meta_path.parent().unwrap().to_path_buf(),
            meta_path,
            meta,
        });
    }

    match hits.len() {
        0 => bail!(
            "subagent `{id}` not found under {} (pass --session <id> if known)",
            root.display()
        ),
        1 => Ok(hits.remove(0)),
        n => {
            let sessions: Vec<_> = hits.iter().map(|h| h.session_id.as_str()).collect();
            bail!(
                "subagent `{id}` found in {n} sessions: {}; pass --session <id>",
                sessions.join(", ")
            );
        }
    }
}

fn list_entries(cwd: &str, session: Option<&str>) -> Result<Vec<ListedSubagent>> {
    let root = sessions_root_for_cwd(cwd);
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let session_dirs: Vec<PathBuf> = if let Some(sid) = session {
        vec![root.join(sid)]
    } else {
        fs::read_dir(&root)
            .with_context(|| format!("read {}", root.display()))?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.path())
            .collect()
    };

    let mut out = Vec::new();
    for session_dir in session_dirs {
        let sid = session_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let sub_root = session_dir.join("subagents");
        if !sub_root.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&sub_root).with_context(|| format!("read {}", sub_root.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            let meta_path = entry.path().join("meta.json");
            if !meta_path.is_file() {
                continue;
            }
            let raw = fs::read_to_string(&meta_path).unwrap_or_default();
            let meta: MetaView = serde_json::from_str(&raw).unwrap_or(MetaView {
                subagent_id: Some(id.clone()),
                parent_session_id: None,
                status: None,
                worktree_path: None,
                snapshot_ref: None,
                patch_path: None,
                worktree_state: None,
                land_status: None,
                child_cwd: None,
            });
            let patch = meta.patch_path.clone().or_else(|| {
                let p = entry.path().join("changes.patch");
                p.is_file().then(|| p.display().to_string())
            });
            out.push(ListedSubagent {
                id: meta.subagent_id.clone().unwrap_or(id),
                session_id: sid.clone(),
                status: meta.status,
                worktree_state: meta.worktree_state,
                land_status: meta.land_status,
                snapshot_ref: meta.snapshot_ref,
                patch_path: patch,
                worktree_path: meta.worktree_path,
                meta_path: meta_path.display().to_string(),
            });
        }
    }
    out.sort_by(|a, b| a.session_id.cmp(&b.session_id).then(a.id.cmp(&b.id)));
    Ok(out)
}

fn cmd_list(cwd: &str, session: Option<&str>, json: bool) -> Result<()> {
    let entries = list_entries(cwd, session)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }
    if entries.is_empty() {
        println!(
            "No subagents found under sessions for cwd hash `{}`.",
            encode_cwd_dirname(cwd)
        );
        println!("Hint: run agents with isolation=worktree, then re-list.");
        return Ok(());
    }
    println!(
        "{:<38} {:<12} {:<10} {:<10} {}",
        "ID", "STATUS", "WT_STATE", "LAND", "SESSION"
    );
    for e in &entries {
        println!(
            "{:<38} {:<12} {:<10} {:<10} {}",
            truncate(&e.id, 38),
            e.status.as_deref().unwrap_or("-"),
            e.worktree_state.as_deref().unwrap_or("-"),
            e.land_status.as_deref().unwrap_or("-"),
            truncate(&e.session_id, 36)
        );
    }
    println!("\n{} subagent(s). Use `hyper subagent open <id>`.", entries.len());
    Ok(())
}

fn cmd_open(r: &Resolved) -> Result<()> {
    println!("subagent_id:  {}", r.id);
    println!("session_id:   {}", r.session_id);
    println!("meta_path:    {}", r.meta_path.display());
    println!("status:       {}", r.meta.status.as_deref().unwrap_or("-"));
    println!(
        "worktree_state: {}",
        r.meta.worktree_state.as_deref().unwrap_or("-")
    );
    println!(
        "land_status:  {}",
        r.meta.land_status.as_deref().unwrap_or("-")
    );
    if let Some(ref p) = r.meta.worktree_path {
        let live = Path::new(p).is_dir();
        println!("worktree_path: {p} ({})", if live { "live" } else { "gone" });
    }
    if let Some(ref s) = r.meta.snapshot_ref {
        println!("snapshot_ref: {s}");
        println!("  recover: git show {s}:<path>   or   git diff HEAD {s}");
    }
    let patch = r.meta.patch_path.as_deref().map(PathBuf::from).or_else(|| {
        let p = r.dir.join("changes.patch");
        p.is_file().then_some(p)
    });
    if let Some(p) = patch {
        println!("patch_path:   {}", p.display());
    }
    Ok(())
}

fn cmd_diff(cwd: &Path, r: &Resolved) -> Result<()> {
    // Prefer live worktree, then snapshot_ref, then changes.patch
    if let Some(ref wt) = r.meta.worktree_path {
        let wt = Path::new(wt);
        if wt.is_dir() {
            let out = git(cwd, &["diff", "--no-ext-diff", "HEAD", "--", &wt.display().to_string()])?;
            // worktree may be outside repo — fall back to -C worktree
            if out.trim().is_empty() {
                let out = git(wt, &["diff", "--no-ext-diff", "HEAD"])?;
                print_diff("live_worktree", &out);
                return Ok(());
            }
            print_diff("live_worktree", &out);
            return Ok(());
        }
    }
    if let Some(ref snap) = r.meta.snapshot_ref {
        let out = git(cwd, &["diff", "--no-ext-diff", "HEAD", snap])?;
        print_diff("snapshot_ref", &out);
        return Ok(());
    }
    let patch = r.meta.patch_path.as_deref().map(PathBuf::from).or_else(|| {
        let p = r.dir.join("changes.patch");
        p.is_file().then_some(p)
    });
    if let Some(p) = patch {
        let text = fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
        print_diff("patch", &text);
        return Ok(());
    }
    bail!("no live worktree, snapshot_ref, or changes.patch for {}", r.id);
}

fn print_diff(source: &str, text: &str) {
    println!("# source: {source}");
    if text.trim().is_empty() {
        println!("(empty diff)");
    } else {
        print!("{text}");
        if !text.ends_with('\n') {
            println!();
        }
    }
}

fn cmd_land(cwd: &Path, r: &Resolved, mode: &str) -> Result<()> {
    let mode = mode.trim().to_ascii_lowercase();
    if mode != "merge" && mode != "overwrite" {
        bail!("mode must be `merge` or `overwrite`");
    }

    // 1) Live worktree: three-way style via git apply of worktree diff is complex;
    //    use `git -C worktree diff` piped into apply when merge, or checkout files.
    if let Some(ref wt) = r.meta.worktree_path {
        let wt = Path::new(wt);
        if wt.is_dir() {
            return land_from_worktree(cwd, wt, &mode, r);
        }
    }
    // 2) Snapshot ref
    if let Some(ref snap) = r.meta.snapshot_ref {
        return land_from_snapshot(cwd, snap, &mode, r);
    }
    // 3) Patch
    let patch = r.meta.patch_path.as_deref().map(PathBuf::from).or_else(|| {
        let p = r.dir.join("changes.patch");
        p.is_file().then_some(p)
    });
    if let Some(p) = patch {
        return land_from_patch(cwd, &p, &mode, r);
    }
    bail!("nothing to land for {}", r.id);
}

fn land_from_worktree(cwd: &Path, wt: &Path, mode: &str, r: &Resolved) -> Result<()> {
    let diff = git(wt, &["diff", "--binary", "HEAD"])?;
    if diff.trim().is_empty() {
        // include untracked via apply of full tree is out of scope; try name-status
        println!("No tracked diff in live worktree (untracked files are not auto-landed).");
        update_land_status(&r.meta_path, "landed_empty")?;
        return Ok(());
    }
    apply_diff_text(cwd, &diff, mode)?;
    update_land_status(&r.meta_path, "landed")?;
    println!("Landed live worktree for {} (mode={mode}).", r.id);
    Ok(())
}

fn land_from_snapshot(cwd: &Path, snap: &str, mode: &str, r: &Resolved) -> Result<()> {
    let diff = git(cwd, &["diff", "--binary", "HEAD", snap])?;
    if diff.trim().is_empty() {
        println!("Snapshot `{snap}` matches HEAD (nothing to land).");
        update_land_status(&r.meta_path, "landed_empty")?;
        return Ok(());
    }
    if mode == "merge" {
        // Fail closed: if apply --check fails, abort.
        apply_diff_text(cwd, &diff, "merge")?;
    } else {
        apply_diff_text(cwd, &diff, "overwrite")?;
    }
    update_land_status(&r.meta_path, "landed")?;
    println!("Landed snapshot_ref `{snap}` for {} (mode={mode}).", r.id);
    Ok(())
}

fn land_from_patch(cwd: &Path, patch: &Path, mode: &str, r: &Resolved) -> Result<()> {
    let text = fs::read_to_string(patch).with_context(|| format!("read {}", patch.display()))?;
    apply_diff_text(cwd, &text, mode)?;
    update_land_status(&r.meta_path, "landed")?;
    println!(
        "Landed patch {} for {} (mode={mode}).",
        patch.display(),
        r.id
    );
    Ok(())
}

fn apply_diff_text(cwd: &Path, diff: &str, mode: &str) -> Result<()> {
    if mode == "merge" {
        // Check first — fail closed
        let status = Command::new("git")
            .args(["apply", "--check", "--3way", "-"])
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                if let Some(mut stdin) = child.stdin.take() {
                    stdin.write_all(diff.as_bytes())?;
                }
                child.wait_with_output()
            })
            .context("git apply --check")?;
        if !status.status.success() {
            let err = String::from_utf8_lossy(&status.stderr);
            bail!(
                "merge land would conflict (nothing applied):\n{}",
                err.trim()
            );
        }
    }
    let mut args = vec!["apply"];
    if mode == "merge" {
        args.push("--3way");
    } else {
        // overwrite: force apply, reject binary safety still via git
        args.push("--reject");
    }
    args.push("-");
    let status = Command::new("git")
        .args(&args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(diff.as_bytes())?;
            }
            child.wait_with_output()
        })
        .context("git apply")?;
    if !status.status.success() {
        let err = String::from_utf8_lossy(&status.stderr);
        bail!("git apply failed:\n{}", err.trim());
    }
    Ok(())
}

fn cmd_discard(cwd: &Path, r: &Resolved, drop_snapshot: bool) -> Result<()> {
    let mut removed = false;
    if let Some(ref wt) = r.meta.worktree_path {
        let wt = Path::new(wt);
        if wt.is_dir() {
            fs::remove_dir_all(wt).with_context(|| format!("remove {}", wt.display()))?;
            removed = true;
            println!("Removed worktree {}", wt.display());
        }
    }
    let mut snapshot_dropped = false;
    if drop_snapshot {
        if let Some(ref snap) = r.meta.snapshot_ref {
            let _ = git(cwd, &["update-ref", "-d", snap]);
            snapshot_dropped = true;
            println!("Dropped snapshot ref {snap}");
        }
    } else if let Some(ref snap) = r.meta.snapshot_ref {
        println!("Kept snapshot_ref {snap} (pass --drop-snapshot to delete)");
    }
    // Update meta
    if let Ok(raw) = fs::read_to_string(&r.meta_path) {
        if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(obj) = v.as_object_mut() {
                obj.insert(
                    "land_status".into(),
                    serde_json::Value::String("discarded".into()),
                );
                if removed {
                    obj.insert("worktree_path".into(), serde_json::Value::Null);
                    obj.insert(
                        "worktree_state".into(),
                        serde_json::Value::String("cleaned".into()),
                    );
                }
                if snapshot_dropped {
                    obj.insert("snapshot_ref".into(), serde_json::Value::Null);
                }
                let _ = fs::write(&r.meta_path, serde_json::to_string_pretty(&v)?);
            }
        }
    }
    println!(
        "Discarded {} (worktree_removed={removed}, snapshot_dropped={snapshot_dropped}).",
        r.id
    );
    Ok(())
}

fn cmd_prune(cwd: &str, session: Option<&str>, older_than: &str, execute: bool) -> Result<()> {
    let max_age = parse_duration(older_than)?;
    let cutoff = SystemTime::now()
        .checked_sub(max_age)
        .context("cutoff underflow")?;
    let entries = list_entries(cwd, session)?;
    let mut candidates = Vec::new();
    for e in entries {
        let meta = PathBuf::from(&e.meta_path);
        let modified = fs::metadata(&meta)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if modified < cutoff {
            candidates.push((e, meta.parent().map(|p| p.to_path_buf())));
        }
    }
    if candidates.is_empty() {
        println!("Nothing older than {older_than} to prune.");
        return Ok(());
    }
    println!(
        "{} subagent dir(s) older than {older_than}:",
        candidates.len()
    );
    for (e, dir) in &candidates {
        println!(
            "  {}  session={}  path={}",
            e.id,
            e.session_id,
            dir.as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        );
    }
    if !execute {
        println!("Dry-run only. Re-run with --execute to delete.");
        return Ok(());
    }
    for (e, dir) in candidates {
        if let Some(dir) = dir {
            match fs::remove_dir_all(&dir) {
                Ok(()) => println!("  deleted {}", dir.display()),
                Err(err) => eprintln!("  failed {}: {err}", dir.display()),
            }
        } else {
            eprintln!("  skip {} (no dir)", e.id);
        }
    }
    Ok(())
}

fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim().to_ascii_lowercase();
    if let Some(num) = s.strip_suffix('h') {
        let n: u64 = num.parse().context("hours")?;
        return Ok(Duration::from_secs(n.saturating_mul(3600)));
    }
    if let Some(num) = s.strip_suffix('d') {
        let n: u64 = num.parse().context("days")?;
        return Ok(Duration::from_secs(n.saturating_mul(86400)));
    }
    if let Some(num) = s.strip_suffix('m') {
        let n: u64 = num.parse().context("minutes")?;
        return Ok(Duration::from_secs(n.saturating_mul(60)));
    }
    // bare number = hours
    let n: u64 = s.parse().context("duration (e.g. 24h, 7d, or hours)")?;
    Ok(Duration::from_secs(n.saturating_mul(3600)))
}

fn update_land_status(meta_path: &Path, status: &str) -> Result<()> {
    let raw = fs::read_to_string(meta_path).unwrap_or_else(|_| "{}".into());
    let mut v: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::json!({}));
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "land_status".into(),
            serde_json::Value::String(status.into()),
        );
    }
    fs::write(meta_path, serde_json::to_string_pretty(&v)?)?;
    Ok(())
}

fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("git {}", args.join(" ")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("git {} failed: {}", args.join(" "), err.trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_accepts_units() {
        assert_eq!(parse_duration("24h").unwrap(), Duration::from_secs(24 * 3600));
        assert_eq!(parse_duration("7d").unwrap(), Duration::from_secs(7 * 86400));
        assert_eq!(parse_duration("90").unwrap(), Duration::from_secs(90 * 3600));
    }
}
