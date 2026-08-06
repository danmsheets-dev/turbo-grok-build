use std::io::{IsTerminal as _, Write};
use std::path::Path;

use anyhow::Result;

use crate::diagnostics::{DiagnosticReport, FixActivation, FixPlan, ShellKind};

mod human;
mod json;

pub const SCHEMA_VERSION: &str = "1";

#[derive(Clone, Debug, Default, Eq, PartialEq, clap::Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct DoctorArgs {
    /// Print the diagnostic report as JSON.
    #[arg(long)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Option<DoctorCommand>,
}

#[derive(Clone, Debug, Eq, PartialEq, clap::Subcommand)]
pub enum DoctorCommand {
    /// Apply an automatic fix.
    Fix(FixArgs),
}

#[derive(Clone, Debug, Eq, PartialEq, clap::Args)]
pub struct FixArgs {
    /// Named fix to apply. Omit it to list available automatic fixes.
    pub id: Option<String>,
    /// Apply the displayed changes without confirmation.
    #[arg(long, requires = "id")]
    pub yes: bool,
}

pub fn run(args: DoctorArgs) -> Result<()> {
    match args.command {
        None => run_report(args.json, &mut std::io::stdout().lock()),
        Some(DoctorCommand::Fix(fix)) => run_fix(
            fix,
            std::io::stdin().is_terminal(),
            &mut std::io::stdin().lock(),
            &mut std::io::stdout().lock(),
        ),
    }
}

pub fn run_with_writer(args: DoctorArgs, writer: &mut impl Write) -> Result<()> {
    match args.command {
        None => run_report(args.json, writer),
        Some(_) => anyhow::bail!("Doctor fixes require interactive input and output."),
    }
}

fn run_report(json_output: bool, writer: &mut impl Write) -> Result<()> {
    let report = collect_report();
    write_report(&report, json_output, writer)
}

pub fn collect_report() -> DiagnosticReport {
    let terminal = crate::terminal::standalone_terminal_context();
    let report = collect_report_with(crate::diagnostics::probes::collect_standalone(&terminal));
    configured_report_for_terminal(report, &terminal)
}

fn configured_report_for_terminal(
    report: DiagnosticReport,
    terminal: &crate::terminal::TerminalContext,
) -> DiagnosticReport {
    if terminal.is_ssh || terminal.is_official_vscode_remote {
        return report;
    }
    let configured = shell_home_and_kind().is_some_and(|(home, shell)| {
        crate::diagnostics::managed_alias_configured(&shell.config_path(&home), shell)
    });
    crate::diagnostics::configured_report(report, configured)
}

/// Pure snapshot→report composition. Reads nothing but the snapshot.
///
/// Split out of [`collect_report_with`] so tests can compose a report from
/// synthetic probe facts without the live host probes below reaching for this
/// machine's audio devices or `~/.grok/config.toml`.
fn compose_report(
    snapshot: crate::diagnostics::probes::StandaloneDiagnosticSnapshot<'_>,
) -> DiagnosticReport {
    crate::diagnostics::view(snapshot.into())
}

/// The two probes that read this machine rather than the snapshot.
///
/// Behind a struct of function pointers purely so tests can substitute fakes:
/// the property worth pinning is that the probes *transit* the composed report
/// and only append to it, and that property is about the plumbing, not about
/// what audio hardware the test host happens to have.
struct LiveProbes {
    /// Appends the voice fact, plus a finding when no input device is present.
    voice: fn(&mut DiagnosticReport, bool),
    /// Appends the oracle model-pin recommendations.
    oracle: fn(&mut DiagnosticReport, Option<&str>),
}

/// The probes production runs. The only place they are named.
const LIVE_PROBES: LiveProbes = LiveProbes {
    voice: crate::diagnostics::apply_voice_probe,
    oracle: crate::diagnostics::apply_oracle_model_pin_probe,
};

/// [`compose_report`] plus the live host probes every production caller needs.
///
/// Both probes only ever *append* to the composed report — they add a voice
/// fact and findings, never remove or reorder what the view produced.
fn collect_report_with(
    snapshot: crate::diagnostics::probes::StandaloneDiagnosticSnapshot<'_>,
) -> DiagnosticReport {
    collect_report_with_probes(snapshot, &LIVE_PROBES)
}

/// [`collect_report_with`] with the host reads injected.
fn collect_report_with_probes(
    snapshot: crate::diagnostics::probes::StandaloneDiagnosticSnapshot<'_>,
    probes: &LiveProbes,
) -> DiagnosticReport {
    let mut report = compose_report(snapshot);
    (probes.voice)(&mut report, true);
    // No live session headless — the same-as-session arm needs a model id the
    // standalone report does not have.
    (probes.oracle)(&mut report, None);
    report
}

fn write_report(
    report: &DiagnosticReport,
    json_output: bool,
    writer: &mut impl Write,
) -> Result<()> {
    if json_output {
        json::write(report, writer)
    } else {
        write!(writer, "{}", human::format(report))?;
        Ok(())
    }
}

fn run_fix(
    args: FixArgs,
    stdin_is_terminal: bool,
    input: &mut impl std::io::BufRead,
    writer: &mut impl Write,
) -> Result<()> {
    let terminal = crate::terminal::standalone_terminal_context();
    let Some(value) = args.id.as_deref() else {
        let report = configured_report_for_terminal(
            collect_report_with(crate::diagnostics::probes::collect_standalone_fix(
                &terminal, None,
            )),
            &terminal,
        );
        write!(
            writer,
            "{}",
            crate::diagnostics::format_applicable_automatic_fixes(&report, &terminal)
        )?;
        return Ok(());
    };
    let id = crate::diagnostics::resolve_fix_id(value)?;
    let report = configured_report_for_terminal(
        collect_report_with(crate::diagnostics::probes::collect_standalone_fix(
            &terminal,
            Some(id),
        )),
        &terminal,
    );
    let request = crate::diagnostics::FixRequest::from_environment(id)?;
    let plan = crate::diagnostics::plan_fix(request, &report, &terminal)?;
    apply_fix_plan(args, stdin_is_terminal, input, writer, &terminal, plan)
}

fn apply_fix_plan(
    args: FixArgs,
    stdin_is_terminal: bool,
    input: &mut impl std::io::BufRead,
    writer: &mut impl Write,
    terminal: &crate::terminal::TerminalContext,
    plan: FixPlan,
) -> Result<()> {
    write_fix_preview(&plan, writer)?;

    if !args.yes {
        if !stdin_is_terminal {
            anyhow::bail!(
                "Cannot apply this fix without confirmation. Run it in an interactive terminal or add `--yes`."
            );
        }
        write!(writer, "\nApply this fix? [y/N] ")?;
        writer.flush()?;
        let mut answer = String::new();
        input.read_line(&mut answer)?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            writeln!(writer, "Fix cancelled.")?;
            return Ok(());
        }
    }

    let outcome = crate::diagnostics::apply_fix(plan)?;
    if outcome.activation() == FixActivation::SatisfiedNow {
        // Use the shell stored on the outcome (from planning), not `$SHELL`.
        // `$SHELL` may be missing or no longer match the shell the plan targeted.
        let post_report = crate::diagnostics::configured_report(
            collect_report_with(crate::diagnostics::probes::collect_standalone(terminal)),
            outcome.managed_alias_is_configured(),
        );
        if post_report
            .findings
            .iter()
            .any(|finding| finding.id == outcome.id())
        {
            anyhow::bail!(
                "The change was applied, but Doctor still reports `{}`.",
                outcome.id()
            );
        }
    } else if !crate::diagnostics::verify_persistent_fix(&outcome) {
        anyhow::bail!(
            "The change was applied, but Doctor could not verify `{}` in persistent configuration.",
            outcome.id()
        );
    }

    writeln!(
        writer,
        "\n{}",
        crate::diagnostics::format_fix_success(&outcome)
    )?;
    Ok(())
}

fn write_fix_preview(plan: &FixPlan, writer: &mut impl Write) -> std::io::Result<()> {
    write!(writer, "{}", crate::diagnostics::format_fix_preview(plan))
}

fn shell_home_and_kind() -> Option<(std::path::PathBuf, ShellKind)> {
    #[allow(deprecated)]
    let home = std::env::home_dir()?;
    let shell = std::env::var_os("SHELL")?;
    let kind = ShellKind::from_shell_path(Path::new(&shell))?;
    Some((home, kind))
}

#[cfg(test)]
mod tests;
