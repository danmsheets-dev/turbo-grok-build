//! `turbo pr` / `turbo pipeline` — GitHub PR + CI control plane wrapping `gh`.
//!
//! Scoped to the current git remote (`origin` → `owner/repo` via `--repo`).
//! Never merges. `pr create` is draft unless the human passes `--open`.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use serde::Serialize;
use wait_timeout::ChildExt;

const GH_TIMEOUT: Duration = Duration::from_secs(30);
const GH_OUTPUT_CAP: usize = 60_000;
const PR_STATUS_JSON_FIELDS: &str =
    "number,title,state,isDraft,statusCheckRollup,reviewDecision,url";
const CI_RUN_JSON_FIELDS: &str = "databaseId,name,status,conclusion,headBranch,event,url";
const CI_RUN_VIEW_JSON_FIELDS: &str = "databaseId,name,status,conclusion,headBranch,event,url,jobs";

// ---------------------------------------------------------------------------
// CLI args
// ---------------------------------------------------------------------------

#[derive(Debug, clap::Args, Clone)]
pub struct PrArgs {
    #[command(subcommand)]
    pub command: PrCommand,
}

#[derive(Debug, Subcommand, Clone)]
pub enum PrCommand {
    /// Show PR status for the current branch (or a given PR)
    Status {
        /// PR number or URL. Omit to detect the current branch's PR.
        pr: Option<String>,
        /// Emit JSON
        #[arg(long)]
        json: bool,
    },
    /// Open a **draft** PR from the current branch (wraps `gh pr create --draft`)
    ///
    /// Pass `--open` to create a ready-for-review PR. That flag is the human
    /// approval to open; Turbo never auto-opens or auto-merges.
    Create {
        /// PR title
        #[arg(long)]
        title: String,
        /// PR body (defaults to the title)
        #[arg(long)]
        body: Option<String>,
        /// Base branch (default: the repository default branch)
        #[arg(long)]
        base: Option<String>,
        /// Create ready-for-review instead of a draft (human approval)
        #[arg(long)]
        open: bool,
        /// Emit JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, clap::Args, Clone)]
pub struct PipelineArgs {
    #[command(subcommand)]
    pub command: PipelineCommand,
}

#[derive(Debug, Subcommand, Clone)]
pub enum PipelineCommand {
    /// Show the latest GitHub Actions run on this branch (or a given run id)
    Status {
        /// Run ID. Omit to use the latest run on the current branch.
        run: Option<String>,
        /// Emit JSON
        #[arg(long)]
        json: bool,
    },
    /// Rerun a GitHub Actions workflow (`gh run rerun`). Does not merge.
    Rerun {
        /// Run ID to rerun
        run: String,
        /// Only rerun failed jobs
        #[arg(long)]
        failed_only: bool,
        /// Emit JSON
        #[arg(long)]
        json: bool,
    },
}

// ---------------------------------------------------------------------------
// Host (live `gh`/`git` or a test mock)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Captured {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

trait GhHost {
    fn git(&self, argv: &[String]) -> Result<Captured>;
    fn gh(&self, argv: &[String]) -> Result<Captured>;
}

struct LiveHost;

impl GhHost for LiveHost {
    fn git(&self, argv: &[String]) -> Result<Captured> {
        run_bin("git", argv, GH_TIMEOUT)
    }

    fn gh(&self, argv: &[String]) -> Result<Captured> {
        run_bin("gh", argv, GH_TIMEOUT)
    }
}

fn run_bin(bin: &str, argv: &[String], timeout: Duration) -> Result<Captured> {
    let mut cmd = Command::new(bin);
    cmd.args(argv)
        .env("GH_NO_BROWSER", "1")
        .env("CI", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    xai_tty_utils::detach_std_command(&mut cmd);

    #[allow(clippy::disallowed_methods)] // waited with timeout; not an unbounded spawn
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if bin == "gh" {
                bail!(
                    "GitHub CLI (`gh`) is not installed or not on PATH. \
                     Install it from https://cli.github.com/ then run `gh auth login`."
                );
            }
            bail!("`{bin}` is not installed or not on PATH ({e}).");
        }
        Err(e) => bail!("Failed to spawn `{bin}`: {e}"),
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let t_out = std::thread::spawn(move || read_capped(stdout, GH_OUTPUT_CAP));
    let t_err = std::thread::spawn(move || read_capped(stderr, GH_OUTPUT_CAP));

    let status = match child.wait_timeout(timeout) {
        Ok(Some(st)) => st,
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            bail!("`{bin}` timed out after {}s.", timeout.as_secs());
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Failed to wait for `{bin}`: {e}");
        }
    };

    let stdout = String::from_utf8_lossy(&t_out.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&t_err.join().unwrap_or_default()).into_owned();
    Ok(Captured {
        stdout,
        stderr,
        exit_code: status.code().unwrap_or(-1),
    })
}

fn read_capped<R: Read>(stream: Option<R>, cap: usize) -> Vec<u8> {
    let Some(mut r) = stream else {
        return Vec::new();
    };
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match r.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let remaining = cap.saturating_sub(buf.len());
                if remaining == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n.min(remaining)]);
            }
            Err(_) => break,
        }
    }
    buf
}

// ---------------------------------------------------------------------------
// Remote scope (current git origin only)
// ---------------------------------------------------------------------------

fn parse_github_owner_repo(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let normalized = url.replace('\\', "/");
    let rest = if let Some(rest) = normalized.strip_prefix("git@github.com:") {
        rest
    } else if let Some(idx) = normalized.find("github.com/") {
        &normalized[idx + "github.com/".len()..]
    } else if let Some(idx) = normalized.find("github.com:") {
        &normalized[idx + "github.com:".len()..]
    } else {
        return None;
    };
    let mut parts = rest.split('/');
    let owner = parts.next()?.trim();
    let repo = parts.next()?.trim();
    if owner.is_empty() || repo.is_empty() || owner.contains(':') {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

fn inject_repo_flag(argv: &mut Vec<String>, repo: &str) {
    if argv.iter().any(|a| a == "--repo" || a == "-R") {
        return;
    }
    if argv.is_empty() {
        return;
    }
    argv.insert(1, "--repo".to_string());
    argv.insert(2, repo.to_string());
}

fn validate_user_token<'a>(value: &'a str, label: &str) -> Result<&'a str> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('-') {
        bail!("{label} is required and must not start with `-`.");
    }
    Ok(value)
}

fn current_origin_repo(host: &dyn GhHost) -> Result<String> {
    let out = host.git(&["remote".into(), "get-url".into(), "origin".into()])?;
    if out.exit_code != 0 {
        bail!(
            "Could not read git remote `origin`. Scope is the current GitHub remote only. \
             Detail: {}",
            out.stderr.trim()
        );
    }
    let url = out.stdout.trim();
    parse_github_owner_repo(url).ok_or_else(|| {
        anyhow::anyhow!(
            "git remote `origin` is not a GitHub URL (`{url}`). \
             `turbo pr` / `turbo pipeline` only target GitHub via `gh`."
        )
    })
}

fn current_git_branch(host: &dyn GhHost) -> Option<String> {
    let out = host
        .git(&["rev-parse".into(), "--abbrev-ref".into(), "HEAD".into()])
        .ok()?;
    if out.exit_code != 0 {
        return None;
    }
    let branch = out.stdout.trim();
    (!branch.is_empty() && branch != "HEAD").then(|| branch.to_string())
}

fn classify_gh_error(stderr: &str, fallback: &str) -> anyhow::Error {
    let msg = stderr.trim();
    if msg.contains("authentication") || msg.contains("auth") || msg.contains("token") {
        anyhow::anyhow!(
            "`gh` auth failed. Run `gh auth login` to authenticate with GitHub, then retry. \
             Detail: {msg}"
        )
    } else if msg.is_empty() {
        anyhow::anyhow!("{fallback}")
    } else {
        anyhow::anyhow!("{fallback} Detail: {msg}")
    }
}

fn json_id(value: Option<&serde_json::Value>) -> Option<String> {
    value.and_then(|v| {
        v.as_str()
            .map(str::to_owned)
            .or_else(|| v.as_u64().map(|n| n.to_string()))
    })
}

fn json_str(value: Option<&serde_json::Value>) -> Option<String> {
    value.and_then(|v| v.as_str()).map(str::to_owned)
}

// ---------------------------------------------------------------------------
// Argv builders
// ---------------------------------------------------------------------------

fn build_pr_status_argv(pr: Option<&str>) -> Result<Vec<String>> {
    let mut argv = vec![
        "pr".into(),
        "view".into(),
        "--json".into(),
        PR_STATUS_JSON_FIELDS.into(),
    ];
    if let Some(pr) = pr {
        let pr = validate_user_token(pr, "PR identifier")?;
        argv.extend(["--".into(), pr.to_string()]);
    }
    Ok(argv)
}

fn build_pr_create_argv(
    title: &str,
    body: &str,
    base: Option<&str>,
    head: Option<&str>,
    open: bool,
) -> Result<Vec<String>> {
    let title = validate_user_token(title, "PR title")?;
    if body.trim_start().starts_with('-') {
        bail!("PR body must not start with `-`.");
    }
    let mut argv = vec!["pr".into(), "create".into()];
    if !open {
        argv.push("--draft".into());
    }
    argv.extend([
        "--title".into(),
        title.to_string(),
        "--body".into(),
        body.to_string(),
    ]);
    if let Some(base) = base {
        let base = validate_user_token(base, "Base branch")?;
        argv.extend(["--base".into(), base.to_string()]);
    }
    if let Some(head) = head {
        let head = validate_user_token(head, "Head branch")?;
        argv.extend(["--head".into(), head.to_string()]);
    }
    Ok(argv)
}

fn build_ci_run_list_argv(branch: Option<&str>) -> Vec<String> {
    let mut argv = vec!["run".into(), "list".into(), "--limit".into(), "1".into()];
    if let Some(branch) = branch {
        argv.extend(["--branch".into(), branch.to_string()]);
    }
    argv.extend(["--json".into(), CI_RUN_JSON_FIELDS.into()]);
    argv
}

fn build_ci_run_view_argv(run_id: &str) -> Result<Vec<String>> {
    let run_id = validate_user_token(run_id, "Run ID")?;
    Ok(vec![
        "run".into(),
        "view".into(),
        "--json".into(),
        CI_RUN_VIEW_JSON_FIELDS.into(),
        "--".into(),
        run_id.to_string(),
    ])
}

fn build_ci_rerun_argv(run_id: &str, failed_only: bool) -> Result<Vec<String>> {
    let run_id = validate_user_token(run_id, "Run ID")?;
    let mut argv = vec!["run".into(), "rerun".into()];
    if failed_only {
        argv.push("--failed".into());
    }
    argv.extend(["--".into(), run_id.to_string()]);
    Ok(argv)
}

fn scoped_gh(host: &dyn GhHost, mut argv: Vec<String>, repo: &str) -> Result<Captured> {
    inject_repo_flag(&mut argv, repo);
    let out = host.gh(&argv)?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// PR status / create
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct PrStatusReport {
    remote: String,
    pr: Option<String>,
    title: Option<String>,
    state: Option<String>,
    is_draft: bool,
    url: Option<String>,
    review_decision: Option<String>,
    checks_pass: usize,
    checks_fail: usize,
    checks_pending: usize,
    failing_checks: Vec<String>,
}

fn tally_checks(
    status_check_rollup: Option<&serde_json::Value>,
) -> (usize, usize, usize, Vec<String>) {
    let mut pass = 0;
    let mut fail = 0;
    let mut pending = 0;
    let mut failing = Vec::new();
    let Some(arr) = status_check_rollup.and_then(|v| v.as_array()) else {
        return (pass, fail, pending, failing);
    };
    for entry in arr {
        let state = entry
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("UNKNOWN");
        match state.to_uppercase().as_str() {
            "SUCCESS" | "COMPLETED" => pass += 1,
            "FAILURE" | "FAILED" | "ERROR" => {
                fail += 1;
                if let Some(n) = entry
                    .get("name")
                    .or_else(|| entry.get("checkName"))
                    .and_then(|v| v.as_str())
                {
                    failing.push(n.to_string());
                }
            }
            _ => pending += 1,
        }
    }
    (pass, fail, pending, failing)
}

fn parse_pr_status(repo: &str, parsed: &serde_json::Value) -> PrStatusReport {
    let (checks_pass, checks_fail, checks_pending, failing_checks) =
        tally_checks(parsed.get("statusCheckRollup"));
    PrStatusReport {
        remote: repo.to_string(),
        pr: json_id(parsed.get("number")),
        title: json_str(parsed.get("title")),
        state: json_str(parsed.get("state")),
        is_draft: parsed
            .get("isDraft")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        url: json_str(parsed.get("url")),
        review_decision: json_str(parsed.get("reviewDecision")),
        checks_pass,
        checks_fail,
        checks_pending,
        failing_checks,
    }
}

fn format_pr_status(report: &PrStatusReport) -> String {
    let pr_label = report
        .pr
        .as_deref()
        .map(|n| format!("PR #{n}"))
        .unwrap_or_else(|| "PR (current branch)".to_string());
    let draft = if report.is_draft { " (draft)" } else { "" };
    let mut message = format!(
        "{pr_label}{draft} — {} — {}\nRemote: {}\nURL: {}\nState: {}\nReview decision: {}\nChecks (pass/fail/pending): {}/{}/{}",
        report.title.as_deref().unwrap_or("unknown"),
        report.url.as_deref().unwrap_or("unknown"),
        report.remote,
        report.url.as_deref().unwrap_or("unknown"),
        report.state.as_deref().unwrap_or("unknown"),
        report.review_decision.as_deref().unwrap_or("none"),
        report.checks_pass,
        report.checks_fail,
        report.checks_pending,
    );
    if !report.failing_checks.is_empty() {
        message.push_str(&format!(
            "\nFailing checks: {}",
            report
                .failing_checks
                .iter()
                .take(10)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    message
}

fn pr_status(host: &dyn GhHost, pr: Option<&str>, json: bool, out: &mut dyn Write) -> Result<()> {
    let repo = current_origin_repo(host)?;
    let argv = build_pr_status_argv(pr)?;
    let cap = scoped_gh(host, argv, &repo)?;
    if cap.exit_code != 0 {
        return Err(classify_gh_error(
            &cap.stderr,
            "`gh pr view` failed. Ensure you are in a git repository with an open PR.",
        ));
    }
    let parsed: serde_json::Value =
        serde_json::from_str(cap.stdout.trim()).context("parse `gh pr view` JSON")?;
    let report = parse_pr_status(&repo, &parsed);
    if json {
        writeln!(out, "{}", serde_json::to_string_pretty(&report)?)?;
    } else {
        writeln!(out, "{}", format_pr_status(&report))?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
struct PrCreateReport {
    remote: String,
    draft: bool,
    url: String,
    title: String,
}

fn pr_create(
    host: &dyn GhHost,
    title: &str,
    body: Option<&str>,
    base: Option<&str>,
    open: bool,
    json: bool,
    out: &mut dyn Write,
) -> Result<()> {
    let repo = current_origin_repo(host)?;
    let body = body
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(title);
    let head = current_git_branch(host);
    let argv = build_pr_create_argv(title, body, base, head.as_deref(), open)?;
    let cap = scoped_gh(host, argv, &repo)?;
    if cap.exit_code != 0 {
        return Err(classify_gh_error(
            &cap.stderr,
            "`gh pr create` failed. Push the branch and retry. Turbo does not auto-merge.",
        ));
    }
    let url = cap.stdout.trim().to_string();
    if url.is_empty() {
        bail!("`gh pr create` succeeded but printed no URL.");
    }
    let report = PrCreateReport {
        remote: repo,
        draft: !open,
        url,
        title: title.to_string(),
    };
    if json {
        writeln!(out, "{}", serde_json::to_string_pretty(&report)?)?;
    } else if report.draft {
        writeln!(
            out,
            "Draft PR created: {}\nRemote: {}\nNot opened for review (pass --open to create ready-for-review). Turbo does not merge.",
            report.url, report.remote
        )?;
    } else {
        writeln!(
            out,
            "PR opened (ready for review): {}\nRemote: {}\nTurbo does not merge.",
            report.url, report.remote
        )?;
    }
    Ok(())
}

pub fn run(args: PrArgs) -> Result<()> {
    run_pr_with(&LiveHost, args, &mut std::io::stdout())
}

fn run_pr_with(host: &dyn GhHost, args: PrArgs, out: &mut dyn Write) -> Result<()> {
    match args.command {
        PrCommand::Status { pr, json } => pr_status(host, pr.as_deref(), json, out),
        PrCommand::Create {
            title,
            body,
            base,
            open,
            json,
        } => pr_create(
            host,
            &title,
            body.as_deref(),
            base.as_deref(),
            open,
            json,
            out,
        ),
    }
}

// ---------------------------------------------------------------------------
// Pipeline status / rerun
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct PipelineStatusReport {
    remote: String,
    run_id: Option<String>,
    name: Option<String>,
    status: Option<String>,
    conclusion: Option<String>,
    head_branch: Option<String>,
    event: Option<String>,
    url: Option<String>,
    failing_jobs: Vec<String>,
}

fn extract_failing_jobs(parsed: &serde_json::Value) -> Vec<String> {
    let mut failing = Vec::new();
    if let Some(jobs) = parsed.get("jobs").and_then(|v| v.as_array()) {
        for job in jobs {
            let conclusion = job.get("conclusion").and_then(|v| v.as_str()).unwrap_or("");
            if matches!(conclusion, "failure" | "timed_out" | "cancelled") {
                if let Some(name) = job.get("name").and_then(|v| v.as_str()) {
                    failing.push(name.to_string());
                }
            }
        }
    }
    failing
}

fn parse_run_view(repo: &str, parsed: &serde_json::Value) -> PipelineStatusReport {
    PipelineStatusReport {
        remote: repo.to_string(),
        run_id: json_id(parsed.get("databaseId")),
        name: json_str(parsed.get("name")),
        status: json_str(parsed.get("status")),
        conclusion: json_str(parsed.get("conclusion")),
        head_branch: json_str(parsed.get("headBranch")),
        event: json_str(parsed.get("event")),
        url: json_str(parsed.get("url")),
        failing_jobs: extract_failing_jobs(parsed),
    }
}

fn format_pipeline_status(report: &PipelineStatusReport) -> String {
    let mut message = format!(
        "CI Run: {} (ID: {})\nRemote: {}\nStatus: {}\nConclusion: {}\nBranch: {}\nEvent: {}\nURL: {}",
        report.name.as_deref().unwrap_or("unknown"),
        report.run_id.as_deref().unwrap_or("unknown"),
        report.remote,
        report.status.as_deref().unwrap_or("unknown"),
        report.conclusion.as_deref().unwrap_or("unknown"),
        report.head_branch.as_deref().unwrap_or("unknown"),
        report.event.as_deref().unwrap_or("unknown"),
        report.url.as_deref().unwrap_or("unknown"),
    );
    if !report.failing_jobs.is_empty() {
        message.push_str(&format!(
            "\nFailing jobs: {}",
            report
                .failing_jobs
                .iter()
                .take(10)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    message
}

fn pipeline_status(
    host: &dyn GhHost,
    run: Option<&str>,
    json: bool,
    out: &mut dyn Write,
) -> Result<()> {
    let repo = current_origin_repo(host)?;
    let run_id = if let Some(run) = run {
        validate_user_token(run, "Run ID")?.to_string()
    } else {
        let branch = current_git_branch(host);
        let list = scoped_gh(host, build_ci_run_list_argv(branch.as_deref()), &repo)?;
        if list.exit_code != 0 {
            return Err(classify_gh_error(
                &list.stderr,
                "`gh run list` failed. Ensure `gh` is authenticated.",
            ));
        }
        let parsed: serde_json::Value =
            serde_json::from_str(list.stdout.trim()).context("parse `gh run list` JSON")?;
        let Some(runs) = parsed.as_array() else {
            bail!("Unexpected `gh run list` output — expected a JSON array.");
        };
        if runs.is_empty() {
            if json {
                let report = PipelineStatusReport {
                    remote: repo,
                    run_id: None,
                    name: None,
                    status: None,
                    conclusion: None,
                    head_branch: branch,
                    event: None,
                    url: None,
                    failing_jobs: vec![],
                };
                writeln!(out, "{}", serde_json::to_string_pretty(&report)?)?;
            } else if branch.is_some() {
                writeln!(out, "No CI runs found for this branch on {repo}.")?;
            } else {
                writeln!(
                    out,
                    "No CI runs found on {repo}; current git branch could not be resolved."
                )?;
            }
            return Ok(());
        }
        json_id(runs[0].get("databaseId"))
            .ok_or_else(|| anyhow::anyhow!("Latest `gh run list` entry had no databaseId."))?
    };
    let view = scoped_gh(host, build_ci_run_view_argv(&run_id)?, &repo)?;
    if view.exit_code != 0 {
        return Err(classify_gh_error(
            &view.stderr,
            &format!("`gh run view {run_id}` failed."),
        ));
    }
    let parsed: serde_json::Value =
        serde_json::from_str(view.stdout.trim()).context("parse `gh run view` JSON")?;
    let report = parse_run_view(&repo, &parsed);
    if json {
        writeln!(out, "{}", serde_json::to_string_pretty(&report)?)?;
    } else {
        writeln!(out, "{}", format_pipeline_status(&report))?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
struct PipelineRerunReport {
    remote: String,
    run_id: String,
    failed_only: bool,
    url: Option<String>,
}

fn pipeline_rerun(
    host: &dyn GhHost,
    run: &str,
    failed_only: bool,
    json: bool,
    out: &mut dyn Write,
) -> Result<()> {
    let repo = current_origin_repo(host)?;
    let argv = build_ci_rerun_argv(run, failed_only)?;
    let cap = scoped_gh(host, argv, &repo)?;
    if cap.exit_code != 0 {
        return Err(classify_gh_error(
            &cap.stderr,
            &format!("`gh run rerun {run}` failed. Turbo does not merge."),
        ));
    }
    let url = {
        let t = cap.stdout.trim();
        (!t.is_empty()).then(|| t.to_string())
    };
    let report = PipelineRerunReport {
        remote: repo,
        run_id: run.trim().to_string(),
        failed_only,
        url,
    };
    if json {
        writeln!(out, "{}", serde_json::to_string_pretty(&report)?)?;
    } else {
        let scope = if failed_only {
            "failed jobs only"
        } else {
            "full run"
        };
        writeln!(
            out,
            "CI run `{}` rerun started ({scope}) on {}.\nTurbo does not merge.",
            report.run_id, report.remote
        )?;
    }
    Ok(())
}

pub fn run_pipeline(args: PipelineArgs) -> Result<()> {
    run_pipeline_with(&LiveHost, args, &mut std::io::stdout())
}

fn run_pipeline_with(host: &dyn GhHost, args: PipelineArgs, out: &mut dyn Write) -> Result<()> {
    match args.command {
        PipelineCommand::Status { run, json } => pipeline_status(host, run.as_deref(), json, out),
        PipelineCommand::Rerun {
            run,
            failed_only,
            json,
        } => pipeline_rerun(host, &run, failed_only, json, out),
    }
}

// ---------------------------------------------------------------------------
// Tests (mocked `gh` — no network)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Command, PagerArgs};
    use clap::Parser as _;
    use std::cell::RefCell;

    struct MockHost {
        origin_url: String,
        branch: String,
        gh: Box<dyn Fn(&[String]) -> Captured>,
        gh_calls: RefCell<Vec<Vec<String>>>,
        git_fail: bool,
    }

    impl MockHost {
        fn new(origin_url: &str, gh: impl Fn(&[String]) -> Captured + 'static) -> Self {
            Self {
                origin_url: origin_url.to_string(),
                branch: "feature/pr-plane".into(),
                gh: Box::new(gh),
                gh_calls: RefCell::new(Vec::new()),
                git_fail: false,
            }
        }
    }

    impl GhHost for MockHost {
        fn git(&self, argv: &[String]) -> Result<Captured> {
            if self.git_fail {
                return Ok(Captured {
                    stdout: String::new(),
                    stderr: "fatal: not a git repository".into(),
                    exit_code: 128,
                });
            }
            let joined: Vec<&str> = argv.iter().map(String::as_str).collect();
            let stdout = match joined.as_slice() {
                ["remote", "get-url", "origin"] => self.origin_url.clone(),
                ["rev-parse", "--abbrev-ref", "HEAD"] => self.branch.clone(),
                _ => String::new(),
            };
            Ok(Captured {
                stdout,
                stderr: String::new(),
                exit_code: 0,
            })
        }

        fn gh(&self, argv: &[String]) -> Result<Captured> {
            self.gh_calls.borrow_mut().push(argv.to_vec());
            Ok((self.gh)(argv))
        }
    }

    fn ok_json(body: &str) -> Captured {
        Captured {
            stdout: body.to_string(),
            stderr: String::new(),
            exit_code: 0,
        }
    }

    fn parse_pr(argv: &[&str]) -> PrCommand {
        let args = PagerArgs::try_parse_from(argv).expect("pr args should parse");
        match args.command {
            Some(Command::Pr(PrArgs { command })) => command,
            other => panic!("expected pr, got {other:?}"),
        }
    }

    fn parse_pipeline(argv: &[&str]) -> PipelineCommand {
        let args = PagerArgs::try_parse_from(argv).expect("pipeline args should parse");
        match args.command {
            Some(Command::Pipeline(PipelineArgs { command })) => command,
            other => panic!("expected pipeline, got {other:?}"),
        }
    }

    #[test]
    fn pr_status_and_create_parse() {
        match parse_pr(&["turbo", "pr", "status"]) {
            PrCommand::Status { pr, json } => {
                assert!(pr.is_none());
                assert!(!json);
            }
            other => panic!("{other:?}"),
        }
        match parse_pr(&["turbo", "pr", "status", "42", "--json"]) {
            PrCommand::Status { pr, json } => {
                assert_eq!(pr.as_deref(), Some("42"));
                assert!(json);
            }
            other => panic!("{other:?}"),
        }
        match parse_pr(&[
            "turbo",
            "pr",
            "create",
            "--title",
            "feat(gh): draft",
            "--body",
            "body",
        ]) {
            PrCommand::Create { open, .. } => assert!(!open),
            other => panic!("{other:?}"),
        }
        match parse_pr(&[
            "turbo",
            "pr",
            "create",
            "--title",
            "feat(gh): open",
            "--open",
        ]) {
            PrCommand::Create { open, .. } => assert!(open),
            other => panic!("{other:?}"),
        }
        assert!(
            PagerArgs::try_parse_from(["turbo", "pr", "merge"]).is_err(),
            "turbo pr must not expose merge"
        );
    }

    #[test]
    fn pipeline_status_and_rerun_parse() {
        match parse_pipeline(&["turbo", "pipeline", "status"]) {
            PipelineCommand::Status { run, json } => {
                assert!(run.is_none());
                assert!(!json);
            }
            other => panic!("{other:?}"),
        }
        match parse_pipeline(&["turbo", "pipeline", "rerun", "99", "--failed-only"]) {
            PipelineCommand::Rerun {
                run, failed_only, ..
            } => {
                assert_eq!(run, "99");
                assert!(failed_only);
            }
            other => panic!("{other:?}"),
        }
        assert!(PagerArgs::try_parse_from(["turbo", "pipeline", "merge"]).is_err());
    }

    #[test]
    fn parse_github_owner_repo_https_and_ssh() {
        assert_eq!(
            parse_github_owner_repo("https://github.com/danmsheets-dev/turbo-grok-build.git")
                .as_deref(),
            Some("danmsheets-dev/turbo-grok-build")
        );
        assert_eq!(
            parse_github_owner_repo("git@github.com:danmsheets-dev/turbo-grok-build.git")
                .as_deref(),
            Some("danmsheets-dev/turbo-grok-build")
        );
        assert_eq!(
            parse_github_owner_repo("ssh://git@github.com/xai-org/grok-build").as_deref(),
            Some("xai-org/grok-build")
        );
        assert_eq!(parse_github_owner_repo("https://gitlab.com/foo/bar"), None);
    }

    #[test]
    fn inject_repo_flag_after_subcommand_unless_already_set() {
        let mut argv = vec!["pr".into(), "view".into(), "--".into(), "1".into()];
        inject_repo_flag(&mut argv, "owner/repo");
        assert_eq!(argv, vec!["pr", "--repo", "owner/repo", "view", "--", "1"]);
        inject_repo_flag(&mut argv, "other/repo");
        assert_eq!(argv[2], "owner/repo");
    }

    #[test]
    fn user_tokens_reject_empty_and_flag_like_values() {
        for value in ["", "  ", "-12345"] {
            assert!(validate_user_token(value, "Run ID").is_err());
        }
        assert_eq!(validate_user_token(" 12345 ", "Run ID").unwrap(), "12345");
    }

    #[test]
    fn pr_create_argv_is_draft_by_default_and_never_merges() {
        let draft =
            build_pr_create_argv("feat: x", "body", None, Some("feature/pr-plane"), false).unwrap();
        assert!(draft.contains(&"--draft".to_string()));
        assert!(!draft.iter().any(|a| a.contains("merge")));
        let open =
            build_pr_create_argv("feat: x", "body", None, Some("feature/pr-plane"), true).unwrap();
        assert!(!open.contains(&"--draft".to_string()));
        assert!(!open.iter().any(|a| a.contains("merge")));
    }

    #[test]
    fn pr_status_from_mocked_gh_json_scopes_to_origin() {
        let host = MockHost::new("git@github.com:acme/app.git", |_| {
            ok_json(
                r#"{
                      "number": 42,
                      "title": "Fix the thing",
                      "state": "OPEN",
                      "isDraft": true,
                      "url": "https://github.com/acme/app/pull/42",
                      "reviewDecision": "REVIEW_REQUIRED",
                      "statusCheckRollup": [
                        {"state": "SUCCESS", "name": "build"},
                        {"state": "FAILURE", "name": "lint"}
                      ]
                    }"#,
            )
        });
        let mut buf = Vec::new();
        run_pr_with(
            &host,
            PrArgs {
                command: PrCommand::Status {
                    pr: None,
                    json: false,
                },
            },
            &mut buf,
        )
        .unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("PR #42"));
        assert!(text.contains("(draft)"));
        assert!(text.contains("Fix the thing"));
        assert!(text.contains("Remote: acme/app"));
        assert!(text.contains("Failing checks: lint"));
        let call = &host.gh_calls.borrow()[0];
        assert_eq!(call[0], "pr");
        assert_eq!(call[1], "--repo");
        assert_eq!(call[2], "acme/app");
        assert_eq!(call[3], "view");
    }

    #[test]
    fn pr_create_draft_from_mocked_gh_does_not_open() {
        let host = MockHost::new("https://github.com/acme/app.git", |_| {
            ok_json("https://github.com/acme/app/pull/7\n")
        });
        let mut buf = Vec::new();
        run_pr_with(
            &host,
            PrArgs {
                command: PrCommand::Create {
                    title: "feat(gh): draft create".into(),
                    body: Some("notes".into()),
                    base: None,
                    open: false,
                    json: true,
                },
            },
            &mut buf,
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["draft"], true);
        assert_eq!(v["remote"], "acme/app");
        assert_eq!(v["url"], "https://github.com/acme/app/pull/7");
        let call = &host.gh_calls.borrow()[0];
        assert!(call.contains(&"--draft".to_string()));
        assert_eq!(call[1], "--repo");
        assert_eq!(call[2], "acme/app");
        assert!(!call.iter().any(|a| a.contains("merge")));
    }

    #[test]
    fn pr_create_open_requires_explicit_flag() {
        let host = MockHost::new("https://github.com/acme/app.git", |_| {
            ok_json("https://github.com/acme/app/pull/8\n")
        });
        let mut buf = Vec::new();
        run_pr_with(
            &host,
            PrArgs {
                command: PrCommand::Create {
                    title: "feat(gh): open".into(),
                    body: None,
                    base: None,
                    open: true,
                    json: false,
                },
            },
            &mut buf,
        )
        .unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("ready for review"));
        let call = &host.gh_calls.borrow()[0];
        assert!(!call.contains(&"--draft".to_string()));
    }

    #[test]
    fn pr_rejects_non_github_origin() {
        let host = MockHost::new("https://gitlab.com/acme/app.git", |_| ok_json("{}"));
        let err = run_pr_with(
            &host,
            PrArgs {
                command: PrCommand::Status {
                    pr: None,
                    json: false,
                },
            },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("not a GitHub URL"), "{}", err);
        assert!(host.gh_calls.borrow().is_empty());
    }

    #[test]
    fn pipeline_status_from_mocked_gh_run_list_then_view() {
        let host = MockHost::new("git@github.com:acme/app.git", |argv| {
            if argv.iter().any(|a| a == "list") {
                ok_json(r#"[{"databaseId": 999, "name": "CI", "status": "COMPLETED"}]"#)
            } else {
                ok_json(
                    r#"{
                      "databaseId": 999,
                      "name": "CI",
                      "status": "COMPLETED",
                      "conclusion": "FAILURE",
                      "headBranch": "feature/pr-plane",
                      "event": "push",
                      "url": "https://github.com/acme/app/actions/runs/999",
                      "jobs": [
                        {"name": "lint", "conclusion": "failure"},
                        {"name": "build", "conclusion": "success"}
                      ]
                    }"#,
                )
            }
        });
        let mut buf = Vec::new();
        run_pipeline_with(
            &host,
            PipelineArgs {
                command: PipelineCommand::Status {
                    run: None,
                    json: false,
                },
            },
            &mut buf,
        )
        .unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("ID: 999"));
        assert!(text.contains("Failing jobs: lint"));
        assert!(text.contains("Remote: acme/app"));
        let calls = host.gh_calls.borrow();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].contains(&"--branch".to_string()));
        assert_eq!(calls[0][1], "--repo");
        assert_eq!(calls[1][1], "--repo");
        assert!(calls[1].contains(&"view".to_string()));
    }

    #[test]
    fn pipeline_rerun_from_mocked_gh_failed_only() {
        let host = MockHost::new("https://github.com/acme/app.git", |_| ok_json(""));
        let mut buf = Vec::new();
        run_pipeline_with(
            &host,
            PipelineArgs {
                command: PipelineCommand::Rerun {
                    run: "555".into(),
                    failed_only: true,
                    json: true,
                },
            },
            &mut buf,
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["run_id"], "555");
        assert_eq!(v["failed_only"], true);
        assert_eq!(v["remote"], "acme/app");
        let call = &host.gh_calls.borrow()[0];
        assert_eq!(
            call.iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["run", "--repo", "acme/app", "rerun", "--failed", "--", "555"]
        );
        assert!(!call.iter().any(|a| a.contains("merge")));
    }

    #[test]
    fn pipeline_status_argv_builders() {
        assert_eq!(
            build_ci_run_list_argv(Some("feature/pr-plane")),
            vec![
                "run",
                "list",
                "--limit",
                "1",
                "--branch",
                "feature/pr-plane",
                "--json",
                CI_RUN_JSON_FIELDS,
            ]
        );
        assert_eq!(
            build_ci_run_view_argv("12345").unwrap(),
            vec![
                "run",
                "view",
                "--json",
                CI_RUN_VIEW_JSON_FIELDS,
                "--",
                "12345",
            ]
        );
        assert_eq!(
            build_ci_rerun_argv("12345", false).unwrap(),
            vec!["run", "rerun", "--", "12345"]
        );
        assert!(build_ci_rerun_argv("-1", false).is_err());
        assert!(build_pr_status_argv(Some("-n")).is_err());
    }

    #[test]
    fn pr_status_handles_missing_fields() {
        let report = parse_pr_status("acme/app", &serde_json::json!({}));
        let msg = format_pr_status(&report);
        assert!(msg.contains("PR (current branch)"));
        assert!(msg.contains("unknown"));
        assert_eq!(report.remote, "acme/app");
    }

    #[test]
    fn pr_status_reports_missing_origin() {
        let mut host = MockHost::new("git@github.com:acme/app.git", |_| ok_json("{}"));
        host.git_fail = true;
        let err = run_pr_with(
            &host,
            PrArgs {
                command: PrCommand::Status {
                    pr: None,
                    json: false,
                },
            },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("git remote `origin`"), "{}", err);
    }
}
