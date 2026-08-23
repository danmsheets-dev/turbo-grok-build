//! `turbo schedule` — list / show / cancel standing jobs with Turbo closed.
//!
//! Reads `{workspace}/.grok/schedules.json` (the actor writes this for
//! `/schedule` product jobs). Fires still only happen while the pager is up.

use anyhow::{Result, bail};
use clap::Subcommand;
use std::path::{Path, PathBuf};
use xai_grok_tools::implementations::grok_build::scheduler::disk::{
    ScheduleIndex, cancel_task, load_schedule_index, schedule_index_path, visible_tasks,
};
use xai_grok_tools::implementations::grok_build::scheduler::interval::task_human_schedule;
use xai_grok_tools::implementations::grok_build::scheduler::types::ScheduledTask;

#[derive(Debug, clap::Args, Clone)]
pub struct ScheduleArgs {
    #[command(subcommand)]
    pub command: ScheduleCliCommand,
}

#[derive(Debug, Subcommand, Clone)]
pub enum ScheduleCliCommand {
    /// List standing `/schedule` jobs from the workspace index
    List {
        /// Emit JSON
        #[arg(long)]
        json: bool,
        /// Workspace root (default: process cwd)
        #[arg(long, value_name = "PATH")]
        cwd: Option<PathBuf>,
    },
    /// Show one job by id
    Show {
        id: String,
        #[arg(long)]
        json: bool,
        #[arg(long, value_name = "PATH")]
        cwd: Option<PathBuf>,
    },
    /// Cancel a job (writes a tombstone so a running pager will not resurrect it)
    Cancel {
        id: String,
        #[arg(long, value_name = "PATH")]
        cwd: Option<PathBuf>,
    },
}

pub fn run(args: ScheduleArgs) -> Result<()> {
    match args.command {
        ScheduleCliCommand::List { json, cwd } => list(json, cwd.as_deref()),
        ScheduleCliCommand::Show { id, json, cwd } => show(&id, json, cwd.as_deref()),
        ScheduleCliCommand::Cancel { id, cwd } => cancel(&id, cwd.as_deref()),
    }
}

fn workspace(cwd: Option<&Path>) -> Result<PathBuf> {
    match cwd {
        Some(p) => Ok(p.to_path_buf()),
        None => std::env::current_dir().map_err(|e| anyhow::anyhow!("current dir: {e}")),
    }
}

fn load(cwd: Option<&Path>) -> Result<(PathBuf, ScheduleIndex)> {
    let root = workspace(cwd)?;
    let index = load_schedule_index(&root)?;
    Ok((root, index))
}

fn list(json: bool, cwd: Option<&Path>) -> Result<()> {
    let (root, index) = load(cwd)?;
    let tasks = visible_tasks(&index);
    if json {
        println!("{}", serde_json::to_string_pretty(&tasks)?);
        return Ok(());
    }
    if tasks.is_empty() {
        println!(
            "No standing /schedule jobs in {}.",
            schedule_index_path(&root).display()
        );
        println!(
            "Create one with `/schedule` while Turbo is running. Fires only when the pager is up."
        );
        return Ok(());
    }
    println!("{:<12} {:<10} {:<28} {}", "ID", "WHEN", "NEXT", "TITLE");
    for t in tasks {
        let title = t
            .title
            .as_deref()
            .or_else(|| t.prompt.lines().next())
            .unwrap_or("")
            .chars()
            .take(48)
            .collect::<String>();
        println!(
            "{:<12} {:<10} {:<28} {}",
            t.id,
            truncate(&task_human_schedule(t), 10),
            t.next_fire_at().format("%Y-%m-%dT%H:%M:%SZ"),
            title
        );
    }
    Ok(())
}

fn show(id: &str, json: bool, cwd: Option<&Path>) -> Result<()> {
    let (_root, index) = load(cwd)?;
    let Some(task) = visible_tasks(&index)
        .into_iter()
        .find(|t| t.id == id)
        .cloned()
    else {
        bail!("no standing job {id}");
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&task)?);
        return Ok(());
    }
    print_task(&task);
    Ok(())
}

fn print_task(task: &ScheduledTask) {
    println!("id:            {}", task.id);
    println!("title:         {}", task.title.as_deref().unwrap_or("-"));
    println!("when:          {}", task_human_schedule(task));
    println!("next:          {}", task.next_fire_at().to_rfc3339());
    println!(
        "last:          {}",
        task.last_fired_at
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| "-".into())
    );
    println!("standing:      {}", task.standing);
    println!("durable:       {}", task.durable);
    println!("meeting_join:  {}", task.meeting_join);
    println!(
        "expires:       {}",
        task.expires_at
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| "never".into())
    );
    println!("prompt:\n{}", task.prompt);
}

fn cancel(id: &str, cwd: Option<&Path>) -> Result<()> {
    let root = workspace(cwd)?;
    let existed = cancel_task(&root, id)?;
    if existed {
        println!(
            "Cancelled {id} in {}.",
            schedule_index_path(&root).display()
        );
    } else {
        println!(
            "Recorded cancel tombstone for {id} in {} (job was not in the index).",
            schedule_index_path(&root).display()
        );
    }
    println!(
        "If Turbo is running, the next fire tick will drop it. Fires never run while the pager is closed."
    );
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Command, PagerArgs};
    use clap::Parser as _;
    use xai_grok_tools::implementations::grok_build::scheduler::disk::upsert_live_tasks;
    use xai_grok_tools::implementations::grok_build::scheduler::types::ScheduledTask;

    fn parse_schedule(argv: &[&str]) -> ScheduleCliCommand {
        let args = PagerArgs::try_parse_from(argv).expect("args should parse");
        match args.command {
            Some(Command::Schedule(ScheduleArgs { command })) => command,
            other => panic!("expected schedule, got {other:?}"),
        }
    }

    #[test]
    fn parses_list_show_cancel() {
        match parse_schedule(&["turbo", "schedule", "list", "--json"]) {
            ScheduleCliCommand::List { json, .. } => assert!(json),
            other => panic!("{other:?}"),
        }
        match parse_schedule(&["turbo", "schedule", "show", "abc123"]) {
            ScheduleCliCommand::Show { id, json, .. } => {
                assert_eq!(id, "abc123");
                assert!(!json);
            }
            other => panic!("{other:?}"),
        }
        match parse_schedule(&["turbo", "schedule", "cancel", "abc123"]) {
            ScheduleCliCommand::Cancel { id, .. } => assert_eq!(id, "abc123"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn list_and_cancel_against_workspace_index() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut task = ScheduledTask::new(3600, "search rust async".into(), true, true);
        task.id = "jobdeadbeef1".into();
        task.apply_standing();
        task.title = Some("rust async".into());
        upsert_live_tasks(dir.path(), &[task]).unwrap();

        list(false, Some(dir.path())).unwrap();
        cancel("jobdeadbeef1", Some(dir.path())).unwrap();
        let idx = load_schedule_index(dir.path()).unwrap();
        assert!(visible_tasks(&idx).is_empty());
        assert!(idx.cancelled.iter().any(|c| c == "jobdeadbeef1"));
    }
}
