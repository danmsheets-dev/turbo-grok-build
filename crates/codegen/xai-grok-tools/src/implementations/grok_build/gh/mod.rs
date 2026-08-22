//! `gh` CLI control-plane tools (GitHub PR + CI status, CI rerun).
//!
//! Thin wrappers around the `gh` CLI — no GitHub API client, no new deps.
//! Spawned shell-free via `tokio::process::Command` with bounded execution.
//!
//! Tools:
//! - `gh_pr_status` — read-only PR status summary
//! - `gh_ci_status` — read-only CI run status summary
//! - `gh_ci_rerun` — mutating CI rerun (Execute kind)

use std::process::Stdio;
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

/// Timeout for a single `gh` invocation.
const GH_TIMEOUT: Duration = Duration::from_secs(30);

/// Max bytes of captured output before truncation.
const GH_OUTPUT_CAP_BYTES: usize = 60_000;

/// Resolve the `gh` binary path, returning an actionable error when missing.
fn resolve_gh() -> Result<std::path::PathBuf, xai_tool_runtime::ToolError> {
    which::which("gh").map_err(|_| {
        xai_tool_runtime::ToolError::custom(
            "gh_not_found",
            "GitHub CLI (`gh`) is not installed or not on PATH. \
             Install it from https://cli.github.com/ or run `gh auth login` \
             after installing. This tool requires `gh` to query GitHub.",
        )
    })
}

/// Run a `gh` command with bounded timeout, capturing stdout/stderr.
/// Returns (stdout, stderr, exit_code) on completion, or a ToolError on timeout/spawn failure.
async fn run_gh(
    argv: &[String],
    timeout_secs: Option<u64>,
    mutating: bool,
) -> Result<(Vec<u8>, Vec<u8>, i32), xai_tool_runtime::ToolError> {
    let gh_path = resolve_gh()?;
    let mut cmd = tokio::process::Command::new(&gh_path);
    cmd.args(argv)
        .env("GH_NO_BROWSER", "1")
        .env("CI", "1")
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let timeout = timeout_secs
        .map(Duration::from_secs)
        .unwrap_or(GH_TIMEOUT);

    let result = tokio::time::timeout(timeout, cmd.output()).await;

    let output = match result {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            return Err(xai_tool_runtime::ToolError::custom(
                "gh_spawn_failed",
                format!("Failed to spawn `gh`: {e}"),
            ));
        }
        Err(_) => {
            let outcome = if mutating {
                " The rerun outcome is unknown; check GitHub before retrying."
            } else {
                ""
            };
            return Err(xai_tool_runtime::ToolError::custom(
                "gh_timeout",
                format!(
                    "`gh` command timed out after {}s. Try narrowing the query or check your GitHub connection.{outcome}",
                    timeout.as_secs()
                ),
            ));
        }
    };

    let stdout = cap_bytes(output.stdout, GH_OUTPUT_CAP_BYTES);
    let stderr = cap_bytes(output.stderr, GH_OUTPUT_CAP_BYTES);
    let exit_code = output.status.code().unwrap_or(-1);

    Ok((stdout, stderr, exit_code))
}

fn validate_user_token<'a>(
    value: &'a str,
    error_code: &'static str,
    label: &'static str,
) -> Result<&'a str, xai_tool_runtime::ToolError> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('-') {
        return Err(xai_tool_runtime::ToolError::custom(
            error_code,
            format!("{label} is required and must not start with `-`."),
        ));
    }
    Ok(value)
}

async fn current_git_branch() -> Option<String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["rev-parse", "--abbrev-ref", "HEAD"])
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = tokio::time::timeout(GH_TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&cap_bytes(output.stdout, GH_OUTPUT_CAP_BYTES))
        .trim()
        .to_string();
    (!branch.is_empty() && branch != "HEAD").then_some(branch)
}

/// Truncate captured output to `cap` bytes, appending a marker if truncated.
fn cap_bytes(input: Vec<u8>, cap: usize) -> Vec<u8> {
    if input.len() <= cap {
        return input;
    }
    let mut truncated = input[..cap].to_vec();
    truncated.extend_from_slice(b"\n[truncated]");
    truncated
}

// ===========================================================================
// gh_pr_status
// ===========================================================================

const PR_STATUS_JSON_FIELDS: &str = "number,title,state,statusCheckRollup,reviewDecision,url";
const CI_RUN_JSON_FIELDS: &str = "databaseId,name,status,conclusion,headBranch,event,url";
const CI_RUN_VIEW_JSON_FIELDS: &str = "databaseId,name,status,conclusion,headBranch,event,url,jobs";

fn build_pr_status_argv(pr: Option<&str>) -> Vec<String> {
    let mut argv = vec![
        "pr".to_string(),
        "view".to_string(),
        "--json".to_string(),
        PR_STATUS_JSON_FIELDS.to_string(),
    ];
    if let Some(pr) = pr {
        argv.extend(["--".to_string(), pr.to_string()]);
    }
    argv
}

fn build_ci_run_view_argv(run_id: &str) -> Vec<String> {
    vec![
        "run".to_string(),
        "view".to_string(),
        "--json".to_string(),
        CI_RUN_VIEW_JSON_FIELDS.to_string(),
        "--".to_string(),
        run_id.to_string(),
    ]
}

fn build_ci_run_list_argv(branch: Option<&str>) -> Vec<String> {
    let mut argv = vec![
        "run".to_string(),
        "list".to_string(),
        "--limit".to_string(),
        "1".to_string(),
    ];
    if let Some(branch) = branch {
        argv.extend(["--branch".to_string(), branch.to_string()]);
    }
    argv.extend(["--json".to_string(), CI_RUN_JSON_FIELDS.to_string()]);
    argv
}

fn build_ci_rerun_argv(run_id: &str, failed_only: bool) -> Vec<String> {
    let mut argv = vec!["run".to_string(), "rerun".to_string()];
    if failed_only {
        argv.push("--failed".to_string());
    }
    argv.extend(["--".to_string(), run_id.to_string()]);
    argv
}

pub const GH_PR_STATUS_TOOL_NAME: &str = "gh_pr_status";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GhPrStatusInput {
    /// PR number to query. If omitted, detects the current branch's PR
    /// via `gh pr view --json ...`.
    #[serde(default)]
    #[schemars(
        description = "PR number to query. If omitted, detects the current branch's PR automatically."
    )]
    pub pr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GhPrStatusOutput {
    pub success: bool,
    pub message: String,
    pub pr_number: Option<String>,
    pub pr_title: Option<String>,
    pub state: Option<String>,
    pub url: Option<String>,
    pub checks_pass: usize,
    pub checks_fail: usize,
    pub checks_pending: usize,
    pub failing_checks: Vec<String>,
    pub review_decision: Option<String>,
}

impl xai_tool_runtime::ToolOutput for GhPrStatusOutput {
    fn model_output(&self) -> Vec<xai_tool_runtime::ContentBlock> {
        vec![xai_tool_runtime::ContentBlock::Text {
            text: self.message.clone(),
        }]
    }
}

#[derive(Debug, Default)]
pub struct GhPrStatusTool;

impl crate::types::tool_metadata::ToolMetadata for GhPrStatusTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Read
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        r#"Query GitHub PR status via the `gh` CLI.

Returns a compact structured summary: PR state, title, URL, check run counts
(pass/fail/pending) with up to 10 failing check names, and the review decision.

If `pr` is omitted, detects the current branch's PR automatically via
`gh pr view`.

Requires the `gh` CLI to be installed and authenticated (`gh auth login`).
This is a v1 read-only tool — it does not create or modify PRs."#
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

impl xai_tool_runtime::Tool for GhPrStatusTool {
    type Args = GhPrStatusInput;
    type Output = GhPrStatusOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(GH_PR_STATUS_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            GH_PR_STATUS_TOOL_NAME,
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: true,
            tool_scope: Some(xai_tool_protocol::ToolScope::Read),
            ..Default::default()
        }
    }

    #[tracing::instrument(
        name = "tool.gh_pr_status",
        skip_all,
        fields(pr = input.pr.as_deref().unwrap_or("auto"))
    )]
    async fn run(
        &self,
        _ctx: xai_tool_runtime::ToolCallContext,
        input: GhPrStatusInput,
    ) -> Result<GhPrStatusOutput, xai_tool_runtime::ToolError> {
        run_gh_pr_status(input).await
    }
}

async fn run_gh_pr_status(input: GhPrStatusInput) -> Result<GhPrStatusOutput, xai_tool_runtime::ToolError> {
    // Early guard: refuse if gh is not installed.
    resolve_gh()?;

    let pr = input
        .pr
        .as_deref()
        .map(|pr| validate_user_token(pr, "gh_pr_invalid_pr", "PR identifier"))
        .transpose()?;
    let argv = build_pr_status_argv(pr);

    let (stdout, stderr, exit_code) = run_gh(&argv, Some(30), false).await?;

    if exit_code != 0 {
        let stderr_str = String::from_utf8_lossy(&stderr);
        let msg = stderr_str.trim();
        if msg.contains("authentication") || msg.contains("auth") || msg.contains("token") {
            return Err(xai_tool_runtime::ToolError::custom(
                "gh_auth_failed",
                format!(
                    "`gh` auth failed. Run `gh auth login` to authenticate with GitHub, \
                     then retry. Detail: {msg}"
                ),
            ));
        }
        if msg.contains("not found") || msg.contains("404") || msg.contains("no such") {
            return Err(xai_tool_runtime::ToolError::custom(
                "gh_pr_not_found",
                format!(
                    "PR not found. Verify the PR number or that you are in a \
                     repository with an open PR on this branch. Detail: {msg}"
                ),
            ));
        }
        return Err(xai_tool_runtime::ToolError::custom(
            "gh_pr_view_failed",
            format!(
                "`gh pr view` failed (exit {}). Ensure you are in a git repository \
                 with the `gh` CLI authenticated. Detail: {msg}",
                exit_code
            ),
        ));
    }

    let body = String::from_utf8_lossy(&stdout);
    let parsed: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            return Err(xai_tool_runtime::ToolError::custom(
                "gh_pr_parse_failed",
                format!(
                    "Could not parse `gh pr view` JSON output ({e}). \
                     Raw output (truncated to {} bytes):\n{}",
                    GH_OUTPUT_CAP_BYTES,
                    truncate_for_display(&body, GH_OUTPUT_CAP_BYTES)
                ),
            ));
        }
    };

    Ok(parse_pr_status(&parsed, input.pr))
}

/// Format the PR status output into a human-readable message.
fn parse_pr_status(parsed: &serde_json::Value, pr_input: Option<String>) -> GhPrStatusOutput {
    let number = parsed
        .get("number")
        .and_then(|v| v.as_str().map(|s| s.to_string()).or_else(|| v.as_u64().map(|n| n.to_string())));
    let title = parsed.get("title").and_then(|v| v.as_str()).map(|s| s.to_string());
    let state = parsed.get("state").and_then(|v| v.as_str()).map(|s| s.to_string());
    let url = parsed.get("url").and_then(|v| v.as_str()).map(|s| s.to_string());

    let review_decision = parsed
        .get("reviewDecision")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // statusCheckRollup is the field name from `--json number,title,state,statusCheckRollup,reviewDecision,url`.
    // gh outputs it as `statusCheckRollup` in JSON. It contains an array of
    // check run objects with `state` field.
    let (checks_pass, checks_fail, checks_pending, failing_names) =
        tally_checks(parsed.get("statusCheckRollup"));

    let pr_label = if let Some(ref n) = number {
        format!("PR #{n}")
    } else if let Some(ref pr) = pr_input {
        format!("PR {pr}")
    } else {
        "PR (current branch)".to_string()
    };

    let state_str = state.clone().unwrap_or_else(|| "unknown".to_string());

    let mut message = format!(
        "{} — {} — {}\nURL: {}\nState: {}\nReview decision: {}\nChecks (pass/fail/pending): {}/{}/{}",
        pr_label,
        title.as_deref().unwrap_or("unknown"),
        url.as_deref().unwrap_or("unknown"),
        url.as_deref().unwrap_or("unknown"),
        state_str,
        review_decision.as_deref().unwrap_or("none"),
        checks_pass,
        checks_fail,
        checks_pending,
    );

    if !failing_names.is_empty() {
        let display = failing_names.iter().take(10).cloned().collect::<Vec<_>>().join(", ");
        message.push_str(&format!("\nFailing checks: {}", display));
    }

    GhPrStatusOutput {
        success: true,
        message,
        pr_number: number,
        pr_title: title,
        state,
        url,
        checks_pass,
        checks_fail,
        checks_pending,
        failing_checks: failing_names,
        review_decision,
    }
}

/// Tally check runs from the `statusCheckRollup` field.
/// Returns (pass, fail, pending, failing_names).
fn tally_checks(status_check_rollup: Option<&serde_json::Value>) -> (usize, usize, usize, Vec<String>) {
    let mut pass = 0;
    let mut fail = 0;
    let mut pending = 0;
    let mut failing = Vec::new();

    let Some(arr) = status_check_rollup.and_then(|v| v.as_array()) else {
        return (pass, fail, pending, failing);
    };

    for entry in arr {
        let state = entry.get("state").and_then(|v| v.as_str()).unwrap_or("UNKNOWN");
        match state.to_uppercase().as_str() {
            "SUCCESS" | "COMPLETED" => pass += 1,
            "FAILURE" | "FAILED" | "ERROR" => {
                fail += 1;
                if let Some(name) = entry.get("name").or_else(|| entry.get("checkName")) {
                    if let Some(n) = name.as_str() {
                        failing.push(n.to_string());
                    }
                }
            }
            "PENDING" | "IN_PROGRESS" | "QUEUED" => pending += 1,
            _ => pending += 1,
        }
    }

    (pass, fail, pending, failing)
}

/// Truncate a string for display in error messages.
fn truncate_for_display(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}... [truncated]", &s[..max])
    }
}

// ===========================================================================
// gh_ci_status
// ===========================================================================

pub const GH_CI_STATUS_TOOL_NAME: &str = "gh_ci_status";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GhCiStatusInput {
    /// Run ID or name. If omitted, fetches the latest run on the current branch.
    #[serde(default)]
    #[schemars(
        description = "CI run ID. If omitted, fetches the latest run on the current branch."
    )]
    pub run: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GhCiStatusOutput {
    pub success: bool,
    pub message: String,
    pub run_id: Option<String>,
    pub run_name: Option<String>,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub head_branch: Option<String>,
    pub event: Option<String>,
    pub url: Option<String>,
    pub failing_jobs: Vec<String>,
}

impl xai_tool_runtime::ToolOutput for GhCiStatusOutput {
    fn model_output(&self) -> Vec<xai_tool_runtime::ContentBlock> {
        vec![xai_tool_runtime::ContentBlock::Text {
            text: self.message.clone(),
        }]
    }
}

#[derive(Debug, Default)]
pub struct GhCiStatusTool;

impl crate::types::tool_metadata::ToolMetadata for GhCiStatusTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Read
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        r#"Query GitHub Actions CI run status via the `gh` CLI.

Returns a summary: run name, status, conclusion, failing jobs (up to 10),
head branch, event type, and URL.

If `run` is omitted, fetches the latest run on the current branch via
`gh run list`. Requires the `gh` CLI to be installed and authenticated
(`gh auth login`)."#
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

impl xai_tool_runtime::Tool for GhCiStatusTool {
    type Args = GhCiStatusInput;
    type Output = GhCiStatusOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(GH_CI_STATUS_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            GH_CI_STATUS_TOOL_NAME,
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: true,
            tool_scope: Some(xai_tool_protocol::ToolScope::Read),
            ..Default::default()
        }
    }

    #[tracing::instrument(
        name = "tool.gh_ci_status",
        skip_all,
        fields(run = input.run.as_deref().unwrap_or("latest"))
    )]
    async fn run(
        &self,
        _ctx: xai_tool_runtime::ToolCallContext,
        input: GhCiStatusInput,
    ) -> Result<GhCiStatusOutput, xai_tool_runtime::ToolError> {
        run_gh_ci_status(input).await
    }
}

async fn run_gh_ci_status(input: GhCiStatusInput) -> Result<GhCiStatusOutput, xai_tool_runtime::ToolError> {
    resolve_gh()?;

    if let Some(run_id) = input.run.as_deref() {
        let run_id = validate_user_token(run_id, "gh_ci_status_invalid_run", "Run ID")?;
        return run_gh_ci_view(run_id).await;
    }

    let branch = current_git_branch().await;
    let argv = build_ci_run_list_argv(branch.as_deref());
    let (stdout, stderr, exit_code) = run_gh(&argv, Some(30), false).await?;

    if exit_code != 0 {
        let stderr_str = String::from_utf8_lossy(&stderr);
        let msg = stderr_str.trim();
        if msg.contains("authentication") || msg.contains("auth") || msg.contains("token") {
            return Err(xai_tool_runtime::ToolError::custom(
                "gh_auth_failed",
                format!(
                    "`gh` auth failed. Run `gh auth login` to authenticate with GitHub, \
                     then retry. Detail: {msg}"
                ),
            ));
        }
        return Err(xai_tool_runtime::ToolError::custom(
            "gh_run_list_failed",
            format!(
                "`gh run list` failed (exit {}). Ensure you are in a git \
                 repository with the `gh` CLI authenticated. Detail: {msg}",
                exit_code
            ),
        ));
    }

    let body = String::from_utf8_lossy(&stdout);
    let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
        xai_tool_runtime::ToolError::custom(
            "gh_run_parse_failed",
            format!("Could not parse `gh run list` JSON output: {e}"),
        )
    })?;

    let Some(runs) = parsed.as_array() else {
        return Err(xai_tool_runtime::ToolError::custom(
            "gh_run_parse_failed",
            "Unexpected `gh run list` output shape — expected a JSON array.",
        ));
    };
    if runs.is_empty() {
        let message = if branch.is_some() {
            "No CI runs found for this branch.".to_string()
        } else {
            "No CI runs found in this repository; current git branch could not be resolved."
                .to_string()
        };
        return Ok(GhCiStatusOutput {
            success: true,
            message,
            run_id: None,
            run_name: None,
            status: None,
            conclusion: None,
            head_branch: None,
            event: None,
            url: None,
            failing_jobs: vec![],
        });
    }

    let run_id = runs[0]
        .get("databaseId")
        .and_then(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .or_else(|| value.as_u64().map(|id| id.to_string()))
        })
        .ok_or_else(|| {
            xai_tool_runtime::ToolError::custom(
                "gh_run_parse_failed",
                "Latest `gh run list` entry did not contain a databaseId.",
            )
        })?;
    let mut output = run_gh_ci_view(&run_id).await?;
    if branch.is_none() {
        output.message.push_str(
            "\nScope: repository-wide latest run (current git branch could not be resolved).",
        );
    }
    Ok(output)
}

async fn run_gh_ci_view(run_id: &str) -> Result<GhCiStatusOutput, xai_tool_runtime::ToolError> {
    let argv = build_ci_run_view_argv(run_id);
    let (stdout, stderr, exit_code) = run_gh(&argv, Some(30), false).await?;

    if exit_code != 0 {
        let stderr_str = String::from_utf8_lossy(&stderr);
        let msg = stderr_str.trim();
        if msg.contains("authentication") || msg.contains("auth") || msg.contains("token") {
            return Err(xai_tool_runtime::ToolError::custom(
                "gh_auth_failed",
                format!(
                    "`gh` auth failed. Run `gh auth login` to authenticate with GitHub, \
                     then retry. Detail: {msg}"
                ),
            ));
        }
        return Err(xai_tool_runtime::ToolError::custom(
            "gh_run_view_failed",
            format!(
                "`gh run view {run_id}` failed (exit {exit_code}). Ensure you are in a git \
                 repository with the `gh` CLI authenticated. Detail: {msg}",
            ),
        ));
    }

    let body = String::from_utf8_lossy(&stdout);
    let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
        xai_tool_runtime::ToolError::custom(
            "gh_run_parse_failed",
            format!("Could not parse `gh run view` JSON output: {e}"),
        )
    })?;
    Ok(parse_run_view(&parsed))
}

/// Format run data from `gh run view` (or `gh run list` item) into output.
fn parse_run_view(parsed: &serde_json::Value) -> GhCiStatusOutput {
    let run_id = parsed.get("databaseId").map(|v| v.to_string());
    let run_name = parsed.get("name").and_then(|v| v.as_str()).map(|s| s.to_string());
    let status = parsed.get("status").and_then(|v| v.as_str()).map(|s| s.to_string());
    let conclusion = parsed.get("conclusion").and_then(|v| v.as_str()).map(|s| s.to_string());
    let head_branch = parsed.get("headBranch").and_then(|v| v.as_str()).map(|s| s.to_string());
    let event = parsed.get("event").and_then(|v| v.as_str()).map(|s| s.to_string());
    let url = parsed.get("url").and_then(|v| v.as_str()).map(|s| s.to_string());

    // `gh run view --json ...jobs` supplies job-level conclusions.
    // Fall back to steps for richer payloads from compatible gh versions.
    let failing_jobs = extract_failing_jobs(parsed);

    let mut message = format!(
        "CI Run: {} (ID: {})\nStatus: {}\nConclusion: {}\nBranch: {}\nEvent: {}\nURL: {}",
        run_name.as_deref().unwrap_or("unknown"),
        run_id.as_deref().unwrap_or("unknown"),
        status.as_deref().unwrap_or("unknown"),
        conclusion.as_deref().unwrap_or("unknown"),
        head_branch.as_deref().unwrap_or("unknown"),
        event.as_deref().unwrap_or("unknown"),
        url.as_deref().unwrap_or("unknown"),
    );

    if !failing_jobs.is_empty() {
        let display = failing_jobs.iter().take(10).cloned().collect::<Vec<_>>().join(", ");
        message.push_str(&format!("\nFailing jobs: {}", display));
    }

    GhCiStatusOutput {
        success: true,
        message,
        run_id,
        run_name,
        status,
        conclusion,
        head_branch,
        event,
        url,
        failing_jobs,
    }
}

/// Extract failing job names from a `gh run view` JSON payload.
fn extract_failing_jobs(parsed: &serde_json::Value) -> Vec<String> {
    let mut failing = Vec::new();

    // The selected run is fetched with `--json ...jobs` so job-level failures
    // are available in the normal tool response.
    if let Some(jobs) = parsed.get("jobs").and_then(|v| v.as_array()) {
        for job in jobs {
            let job_conclusion = job
                .get("conclusion")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if job_conclusion == "failure" || job_conclusion == "timed_out" || job_conclusion == "cancelled" {
                if let Some(name) = job.get("name").and_then(|v| v.as_str()) {
                    failing.push(name.to_string());
                }
            }
        }
    }

    // Also check `steps` field (from `--json steps`), where each step has
    // a `conclusion` and `name`.
    if failing.is_empty() {
        if let Some(steps) = parsed.get("steps").and_then(|v| v.as_array()) {
            for step in steps {
                let step_conclusion = step
                    .get("conclusion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if step_conclusion == "failure" || step_conclusion == "timed_out" {
                    if let Some(name) = step.get("name").and_then(|v| v.as_str()) {
                        failing.push(name.to_string());
                    }
                }
            }
        }
    }

    failing
}

// ===========================================================================
// gh_ci_rerun
// ===========================================================================

pub const GH_CI_RERUN_TOOL_NAME: &str = "gh_ci_rerun";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GhCiRerunInput {
    /// CI run ID to rerun.
    #[schemars(
        description = "CI run ID to rerun. Required."
    )]
    pub run: String,

    /// If true, only rerun failed jobs within the run.
    #[serde(default)]
    #[schemars(description = "If true, only rerun failed jobs within the run.")]
    pub failed_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GhCiRerunOutput {
    pub success: bool,
    pub message: String,
    pub run_id: Option<String>,
    pub rerun_url: Option<String>,
}

impl xai_tool_runtime::ToolOutput for GhCiRerunOutput {
    fn model_output(&self) -> Vec<xai_tool_runtime::ContentBlock> {
        vec![xai_tool_runtime::ContentBlock::Text {
            text: self.message.clone(),
        }]
    }
}

#[derive(Debug, Default)]
pub struct GhCiRerunTool;

impl crate::types::tool_metadata::ToolMetadata for GhCiRerunTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Execute
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        r#"Rerun a GitHub Actions CI run via the `gh` CLI.

Reruns the given CI run. When `failed_only` is true, only failed jobs are
rerun (`gh run rerun <id> --failed`); otherwise the entire run is rerun.

**This is a mutating action.** It triggers a new CI run on GitHub. The `gh`
CLI must be installed and authenticated (`gh auth login`). If `gh` is missing
or auth is broken, the tool fails with a clear error naming the fix."#
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }

    fn is_read_only(&self) -> bool {
        false
    }
}

impl xai_tool_runtime::Tool for GhCiRerunTool {
    type Args = GhCiRerunInput;
    type Output = GhCiRerunOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(GH_CI_RERUN_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            GH_CI_RERUN_TOOL_NAME,
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: false,
            tool_scope: Some(xai_tool_protocol::ToolScope::Write),
            ..Default::default()
        }
    }

    #[tracing::instrument(
        name = "tool.gh_ci_rerun",
        skip_all,
        fields(run = %input.run, failed_only = %input.failed_only)
    )]
    async fn run(
        &self,
        _ctx: xai_tool_runtime::ToolCallContext,
        input: GhCiRerunInput,
    ) -> Result<GhCiRerunOutput, xai_tool_runtime::ToolError> {
        run_gh_ci_rerun(input).await
    }
}

async fn run_gh_ci_rerun(input: GhCiRerunInput) -> Result<GhCiRerunOutput, xai_tool_runtime::ToolError> {
    // Refuse early if gh is not installed — do not even build the command.
    let _gh_path = resolve_gh()?;

    let run_id = validate_user_token(&input.run, "gh_ci_rerun_invalid_run", "Run ID")?;
    let argv = build_ci_rerun_argv(run_id, input.failed_only);

    let (stdout, stderr, exit_code) = run_gh(&argv, Some(30), true).await?;

    if exit_code != 0 {
        let stderr_str = String::from_utf8_lossy(&stderr);
        let msg = stderr_str.trim();
        if msg.contains("authentication") || msg.contains("auth") || msg.contains("token") {
            return Err(xai_tool_runtime::ToolError::custom(
                "gh_auth_failed",
                format!(
                    "`gh` auth failed. Run `gh auth login` to authenticate with GitHub, \
                     then retry the rerun. Detail: {msg}"
                ),
            ));
        }
        return Err(xai_tool_runtime::ToolError::custom(
            "gh_rerun_failed",
            format!(
                "`gh run rerun {}` failed (exit {}). Ensure you are in a git \
                 repository and the run ID is valid. Detail: {msg}",
                run_id, exit_code
            ),
        ));
    }

    // Try to get the rerun URL by querying the run view after rerun.
    // In v1, we return the run ID and a confirmation; the URL is optional
    // because `gh run rerun` may not return a URL directly.
    let stdout_str = String::from_utf8_lossy(&stdout);
    let rerun_url = if !stdout_str.is_empty() {
        Some(stdout_str.trim().to_string())
    } else {
        None
    };

    let scope = if input.failed_only { "failed jobs only" } else { "full run" };
    let message = format!(
        "CI run `{}` rerun started ({}). A new run has been triggered on GitHub.",
        run_id, scope
    );

    Ok(GhCiRerunOutput {
        success: true,
        message,
        run_id: Some(run_id.to_string()),
        rerun_url,
    })
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- Input deserialization tests ---

    #[test]
    fn gh_pr_status_input_deserializes_with_pr() {
        let json = serde_json::json!({ "pr": "12345" });
        let input: GhPrStatusInput = serde_json::from_value(json).expect("valid input");
        assert_eq!(input.pr.as_deref(), Some("12345"));
    }

    #[test]
    fn gh_pr_status_input_deserializes_without_pr() {
        let json = serde_json::json!({});
        let input: GhPrStatusInput = serde_json::from_value(json).expect("valid input");
        assert!(input.pr.is_none());
    }

    #[test]
    fn gh_pr_status_input_deserializes_serde_rename() {
        // The field is `pr` — verify it maps from JSON "pr".
        let json = serde_json::json!({ "pr": "42" });
        let input: GhPrStatusInput = serde_json::from_value(json).expect("valid input");
        assert_eq!(input.pr.as_deref(), Some("42"));
    }

    #[test]
    fn gh_ci_status_input_deserializes_with_run() {
        let json = serde_json::json!({ "run": "12345" });
        let input: GhCiStatusInput = serde_json::from_value(json).expect("valid input");
        assert_eq!(input.run.as_deref(), Some("12345"));
    }

    #[test]
    fn gh_ci_status_input_deserializes_without_run() {
        let json = serde_json::json!({});
        let input: GhCiStatusInput = serde_json::from_value(json).expect("valid input");
        assert!(input.run.is_none());
    }

    #[test]
    fn gh_ci_rerun_input_deserializes_failed_only_default_false() {
        let json = serde_json::json!({ "run": "12345" });
        let input: GhCiRerunInput = serde_json::from_value(json).expect("valid input");
        assert_eq!(input.run, "12345");
        assert!(!input.failed_only);
    }

    #[test]
    fn gh_ci_rerun_input_deserializes_failed_only_true() {
        let json = serde_json::json!({ "run": "12345", "failed_only": true });
        let input: GhCiRerunInput = serde_json::from_value(json).expect("valid input");
        assert_eq!(input.run, "12345");
        assert!(input.failed_only);
    }

    #[test]
    fn gh_ci_rerun_input_rejects_missing_run() {
        let json = serde_json::json!({ "failed_only": true });
        let result: Result<GhCiRerunInput, _> = serde_json::from_value(json);
        assert!(result.is_err(), "run is required");
    }

    // --- Command construction tests (no process spawned) ---

    #[test]
    fn gh_pr_status_command_with_pr_number() {
        let argv = build_pr_status_argv(Some("12345"));
        assert_eq!(
            argv.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            vec![
                "pr",
                "view",
                "--json",
                PR_STATUS_JSON_FIELDS,
                "--",
                "12345",
            ]
        );
    }

    #[test]
    fn gh_pr_status_command_without_pr_number() {
        let argv = build_pr_status_argv(None);
        assert_eq!(
            argv.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            vec!["pr", "view", "--json", PR_STATUS_JSON_FIELDS]
        );
    }

    #[test]
    fn gh_ci_status_command_with_run_id() {
        let argv = build_ci_run_view_argv("12345");
        assert_eq!(
            argv.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            vec![
                "run",
                "view",
                "--json",
                CI_RUN_VIEW_JSON_FIELDS,
                "--",
                "12345",
            ]
        );
    }

    #[test]
    fn gh_ci_status_command_without_run_id_uses_branch_filter() {
        let argv = build_ci_run_list_argv(Some("feature/phase5"));
        assert_eq!(
            argv.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            vec![
                "run",
                "list",
                "--limit",
                "1",
                "--branch",
                "feature/phase5",
                "--json",
                CI_RUN_JSON_FIELDS,
            ]
        );
    }

    #[test]
    fn gh_ci_status_command_without_branch_filter_is_repo_wide() {
        let argv = build_ci_run_list_argv(None);
        assert_eq!(
            argv.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            vec!["run", "list", "--limit", "1", "--json", CI_RUN_JSON_FIELDS]
        );
    }

    #[test]
    fn gh_ci_rerun_command_exact_argv() {
        let argv = build_ci_rerun_argv("12345", true);
        assert_eq!(
            argv,
            vec!["run", "rerun", "--failed", "--", "12345"]
                .iter().map(|s| s.to_string()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn gh_ci_rerun_command_without_failed_only() {
        let argv = build_ci_rerun_argv("12345", false);
        assert_eq!(
            argv,
            vec!["run", "rerun", "--", "12345"]
                .iter().map(|s| s.to_string()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn user_tokens_reject_empty_and_flag_like_values() {
        for value in ["", "  ", "-12345"] {
            assert!(validate_user_token(value, "invalid", "Run ID").is_err());
        }
        assert_eq!(
            validate_user_token(" 12345 ", "invalid", "Run ID").unwrap(),
            "12345"
        );
    }

    // --- Output formatting tests ---

    #[test]
    fn pr_status_message_formats_correctly() {
        let parsed = serde_json::json!({
            "number": 42,
            "title": "Fix the thing",
            "state": "OPEN",
            "url": "https://github.com/org/repo/pull/42",
            "reviewDecision": "REVIEW_REQUIRED",
            "statusCheckRollup": []
        });
        let output = parse_pr_status(&parsed, Some("42".to_string()));
        assert!(output.message.contains("PR #42"));
        assert!(output.message.contains("Fix the thing"));
        assert!(output.message.contains("OPEN"));
        assert!(output.message.contains("REVIEW_REQUIRED"));
        assert_eq!(output.pr_number, Some("42".to_string()));
        assert_eq!(output.pr_title, Some("Fix the thing".to_string()));
        assert_eq!(output.state, Some("OPEN".to_string()));
    }

    #[test]
    fn pr_status_message_includes_failing_checks() {
        let parsed = serde_json::json!({
            "number": 7,
            "title": "CI fixes",
            "state": "OPEN",
            "url": "https://github.com/org/repo/pull/7",
            "reviewDecision": null,
            "statusCheckRollup": [
                {"state": "SUCCESS", "name": "build"},
                {"state": "FAILURE", "name": "lint"},
                {"state": "FAILURE", "name": "test"},
                {"state": "PENDING", "name": "deploy"}
            ]
        });
        let output = parse_pr_status(&parsed, Some("7".to_string()));
        assert_eq!(output.checks_pass, 1);
        assert_eq!(output.checks_fail, 2);
        assert_eq!(output.checks_pending, 1);
        assert_eq!(output.failing_checks, vec!["lint", "test"]);
        assert!(output.message.contains("Failing checks: lint, test"));
    }

    #[test]
    fn pr_status_handles_empty_checks() {
        let parsed = serde_json::json!({
            "number": 1,
            "title": "Initial",
            "state": "OPEN",
            "url": "https://github.com/org/repo/pull/1",
            "reviewDecision": null,
            "statusCheckRollup": []
        });
        let output = parse_pr_status(&parsed, None);
        assert_eq!(output.checks_pass, 0);
        assert_eq!(output.checks_fail, 0);
        assert_eq!(output.checks_pending, 0);
        assert!(output.failing_checks.is_empty());
    }

    #[test]
    fn pr_status_handles_missing_fields() {
        let parsed = serde_json::json!({});
        let output = parse_pr_status(&parsed, None);
        assert!(output.message.contains("PR (current branch)"));
        assert!(output.message.contains("unknown"));
    }

    #[test]
    fn ci_status_message_formats_correctly() {
        let parsed = serde_json::json!({
            "databaseId": 99999,
            "name": "CI",
            "status": "COMPLETED",
            "conclusion": "SUCCESS",
            "headBranch": "main",
            "event": "push",
            "url": "https://github.com/org/repo/actions/runs/99999"
        });
        let output = parse_run_view(&parsed);
        assert!(output.message.contains("CI"));
        assert!(output.message.contains("99999"));
        assert!(output.message.contains("COMPLETED"));
        assert!(output.message.contains("SUCCESS"));
        assert!(output.message.contains("main"));
        assert_eq!(output.run_id, Some("99999".to_string()));
        assert_eq!(output.conclusion, Some("SUCCESS".to_string()));
    }

    #[test]
    fn ci_status_extracts_failing_jobs() {
        let parsed = serde_json::json!({
            "databaseId": 1,
            "name": "CI",
            "status": "COMPLETED",
            "conclusion": "FAILURE",
            "headBranch": "main",
            "event": "push",
            "url": "https://example.com",
            "jobs": [
                {"name": "lint", "conclusion": "failure"},
                {"name": "build", "conclusion": "success"},
                {"name": "test", "conclusion": "timed_out"}
            ]
        });
        let output = parse_run_view(&parsed);
        assert_eq!(output.failing_jobs, vec!["lint", "test"]);
        assert!(output.message.contains("Failing jobs: lint, test"));
    }

    #[test]
    fn ci_status_no_failing_jobs_when_all_pass() {
        let parsed = serde_json::json!({
            "databaseId": 1,
            "name": "CI",
            "status": "COMPLETED",
            "conclusion": "SUCCESS",
            "headBranch": "main",
            "event": "push",
            "url": "https://example.com",
            "jobs": [
                {"name": "lint", "conclusion": "success"},
                {"name": "build", "conclusion": "success"}
            ]
        });
        let output = parse_run_view(&parsed);
        assert!(output.failing_jobs.is_empty());
        assert!(!output.message.contains("Failing jobs"));
    }

    #[test]
    fn tally_checks_correctly() {
        let rollup = serde_json::json!([
            {"state": "SUCCESS", "name": "build"},
            {"state": "FAILURE", "name": "lint"},
            {"state": "PENDING", "name": "deploy"},
            {"state": "ERROR", "name": "security"},
            {"state": "IN_PROGRESS", "name": "test"},
        ]);
        let (pass, fail, pending, failing) = tally_checks(Some(&rollup));
        assert_eq!(pass, 1);
        assert_eq!(fail, 2);
        assert_eq!(pending, 2);
        assert_eq!(failing, vec!["lint", "security"]);
    }

    #[test]
    fn tally_checks_handles_none() {
        let (pass, fail, pending, failing) = tally_checks(None);
        assert_eq!((pass, fail, pending, failing), (0, 0, 0, Vec::new()));
    }

    #[test]
    fn cap_bytes_truncates() {
        let input = b"x".repeat(200);
        let capped = cap_bytes(input, 50);
        assert!(capped.len() > 50);
        assert!(capped.ends_with(b"[truncated]"));
    }

    #[test]
    fn cap_bytes_no_truncation() {
        let input = b"hello".to_vec();
        let capped = cap_bytes(input, 50);
        assert_eq!(capped, b"hello");
    }

    #[test]
    fn truncate_for_display_within_limit() {
        let s = "hello";
        assert_eq!(truncate_for_display(s, 10), "hello");
    }

    #[test]
    fn truncate_for_display_truncates() {
        let s = "this is a very long string that should be truncated";
        let truncated = truncate_for_display(s, 20);
        assert!(truncated.ends_with("[truncated]"));
        assert!(truncated.len() < s.len());
    }

    // --- Tool metadata tests ---

    use crate::types::tool_metadata::ToolMetadata;

    #[test]
    fn gh_pr_status_tool_metadata() {
        let tool = GhPrStatusTool;
        assert_eq!(tool.kind(), ToolKind::Read);
        assert_eq!(tool.tool_namespace(), ToolNamespace::GrokBuild);
        assert!(tool.is_read_only());
    }

    #[test]
    fn gh_ci_status_tool_metadata() {
        let tool = GhCiStatusTool;
        assert_eq!(tool.kind(), ToolKind::Read);
        assert_eq!(tool.tool_namespace(), ToolNamespace::GrokBuild);
        assert!(tool.is_read_only());
    }

    #[test]
    fn gh_ci_rerun_tool_metadata() {
        let tool = GhCiRerunTool;
        assert_eq!(tool.kind(), ToolKind::Execute);
        assert_eq!(tool.tool_namespace(), ToolNamespace::GrokBuild);
        assert!(!tool.is_read_only());
    }

    // --- Integration tests requiring real `gh` (ignored by default) ---

    #[tokio::test]
    #[ignore = "requires gh CLI + GitHub auth"]
    async fn gh_pr_status_live() {
        let result = run_gh_pr_status(GhPrStatusInput { pr: None }).await;
        assert!(result.is_ok(), "gh is installed and authenticated");
    }

    #[tokio::test]
    #[ignore = "requires gh CLI + GitHub auth"]
    async fn gh_ci_status_live() {
        let result = run_gh_ci_status(GhCiStatusInput { run: None }).await;
        assert!(result.is_ok(), "gh is installed and authenticated");
    }

    #[tokio::test]
    #[ignore = "requires gh CLI + GitHub auth + a real run ID"]
    async fn gh_ci_rerun_live() {
        // This test is destructive — only runs manually.
    }
}
