//! `gh` CLI transport. No GitHub API crate.
//!
//! Follows `xai-grok-tools` gh tools: `which::which("gh")`, `GH_NO_BROWSER=1`,
//! `CI=1`, `xai_tty_utils::detach_command` (CREATE_NO_WINDOW), timeout, `--`
//! before user ids.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use wait_timeout::ChildExt;

use super::mapping::{LogKind, RemoteState};
use super::{GithubTransport, IssueDraft, RemoteIssue, RepoMeta, SyncError};

const GH_TIMEOUT: Duration = Duration::from_secs(30);
const GH_LIST_TIMEOUT: Duration = Duration::from_secs(45);
const GH_OUTPUT_CAP: usize = 2_000_000;

/// Live `gh` CLI.
#[derive(Debug, Default, Clone, Copy)]
pub struct GhCli;

impl GhCli {
    pub fn new() -> Self {
        Self
    }
}

impl GithubTransport for GhCli {
    fn ensure_gh_and_auth(&self) -> Result<(), SyncError> {
        resolve_gh()?;
        let out = run_gh(&["auth".into(), "status".into()], GH_TIMEOUT)?;
        if out.exit_code != 0 {
            let msg = String::from_utf8_lossy(&out.stderr);
            return Err(SyncError::GhUnauthenticated(msg.trim().to_string()));
        }
        Ok(())
    }

    fn repo_meta(&self, repo: &str) -> Result<RepoMeta, SyncError> {
        let argv = vec![
            "repo".into(),
            "view".into(),
            "--json".into(),
            "isPrivate,nameWithOwner,url,hasIssuesEnabled,viewerPermission,isFork,isArchived"
                .into(),
            "--".into(),
            repo.to_string(),
        ];
        let out = run_gh(&argv, GH_TIMEOUT)?;
        if out.exit_code != 0 {
            return Err(gh_fail("repo view", &out));
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            is_private: bool,
            name_with_owner: String,
            url: String,
            // `#[serde(default)]` on each capability field: an older `gh` that
            // does not know one of these must not turn a working sync into a
            // parse error. Absent reads as "no reason to refuse".
            #[serde(default = "yes")]
            has_issues_enabled: bool,
            #[serde(default)]
            viewer_permission: Option<String>,
            #[serde(default)]
            is_fork: bool,
            #[serde(default)]
            is_archived: bool,
        }
        fn yes() -> bool {
            true
        }
        let raw: Raw = serde_json::from_slice(&out.stdout)?;
        Ok(RepoMeta {
            name_with_owner: raw.name_with_owner,
            is_private: raw.is_private,
            url: raw.url,
            has_issues_enabled: raw.has_issues_enabled,
            viewer_permission: raw.viewer_permission.filter(|p| !p.trim().is_empty()),
            is_fork: raw.is_fork,
            is_archived: raw.is_archived,
        })
    }

    fn list_issues(&self, repo: &str, kind: LogKind) -> Result<Vec<RemoteIssue>, SyncError> {
        let argv = vec![
            "issue".into(),
            "list".into(),
            "--repo".into(),
            repo.to_string(),
            "--label".into(),
            kind.type_label().to_string(),
            "--state".into(),
            "all".into(),
            "--limit".into(),
            "200".into(),
            "--json".into(),
            "number,title,state,body,labels,url".into(),
        ];
        let out = run_gh(&argv, GH_LIST_TIMEOUT)?;
        if out.exit_code != 0 {
            return Err(gh_fail("issue list", &out));
        }
        parse_issue_list(&out.stdout)
    }

    fn search_fingerprint(
        &self,
        repo: &str,
        fingerprint: &str,
    ) -> Result<Option<RemoteIssue>, SyncError> {
        if fingerprint.starts_with('-') || fingerprint.is_empty() {
            return Ok(None);
        }
        let argv = vec![
            "issue".into(),
            "list".into(),
            "--repo".into(),
            repo.to_string(),
            "--state".into(),
            "all".into(),
            "--limit".into(),
            "20".into(),
            "--search".into(),
            format!("{fingerprint} in:body"),
            "--json".into(),
            "number,title,state,body,labels,url".into(),
        ];
        let out = run_gh(&argv, GH_LIST_TIMEOUT)?;
        if out.exit_code != 0 {
            return Err(gh_fail("issue search", &out));
        }
        let issues = parse_issue_list(&out.stdout)?;
        Ok(issues.into_iter().find(|i| {
            i.body.contains(fingerprint)
                || i.labels.iter().any(|l| l == &format!("fp:{fingerprint}"))
        }))
    }

    fn ensure_label(&self, repo: &str, name: &str) -> Result<(), SyncError> {
        if name.is_empty() || name.starts_with('-') || name.len() > 50 {
            return Ok(());
        }
        let argv = vec![
            "label".into(),
            "create".into(),
            name.to_string(),
            "--repo".into(),
            repo.to_string(),
            "--force".into(),
            "--color".into(),
            label_color(name).into(),
        ];
        let out = run_gh(&argv, GH_TIMEOUT)?;
        if out.exit_code != 0 {
            tracing::debug!(
                label = name,
                stderr = %String::from_utf8_lossy(&out.stderr),
                "gh label create failed (continuing)"
            );
        }
        Ok(())
    }

    fn create_issue(&self, repo: &str, draft: &IssueDraft) -> Result<RemoteIssue, SyncError> {
        for l in &draft.labels {
            let _ = self.ensure_label(repo, l);
        }
        let body_file = write_temp_body(&draft.body)?;
        let mut argv = vec![
            "issue".into(),
            "create".into(),
            "--repo".into(),
            repo.to_string(),
            format!("--title={}", draft.title),
            format!("--body-file={}", body_file.display()),
        ];
        for l in &draft.labels {
            argv.push(format!("--label={l}"));
        }
        let out = run_gh(&argv, GH_TIMEOUT);
        let _ = std::fs::remove_file(&body_file);
        let out = out?;
        if out.exit_code != 0 {
            return Err(gh_fail("issue create", &out));
        }
        let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let number = parse_issue_number_from_url(&url)
            .ok_or_else(|| SyncError::Gh(format!("could not parse issue number from `{url}`")))?;
        if draft.state == RemoteState::Closed {
            self.set_state(repo, number, RemoteState::Closed)?;
        }
        Ok(RemoteIssue {
            number,
            title: draft.title.clone(),
            body: draft.body.clone(),
            state: draft.state,
            labels: draft.labels.clone(),
            url,
        })
    }

    fn update_issue(
        &self,
        repo: &str,
        number: u64,
        draft: &IssueDraft,
        add_labels: &[String],
        remove_labels: &[String],
    ) -> Result<RemoteIssue, SyncError> {
        for l in add_labels {
            let _ = self.ensure_label(repo, l);
        }
        let body_file = write_temp_body(&draft.body)?;
        let mut argv = vec![
            "issue".into(),
            "edit".into(),
            "--repo".into(),
            repo.to_string(),
            format!("--title={}", draft.title),
            format!("--body-file={}", body_file.display()),
        ];
        for l in add_labels {
            argv.push(format!("--add-label={l}"));
        }
        for l in remove_labels {
            argv.push(format!("--remove-label={l}"));
        }
        argv.push("--".into());
        argv.push(number.to_string());
        let out = run_gh(&argv, GH_TIMEOUT);
        let _ = std::fs::remove_file(&body_file);
        let out = out?;
        if out.exit_code != 0 {
            return Err(gh_fail("issue edit", &out));
        }
        self.set_state(repo, number, draft.state)?;
        Ok(RemoteIssue {
            number,
            title: draft.title.clone(),
            body: draft.body.clone(),
            state: draft.state,
            labels: draft.labels.clone(),
            url: format!("https://github.com/{repo}/issues/{number}"),
        })
    }

    fn comment(&self, repo: &str, number: u64, body: &str) -> Result<(), SyncError> {
        let body_file = write_temp_body(body)?;
        let argv = vec![
            "issue".into(),
            "comment".into(),
            "--repo".into(),
            repo.to_string(),
            format!("--body-file={}", body_file.display()),
            "--".into(),
            number.to_string(),
        ];
        let out = run_gh(&argv, GH_TIMEOUT);
        let _ = std::fs::remove_file(&body_file);
        let out = out?;
        if out.exit_code != 0 {
            return Err(gh_fail("issue comment", &out));
        }
        Ok(())
    }

    fn set_state(&self, repo: &str, number: u64, state: RemoteState) -> Result<(), SyncError> {
        let verb = match state {
            RemoteState::Open => "reopen",
            RemoteState::Closed => "close",
        };
        let argv = vec![
            "issue".into(),
            verb.into(),
            "--repo".into(),
            repo.to_string(),
            "--".into(),
            number.to_string(),
        ];
        let out = run_gh(&argv, GH_TIMEOUT)?;
        if out.exit_code != 0 {
            let stderr = String::from_utf8_lossy(&out.stderr);
            // Idempotent: already closed / already open is success.
            if stderr.contains("already")
                || stderr.contains("not open")
                || stderr.contains("not closed")
            {
                return Ok(());
            }
            return Err(gh_fail(&format!("issue {verb}"), &out));
        }
        Ok(())
    }
}

struct GhOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: i32,
}

fn resolve_gh() -> Result<PathBuf, SyncError> {
    which::which("gh").map_err(|_| SyncError::GhMissing)
}

fn run_gh(argv: &[String], timeout: Duration) -> Result<GhOutput, SyncError> {
    let gh = resolve_gh()?;
    let mut cmd = Command::new(&gh);
    cmd.args(argv)
        .env("GH_NO_BROWSER", "1")
        .env("CI", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    xai_tty_utils::detach_std_command(&mut cmd);

    #[allow(clippy::disallowed_methods)] // waited on with timeout; enrolled in global ProcessScope
    let mut child = cmd
        .spawn()
        .map_err(|e| SyncError::Gh(format!("spawn gh: {e}")))?;

    let mut group = xai_tty_utils::ProcessGroup::new()
        .map_err(|e| SyncError::Gh(format!("process group: {e}")))?;
    if let Err(e) = group.attach_std(&child) {
        tracing::debug!(error = %e, "gh process group attach failed");
    }
    let group = Arc::new(group);
    let _ = xai_tty_utils::global_process_scope().register(&group);

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let t_out = std::thread::spawn(move || read_capped(stdout, GH_OUTPUT_CAP));
    let t_err = std::thread::spawn(move || read_capped(stderr, GH_OUTPUT_CAP));

    let wait = child.wait_timeout(timeout);
    let status = match wait {
        Ok(Some(st)) => st,
        Ok(None) => {
            let _ = group.kill();
            let _ = child.kill();
            let _ = child.wait();
            return Err(SyncError::Timeout(timeout.as_secs()));
        }
        Err(e) => {
            let _ = group.kill();
            let _ = child.kill();
            let _ = child.wait();
            return Err(SyncError::Gh(format!("wait gh: {e}")));
        }
    };

    let stdout = t_out.join().unwrap_or_default();
    let stderr = t_err.join().unwrap_or_default();
    Ok(GhOutput {
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
    let mut tmp = [0u8; 8192];
    loop {
        match r.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                let remain = cap.saturating_sub(buf.len());
                if remain == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n.min(remain)]);
                if n > remain {
                    buf.extend_from_slice(b"\n[truncated]");
                    break;
                }
            }
            Err(_) => break,
        }
    }
    buf
}

fn write_temp_body(body: &str) -> Result<PathBuf, SyncError> {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "turbo-log-gh-{}-{}.md",
        std::process::id(),
        uuid::Uuid::now_v7().simple()
    ));
    let mut f = std::fs::File::create(&path)?;
    f.write_all(body.as_bytes())?;
    Ok(path)
}

fn parse_issue_number_from_url(url: &str) -> Option<u64> {
    url.trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .and_then(|s| s.parse().ok())
}

fn gh_fail(op: &str, out: &GhOutput) -> SyncError {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let msg = if !stderr.trim().is_empty() {
        stderr.trim().to_string()
    } else {
        stdout.trim().to_string()
    };
    if msg.to_ascii_lowercase().contains("rate limit") {
        return SyncError::RateLimited(msg);
    }
    SyncError::Gh(format!("{op}: {msg}"))
}

fn parse_issue_list(stdout: &[u8]) -> Result<Vec<RemoteIssue>, SyncError> {
    #[derive(Deserialize)]
    struct Raw {
        number: u64,
        title: String,
        state: String,
        #[serde(default)]
        body: String,
        #[serde(default)]
        url: String,
        #[serde(default)]
        labels: Vec<RawLabel>,
    }
    #[derive(Deserialize)]
    struct RawLabel {
        name: String,
    }
    let raw: Vec<Raw> = serde_json::from_slice(stdout)?;
    Ok(raw
        .into_iter()
        .map(|r| RemoteIssue {
            number: r.number,
            title: r.title,
            body: r.body,
            state: if r.state.eq_ignore_ascii_case("closed") {
                RemoteState::Closed
            } else {
                RemoteState::Open
            },
            labels: r.labels.into_iter().map(|l| l.name).collect(),
            url: r.url,
        })
        .collect())
}

fn label_color(name: &str) -> &'static str {
    match name {
        "type:incident" | "p0" | "must_have" => "b60205",
        "p1" | "should_have" => "d93f0b",
        "p2" | "nice_to_have" | "acknowledged" => "fbca04",
        "p3" | "resolved" | "shipped" => "0e8a16",
        "type:feature" | "planned" => "1d76db",
        "declined" => "5319e7",
        "exploratory" => "c5def5",
        _ if name.starts_with("class:") => "d4c5f9",
        _ if name.starts_with("component:") => "bfdadc",
        _ if name.starts_with("fp:") => "ededed",
        _ => "ededed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_issue_url() {
        assert_eq!(
            parse_issue_number_from_url("https://github.com/o/n/issues/12\n"),
            Some(12)
        );
    }

    #[test]
    fn parse_list_json() {
        let raw = br#"[{"number":1,"title":"t","state":"OPEN","body":"b","url":"u","labels":[{"name":"type:incident"}]}]"#;
        let issues = parse_issue_list(raw).unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].number, 1);
        assert_eq!(issues[0].state, RemoteState::Open);
        assert_eq!(issues[0].labels, vec!["type:incident"]);
    }
}
