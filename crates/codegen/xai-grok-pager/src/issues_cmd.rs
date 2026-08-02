//! `turbo issues` — list / show / export / resolve Auto Developer Log incidents.

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use xai_grok_developer_log::{
    DIR_ENV, DeveloperLogStore, Environment, ErrorClass, ExportOptions, IncidentStatus,
    ListFilter, ReportRequest, ReporterKind, Severity, Source, clear_configured_dir,
    config_file_path, export_pack, root_resolution_note, set_configured_dir,
};

#[derive(Debug, clap::Args, Clone)]
pub struct IssuesArgs {
    #[command(subcommand)]
    pub command: IssuesCommand,
}

#[derive(Debug, Subcommand, Clone)]
pub enum IssuesCommand {
    /// List product incidents (default: open + acknowledged)
    List {
        /// Emit JSON
        #[arg(long)]
        json: bool,
        /// Filter by severity (repeatable): p0,p1,p2,p3
        #[arg(long = "severity", value_name = "SEV")]
        severity: Vec<String>,
        /// Filter by error_class
        #[arg(long = "class")]
        error_class: Option<String>,
        /// Filter by component tag
        #[arg(long)]
        component: Option<String>,
        /// Include resolved / wontdo
        #[arg(long)]
        all: bool,
        /// Max rows (default unlimited for human, 200 for json)
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Show one incident by id or fingerprint
    Show {
        /// Incident id (`inc_…`) or fingerprint
        id: String,
        /// Emit JSON
        #[arg(long)]
        json: bool,
    },
    /// Export a maintainer pack (summary.md, incidents.ndjson, evidence/)
    Export {
        /// Output directory (default: ~/.grok/developer-log/bundles/export-<ts>/)
        #[arg(long)]
        out: Option<std::path::PathBuf>,
        /// Filter by severity (repeatable)
        #[arg(long = "severity", value_name = "SEV")]
        severity: Vec<String>,
        /// Include resolved / wontdo
        #[arg(long)]
        all: bool,
        /// Only incidents with this error_class
        #[arg(long = "class")]
        error_class: Option<String>,
    },
    /// Mark an incident resolved
    Resolve {
        id: String,
    },
    /// Mark an incident acknowledged
    Ack {
        id: String,
    },
    /// Print the developer-log root path and how it was resolved
    Path {
        /// Emit JSON
        #[arg(long)]
        json: bool,
    },
    /// Persist where Auto Developer Log incidents are stored
    ///
    /// Writes `$GROK_HOME/developer-log.toml` (`dir = "..."`). Precedence:
    /// process override → env `GROK_DEVELOPER_LOG_DIR` → this config → default
    /// `$GROK_HOME/developer-log`.
    SetDir {
        /// Absolute path (or `~/...`) for the log root directory
        dir: std::path::PathBuf,
    },
    /// Clear a custom log dir and revert to `$GROK_HOME/developer-log`
    ClearDir,
    /// File a product incident from the CLI (human / script fallback when the
    /// agent `developer_log` tool is unavailable)
    #[command(visible_alias = "report")]
    File {
        /// Short title
        #[arg(long)]
        title: String,
        /// Summary of the issue
        #[arg(long)]
        summary: String,
        /// error_class (e.g. feature_gap, land_conflict, tool_schema)
        #[arg(long = "class", default_value = "unknown")]
        error_class: String,
        /// Severity override: p0,p1,p2,p3
        #[arg(long)]
        severity: Option<String>,
        /// Component tags (repeatable)
        #[arg(long = "component")]
        component: Vec<String>,
        /// Suggested fix
        #[arg(long)]
        suggested_fix: Option<String>,
    },
}

pub fn run(args: IssuesArgs) -> Result<()> {
    let store = DeveloperLogStore::default();
    match args.command {
        IssuesCommand::List {
            json,
            severity,
            error_class,
            component,
            all,
            limit,
        } => {
            let filter = ListFilter {
                severity: parse_severities(&severity)?,
                error_class,
                component,
                include_closed: all,
                limit: limit.or(if json { Some(200) } else { None }),
                ..Default::default()
            };
            let entries = store.list(&filter).context("list incidents")?;
            if json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if entries.is_empty() {
                println!(
                    "No open Auto Developer Log incidents under {}.",
                    store.root().display()
                );
                println!("Agents file them with the `developer_log` tool; export with `turbo issues export`.");
            } else {
                println!(
                    "{:<4} {:<6} {:>5} {:<22} {}",
                    "SEV", "STATUS", "COUNT", "CLASS", "TITLE"
                );
                for e in &entries {
                    println!(
                        "{:<4} {:<6} {:>5} {:<22} {}  ({})",
                        e.severity.as_str().to_ascii_uppercase(),
                        e.status.as_str(),
                        e.occurrence_count,
                        e.error_class,
                        e.title,
                        e.incident_id,
                    );
                }
                println!(
                    "\n{} incident(s). Show: turbo issues show <id> · Export: turbo issues export",
                    entries.len()
                );
            }
            Ok(())
        }
        IssuesCommand::Show { id, json } => {
            let inc = store.get(&id).with_context(|| format!("get incident `{id}`"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&inc)?);
            } else {
                println!("# {} [{}]", inc.title, inc.severity.as_str().to_ascii_uppercase());
                println!();
                println!("- id:           {}", inc.incident_id);
                println!("- fingerprint:  {}", inc.fingerprint);
                println!("- class:        {}", inc.error_class);
                println!("- kind:         {}", inc.kind);
                println!("- status:       {}", inc.status);
                println!("- occurrences:  {}", inc.occurrence_count);
                println!("- first_seen:   {}", inc.first_seen.to_rfc3339());
                println!("- last_seen:    {}", inc.last_seen.to_rfc3339());
                if !inc.component.is_empty() {
                    println!("- components:   {}", inc.component.join(", "));
                }
                println!();
                println!("## Summary\n\n{}\n", inc.summary);
                if !inc.repro.steps.is_empty() {
                    println!("## Repro\n");
                    for (i, step) in inc.repro.steps.iter().enumerate() {
                        println!("{}. {step}", i + 1);
                    }
                    if let Some(ref e) = inc.repro.expected {
                        println!("\nExpected: {e}");
                    }
                    if let Some(ref a) = inc.repro.actual {
                        println!("Actual:   {a}");
                    }
                    println!();
                }
                if let Some(ref fix) = inc.suggested_fix {
                    println!("## Suggested fix\n\n{fix}\n");
                }
                if let Some(ref meta) = inc.evidence.meta_path {
                    println!("Evidence meta: {meta}");
                }
                if let Some(ref snap) = inc.evidence.snapshot_ref {
                    println!("Snapshot ref:  {snap}");
                }
            }
            Ok(())
        }
        IssuesCommand::Export {
            out,
            severity,
            all,
            error_class,
        } => {
            let filter = ListFilter {
                severity: parse_severities(&severity)?,
                error_class,
                include_closed: all,
                ..Default::default()
            };
            let result = export_pack(
                &store,
                ExportOptions {
                    filter,
                    out_dir: out,
                },
            )
            .context("export pack")?;
            println!(
                "Exported {} incident(s) to {}",
                result.incident_count,
                result.out_dir.display()
            );
            println!("  summary:  {}", result.summary_path.display());
            println!("  ndjson:   {}", result.ndjson_path.display());
            println!("  manifest: {}", result.manifest_path.display());
            Ok(())
        }
        IssuesCommand::Resolve { id } => {
            let inc = store
                .set_status(&id, IncidentStatus::Resolved)
                .with_context(|| format!("resolve `{id}`"))?;
            println!("Resolved {} ({})", inc.incident_id, inc.title);
            Ok(())
        }
        IssuesCommand::Ack { id } => {
            let inc = store
                .set_status(&id, IncidentStatus::Acknowledged)
                .with_context(|| format!("ack `{id}`"))?;
            println!("Acknowledged {} ({})", inc.incident_id, inc.title);
            Ok(())
        }
        IssuesCommand::Path { json } => {
            let root = store.root();
            let note = root_resolution_note();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "root": root.display().to_string(),
                        "resolution": note,
                        "config_file": config_file_path().display().to_string(),
                        "env": DIR_ENV,
                        "enabled": xai_grok_developer_log::is_enabled(),
                    }))?
                );
            } else {
                println!("{}", root.display());
                println!("resolved via: {note}");
                println!("config file:  {}", config_file_path().display());
                println!("env override: {DIR_ENV}");
                println!(
                    "enabled:      {} (set {DIR_ENV}=0 to disable)",
                    xai_grok_developer_log::is_enabled()
                );
            }
            Ok(())
        }
        IssuesCommand::SetDir { dir } => {
            let path = set_configured_dir(&dir).context("set developer log dir")?;
            println!("Auto Developer Log directory set to:\n  {}", path.display());
            println!(
                "Saved in {}\nAgents will write product incidents here. Review with `turbo issues list`.",
                config_file_path().display()
            );
            Ok(())
        }
        IssuesCommand::ClearDir => {
            clear_configured_dir().context("clear developer log dir")?;
            let store = DeveloperLogStore::default();
            println!(
                "Cleared custom log dir. Using default:\n  {}",
                store.root().display()
            );
            Ok(())
        }
        IssuesCommand::File {
            title,
            summary,
            error_class,
            severity,
            component,
            suggested_fix,
        } => {
            let error_class =
                ErrorClass::parse(&error_class).unwrap_or(ErrorClass::Unknown);
            let severity = severity
                .as_deref()
                .and_then(|s| match s.trim().to_ascii_lowercase().as_str() {
                    "p0" => Some(Severity::P0),
                    "p1" => Some(Severity::P1),
                    "p2" => Some(Severity::P2),
                    "p3" => Some(Severity::P3),
                    _ => None,
                });
            let req = ReportRequest {
                title,
                summary,
                error_class,
                severity,
                component,
                suggested_fix,
                source: Source {
                    reporter: ReporterKind::Human,
                    auto: false,
                    tool: Some("turbo issues file".into()),
                    ..Default::default()
                },
                environment: Environment {
                    product_version: Some(xai_grok_version::installed()),
                    os: Some(std::env::consts::OS.into()),
                    arch: Some(std::env::consts::ARCH.into()),
                    ..Default::default()
                },
                ..Default::default()
            };
            let result = store.report(req).context("file incident")?;
            println!(
                "{} {} ({}) — {} occurrence(s)\n  {}",
                if result.is_new { "Filed" } else { "Merged" },
                result.incident_id,
                result.error_class.as_str(),
                result.occurrence_count,
                result.path,
            );
            Ok(())
        }
    }
}

fn parse_severities(raw: &[String]) -> Result<Option<Vec<Severity>>> {
    if raw.is_empty() {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(raw.len());
    for s in raw {
        // allow comma-separated too
        for part in s.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let sev = Severity::parse(part)
                .ok_or_else(|| anyhow::anyhow!("invalid severity `{part}` (use p0..p3)"))?;
            out.push(sev);
        }
    }
    if out.is_empty() {
        bail!("no valid severities");
    }
    Ok(Some(out))
}
