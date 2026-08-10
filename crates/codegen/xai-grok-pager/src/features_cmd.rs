//! `turbo features` — list / show / export Feature Request Log entries.

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use xai_grok_developer_log::{
    FR_DIR_ENV, FeatureRequestReport, FeatureRequestStore, FrExportOptions, FrListFilter,
    ReporterKind, RequestClass, RequestPriority, RequestStatus, Source, export_feature_requests,
    fr_clear_configured_dir, fr_config_file_path, fr_root_resolution_note, fr_set_configured_dir,
};

#[derive(Debug, clap::Args, Clone)]
pub struct FeaturesArgs {
    #[command(subcommand)]
    pub command: FeaturesCommand,
}

#[derive(Debug, Subcommand, Clone)]
pub enum FeaturesCommand {
    /// List product feature requests (default: open / ack / planned)
    List {
        /// Emit JSON
        #[arg(long)]
        json: bool,
        /// Filter by priority (repeatable): must_have, should_have, nice_to_have, exploratory
        #[arg(long = "priority", value_name = "PRI")]
        priority: Vec<String>,
        /// Filter by request_class
        #[arg(long = "class")]
        request_class: Option<String>,
        /// Filter by component tag
        #[arg(long)]
        component: Option<String>,
        /// Include shipped / declined
        #[arg(long)]
        all: bool,
        /// Max rows
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Show one request by id or fingerprint
    Show {
        /// Request id (`fr_…`) or fingerprint
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Export a maintainer pack
    Export {
        #[arg(long)]
        out: Option<std::path::PathBuf>,
        #[arg(long = "priority", value_name = "PRI")]
        priority: Vec<String>,
        #[arg(long)]
        all: bool,
        #[arg(long = "class")]
        request_class: Option<String>,
    },
    /// Mark a request shipped
    Ship {
        id: String,
    },
    /// Mark a request acknowledged
    Ack {
        id: String,
    },
    /// Mark a request planned (roadmap)
    Plan {
        id: String,
    },
    /// Mark a request declined
    Decline {
        id: String,
    },
    /// Print the feature-request-log root path
    Path {
        #[arg(long)]
        json: bool,
    },
    /// Persist where Feature Request Log entries are stored
    SetDir {
        dir: std::path::PathBuf,
    },
    /// Clear a custom log dir and revert to `$GROK_HOME/feature-request-log`
    ClearDir,
    /// File a feature request from the CLI
    #[command(visible_alias = "report")]
    File {
        #[arg(long)]
        title: String,
        #[arg(long)]
        summary: String,
        /// request_class (e.g. tool_surface, scheduler, subagent)
        ///
        /// Aliases match the agent `feature_request_log` tool field `request_class`.
        #[arg(
            long = "class",
            visible_alias = "request-class",
            default_value = "other"
        )]
        request_class: String,
        /// Priority: must_have, should_have, nice_to_have, exploratory
        #[arg(long)]
        priority: Option<String>,
        #[arg(long = "component")]
        component: Vec<String>,
        #[arg(long)]
        use_case: Option<String>,
        #[arg(long)]
        workaround: Option<String>,
        #[arg(long = "proposed")]
        proposed_behavior: Option<String>,
    },
}

pub fn run(args: FeaturesArgs) -> Result<()> {
    let store = FeatureRequestStore::default();
    match args.command {
        FeaturesCommand::List {
            json,
            priority,
            request_class,
            component,
            all,
            limit,
        } => {
            let filter = FrListFilter {
                priority: parse_priorities(&priority)?,
                request_class,
                component,
                include_closed: all,
                limit: limit.or(if json { Some(200) } else { None }),
                ..Default::default()
            };
            let entries = store.list(&filter).context("list feature requests")?;
            if json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if entries.is_empty() {
                println!(
                    "No open Feature Request Log entries under {}.",
                    store.root().display()
                );
                println!(
                    "Agents file them with the `feature_request_log` tool; export with `turbo features export`."
                );
            } else {
                println!(
                    "{:<12} {:<10} {:>5} {:<16} {}",
                    "PRIORITY", "STATUS", "COUNT", "CLASS", "TITLE"
                );
                for e in &entries {
                    println!(
                        "{:<12} {:<10} {:>5} {:<16} {}  ({})",
                        e.priority.as_str(),
                        e.status.as_str(),
                        e.occurrence_count,
                        e.request_class,
                        e.title,
                        e.request_id,
                    );
                }
                println!(
                    "\n{} request(s). Show: turbo features show <id> · Export: turbo features export",
                    entries.len()
                );
            }
            Ok(())
        }
        FeaturesCommand::Show { id, json } => {
            let fr = store
                .get(&id)
                .with_context(|| format!("get feature request `{id}`"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&fr)?);
            } else {
                println!("# {} [{}]", fr.title, fr.priority.as_str());
                println!();
                println!("- id:           {}", fr.request_id);
                println!("- fingerprint:  {}", fr.fingerprint);
                println!("- class:        {}", fr.request_class);
                println!("- priority:     {}", fr.priority);
                println!("- status:       {}", fr.status);
                println!("- occurrences:  {}", fr.occurrence_count);
                println!("- first_seen:   {}", fr.first_seen.to_rfc3339());
                println!("- last_seen:    {}", fr.last_seen.to_rfc3339());
                if !fr.component.is_empty() {
                    println!("- components:   {}", fr.component.join(", "));
                }
                println!();
                println!("## Summary\n\n{}\n", fr.summary);
                if let Some(ref u) = fr.use_case {
                    println!("## Use case\n\n{u}\n");
                }
                if let Some(ref w) = fr.current_workaround {
                    println!("## Current workaround\n\n{w}\n");
                }
                if let Some(ref p) = fr.proposed_behavior {
                    println!("## Proposed behavior\n\n{p}\n");
                }
                if !fr.acceptance_criteria.is_empty() {
                    println!("## Acceptance criteria\n");
                    for c in &fr.acceptance_criteria {
                        println!("- {c}");
                    }
                    println!();
                }
            }
            Ok(())
        }
        FeaturesCommand::Export {
            out,
            priority,
            all,
            request_class,
        } => {
            let filter = FrListFilter {
                priority: parse_priorities(&priority)?,
                request_class,
                include_closed: all,
                ..Default::default()
            };
            let result = export_feature_requests(
                &store,
                FrExportOptions {
                    filter,
                    out_dir: out,
                },
            )
            .context("export feature requests")?;
            println!(
                "Exported {} feature request(s) to {}",
                result.request_count,
                result.out_dir.display()
            );
            println!("  summary:  {}", result.summary_path.display());
            println!("  ndjson:   {}", result.ndjson_path.display());
            println!("  manifest: {}", result.manifest_path.display());
            Ok(())
        }
        FeaturesCommand::Ship { id } => {
            let fr = store
                .set_status(&id, RequestStatus::Shipped)
                .with_context(|| format!("ship `{id}`"))?;
            println!("Shipped {} ({})", fr.request_id, fr.title);
            Ok(())
        }
        FeaturesCommand::Ack { id } => {
            let fr = store
                .set_status(&id, RequestStatus::Acknowledged)
                .with_context(|| format!("ack `{id}`"))?;
            println!("Acknowledged {} ({})", fr.request_id, fr.title);
            Ok(())
        }
        FeaturesCommand::Plan { id } => {
            let fr = store
                .set_status(&id, RequestStatus::Planned)
                .with_context(|| format!("plan `{id}`"))?;
            println!("Planned {} ({})", fr.request_id, fr.title);
            Ok(())
        }
        FeaturesCommand::Decline { id } => {
            let fr = store
                .set_status(&id, RequestStatus::Declined)
                .with_context(|| format!("decline `{id}`"))?;
            println!("Declined {} ({})", fr.request_id, fr.title);
            Ok(())
        }
        FeaturesCommand::Path { json } => {
            let root = store.root();
            let note = fr_root_resolution_note();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "root": root.display().to_string(),
                        "resolution": note,
                        "config_file": fr_config_file_path().display().to_string(),
                        "env": FR_DIR_ENV,
                        "enabled": xai_grok_developer_log::fr_is_enabled(),
                    }))?
                );
            } else {
                println!("{}", root.display());
                println!("resolved via: {note}");
                println!("config file:  {}", fr_config_file_path().display());
                println!("env override: {FR_DIR_ENV}");
                println!(
                    "enabled:      {} (set GROK_FEATURE_REQUEST_LOG=0 to disable)",
                    xai_grok_developer_log::fr_is_enabled()
                );
            }
            Ok(())
        }
        FeaturesCommand::SetDir { dir } => {
            let path = fr_set_configured_dir(&dir).context("set feature request log dir")?;
            println!(
                "Feature Request Log directory set to:\n  {}",
                path.display()
            );
            println!(
                "Saved in {}\nAgents will write capability requests here. Review with `turbo features list`.",
                fr_config_file_path().display()
            );
            Ok(())
        }
        FeaturesCommand::ClearDir => {
            fr_clear_configured_dir().context("clear feature request log dir")?;
            let store = FeatureRequestStore::default();
            println!(
                "Cleared custom feature-request log dir. Using default:\n  {}",
                store.root().display()
            );
            Ok(())
        }
        FeaturesCommand::File {
            title,
            summary,
            request_class,
            priority,
            component,
            use_case,
            workaround,
            proposed_behavior,
        } => {
            let request_class =
                RequestClass::parse(&request_class).unwrap_or(RequestClass::Other);
            let priority = priority
                .as_deref()
                .and_then(RequestPriority::parse);
            let req = FeatureRequestReport {
                title,
                summary,
                request_class,
                priority,
                component,
                use_case,
                current_workaround: workaround,
                proposed_behavior,
                source: Source {
                    reporter: ReporterKind::Human,
                    auto: false,
                    tool: Some("turbo features file".into()),
                    ..Default::default()
                },
                environment: xai_grok_developer_log::Environment {
                    product_version: Some(xai_grok_version::installed()),
                    os: Some(std::env::consts::OS.into()),
                    arch: Some(std::env::consts::ARCH.into()),
                    ..Default::default()
                },
                ..Default::default()
            };
            let result = store.report(req).context("file feature request")?;
            println!(
                "{} {} ({}) — {} occurrence(s)\n  {}",
                if result.is_new { "Created" } else { "Updated" },
                result.request_id,
                result.priority.as_str(),
                result.occurrence_count,
                result.path,
            );
            Ok(())
        }
    }
}

fn parse_priorities(raw: &[String]) -> Result<Option<Vec<RequestPriority>>> {
    if raw.is_empty() {
        return Ok(None);
    }
    let mut out = Vec::new();
    for s in raw {
        let p = RequestPriority::parse(s)
            .ok_or_else(|| anyhow::anyhow!("invalid priority `{s}` (use must_have|should_have|nice_to_have|exploratory)"))?;
        out.push(p);
    }
    if out.is_empty() {
        bail!("no valid priorities");
    }
    Ok(Some(out))
}
