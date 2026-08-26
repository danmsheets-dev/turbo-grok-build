//! `turbo test` — package-scoped cargo tests with OR'd name prefixes.
//!
//! Agents used to pass `foo|bar` as a cargo FILTER (it is a substring, not a
//! regex) or spawn one `cargo test` per prefix (N compiles). This command
//! lists once, ORs `--match` prefixes, and runs matching names in a second
//! cargo invocation that reuses that compile.
//!
//! ```text
//! turbo test --package xai-grok-tools --lib --match spawn_queues --match live_worktree
//! ```
//!
//! Zero matches exits nonzero and hints that `|` is not regex.

use anyhow::{Result, bail};
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Debug, clap::Args, Clone)]
#[command(after_help = "\
Repeated --match PREFIX values are OR'd (name prefix, including any `::` segment).
Cargo's own FILTER is a substring, not a regex (`|` is not regex).

Examples:
  turbo test --package xai-grok-tools --lib --match spawn_queues --match live_worktree
  turbo test -p xai-grok-shell --lib --match prune_soft_preserved -- --test-threads=4
")]
pub struct TestArgs {
    /// Cargo package (`cargo test -p`)
    #[arg(long, short = 'p', value_name = "PKG")]
    pub package: String,
    /// Restrict to library unit tests (`cargo test --lib`)
    #[arg(long)]
    pub lib: bool,
    /// Test-name prefix (repeatable; OR'd). Not a regex.
    #[arg(long = "match", value_name = "PREFIX", required = true)]
    pub match_prefixes: Vec<String>,
    /// Workspace root (default: process cwd)
    #[arg(long, value_name = "PATH")]
    pub root: Option<PathBuf>,
    /// Extra libtest args after `--` (e.g. `--test-threads=4 --nocapture`)
    #[arg(last = true)]
    pub extra: Vec<String>,
}

pub fn run(args: TestArgs) -> Result<()> {
    let prefixes = normalize_prefixes(&args.match_prefixes)?;
    let root = match args.root {
        Some(p) => p,
        None => std::env::current_dir().map_err(|e| anyhow::anyhow!("current dir: {e}"))?,
    };

    let list_output = spawn_cargo(&root, &cargo_list_args(&args.package, args.lib), true)?;
    if !list_output.status.success() {
        bail!(
            "cargo test --list failed with status {}",
            list_output.status
        );
    }
    let listed = parse_listed_tests(&String::from_utf8_lossy(&list_output.stdout));
    let prefix_refs: Vec<&str> = prefixes.iter().map(String::as_str).collect();
    let matched = plan_matches(&listed, &prefix_refs);
    match matched {
        MatchPlan::None => bail!("{}", zero_match_message(&prefix_refs)),
        MatchPlan::Run(names) => {
            eprintln!(
                "turbo test: {} matched for --match {} (one compile)",
                names.len(),
                prefixes
                    .iter()
                    .map(|p| format!("`{p}`"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            let run_output = spawn_cargo(
                &root,
                &cargo_run_args(&args.package, args.lib, &names, &args.extra),
                false,
            )?;
            if !run_output.status.success() {
                bail!("cargo test failed with status {}", run_output.status);
            }
            Ok(())
        }
    }
}

fn cargo_bin() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

fn spawn_cargo(
    root: &std::path::Path,
    args: &[String],
    capture_stdout: bool,
) -> Result<std::process::Output> {
    let mut cmd = Command::new(cargo_bin());
    cmd.current_dir(root).args(args).stderr(Stdio::inherit());
    if capture_stdout {
        cmd.stdout(Stdio::piped());
    } else {
        cmd.stdout(Stdio::inherit());
    }
    cmd.output()
        .map_err(|e| anyhow::anyhow!("failed to run cargo: {e}"))
}

/// Trim, drop empties, keep first-seen order.
pub fn normalize_prefixes(raw: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for p in raw {
        let t = p.trim();
        if t.is_empty() {
            bail!("--match PREFIX must be non-empty");
        }
        if !out.iter().any(|e: &String| e == t) {
            out.push(t.to_string());
        }
    }
    if out.is_empty() {
        bail!("at least one --match PREFIX is required");
    }
    Ok(out)
}

/// Full name or any `::` segment starts with `prefix`. Stricter than cargo substring FILTER.
pub fn test_name_matches_prefix(name: &str, prefix: &str) -> bool {
    if prefix.is_empty() || name.is_empty() {
        return false;
    }
    name.starts_with(prefix) || name.split("::").any(|segment| segment.starts_with(prefix))
}

pub fn filter_tests_by_prefixes<'a>(names: &[&'a str], prefixes: &[&str]) -> Vec<&'a str> {
    names
        .iter()
        .copied()
        .filter(|name| prefixes.iter().any(|p| test_name_matches_prefix(name, p)))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchPlan {
    Run(Vec<String>),
    None,
}

pub fn plan_matches(listed_names: &[String], prefixes: &[&str]) -> MatchPlan {
    let refs: Vec<&str> = listed_names.iter().map(String::as_str).collect();
    let matched = filter_tests_by_prefixes(&refs, prefixes);
    if matched.is_empty() {
        MatchPlan::None
    } else {
        MatchPlan::Run(matched.into_iter().map(str::to_string).collect())
    }
}

/// Parse `cargo test -- --list` stdout into unit/integration test names.
pub fn parse_listed_tests(output: &str) -> Vec<String> {
    output.lines().filter_map(parse_list_line).collect()
}

fn parse_list_line(line: &str) -> Option<String> {
    let line = line.trim();
    let (name, kind) = line.rsplit_once(": ")?;
    if kind != "test" || name.is_empty() || name.contains(char::is_whitespace) {
        return None;
    }
    Some(name.to_string())
}

pub fn zero_match_message(prefixes: &[&str]) -> String {
    let shown = prefixes
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "no tests matched prefix(es): {shown}\n\
         cargo's test FILTER is a substring, not a regex (`|` is not regex).\n\
         Use repeated --match PREFIX to OR name prefixes in one compile:\n\
           turbo test --package <pkg> --lib --match PREFIX --match OTHER"
    )
}

pub fn cargo_list_args(package: &str, lib: bool) -> Vec<String> {
    let mut a = vec!["test".into(), "--package".into(), package.into()];
    if lib {
        a.push("--lib".into());
    }
    a.push("--".into());
    a.push("--list".into());
    a
}

pub fn cargo_run_args(
    package: &str,
    lib: bool,
    exact_names: &[String],
    extra: &[String],
) -> Vec<String> {
    let mut a = vec!["test".into(), "--package".into(), package.into()];
    if lib {
        a.push("--lib".into());
    }
    a.push("--".into());
    a.push("--exact".into());
    a.extend(exact_names.iter().cloned());
    a.extend(extra.iter().cloned());
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Command, PagerArgs};
    use clap::Parser as _;

    fn parse_test(argv: &[&str]) -> TestArgs {
        let args = PagerArgs::try_parse_from(argv).expect("args should parse");
        match args.command {
            Some(Command::Test(t)) => t,
            other => panic!("expected test, got {other:?}"),
        }
    }

    #[test]
    fn prefix_matches_full_name_and_segment() {
        assert!(test_name_matches_prefix(
            "spawn_queues::enqueue_ok",
            "spawn_queues"
        ));
        assert!(test_name_matches_prefix(
            "mod::spawn_queues_works",
            "spawn_queues"
        ));
        assert!(test_name_matches_prefix(
            "spawn_queues_works",
            "spawn_queues"
        ));
        assert!(test_name_matches_prefix("spawn_queues", "spawn_queues"));
    }

    #[test]
    fn prefix_does_not_substring_match() {
        assert!(!test_name_matches_prefix("spawn_queues::enqueue", "queues"));
        assert!(!test_name_matches_prefix(
            "foo_spawn_queues",
            "spawn_queues"
        ));
        assert!(!test_name_matches_prefix("spawn_queues", "spawn_queues::"));
    }

    #[test]
    fn pipe_is_literal_prefix_not_regex() {
        assert!(!test_name_matches_prefix("foo_test", "foo|bar"));
        assert!(!test_name_matches_prefix("bar_test", "foo|bar"));
        assert!(test_name_matches_prefix("foo|bar_test", "foo|bar"));
        assert!(!test_name_matches_prefix("foo", "foo|bar"));
        assert!(!test_name_matches_prefix("bar", "foo|bar"));
    }

    #[test]
    fn or_prefixes_union_names() {
        let names = [
            "spawn_queues::a",
            "live_worktree::b",
            "other::c",
            "mod::live_worktree_seed",
        ];
        let got = filter_tests_by_prefixes(&names, &["spawn_queues", "live_worktree"]);
        assert_eq!(
            got,
            vec![
                "spawn_queues::a",
                "live_worktree::b",
                "mod::live_worktree_seed"
            ]
        );
    }

    #[test]
    fn empty_prefix_or_name_never_matches() {
        assert!(!test_name_matches_prefix("foo", ""));
        assert!(!test_name_matches_prefix("", "foo"));
    }

    #[test]
    fn parse_list_skips_cargo_noise_and_benches() {
        let out = "\
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.05s\r
     Running unittests src/lib.rs (target/debug/deps/foo-abc)\r
spawn_queues::enqueue: test\r
live_worktree::seed: test\r
heavy_bench: bench\r
src/lib.rs - documented (line 4): test\r
1 test, 0 benchmarks\r
";
        assert_eq!(
            parse_listed_tests(out),
            vec!["spawn_queues::enqueue", "live_worktree::seed"]
        );
    }

    #[test]
    fn plan_none_when_only_regex_pipe() {
        let listed = vec!["foo_test".into(), "bar_test".into()];
        assert_eq!(plan_matches(&listed, &["foo|bar"]), MatchPlan::None);
        let msg = zero_match_message(&["foo|bar"]);
        assert!(msg.contains("substring, not a regex"), "{msg}");
        assert!(msg.contains("`|` is not regex"), "{msg}");
        assert!(msg.contains("repeated --match"), "{msg}");
        assert!(msg.contains("`foo|bar`"), "{msg}");
    }

    #[test]
    fn plan_run_ors_and_preserves_list_order() {
        let listed = vec!["alpha::one".into(), "skip::me".into(), "beta::two".into()];
        assert_eq!(
            plan_matches(&listed, &["beta", "alpha"]),
            MatchPlan::Run(vec!["alpha::one".into(), "beta::two".into()])
        );
    }

    #[test]
    fn normalize_trims_and_dedups() {
        let got = normalize_prefixes(&[
            "  spawn_queues  ".into(),
            "live_worktree".into(),
            "spawn_queues".into(),
        ])
        .expect("ok");
        assert_eq!(got, vec!["spawn_queues", "live_worktree"]);
        assert!(normalize_prefixes(&["".into(), "  ".into()]).is_err());
    }

    #[test]
    fn cargo_args_are_one_test_invocation() {
        let list = cargo_list_args("xai-grok-tools", true);
        assert_eq!(
            list,
            vec![
                "test",
                "--package",
                "xai-grok-tools",
                "--lib",
                "--",
                "--list"
            ]
        );
        assert_eq!(list.iter().filter(|a| *a == "test").count(), 1);

        let run = cargo_run_args(
            "xai-grok-tools",
            true,
            &["spawn_queues::a".into(), "live_worktree::b".into()],
            &["--test-threads=4".into()],
        );
        assert_eq!(
            run,
            vec![
                "test",
                "--package",
                "xai-grok-tools",
                "--lib",
                "--",
                "--exact",
                "spawn_queues::a",
                "live_worktree::b",
                "--test-threads=4",
            ]
        );
        assert_eq!(run.iter().filter(|a| *a == "test").count(), 1);
        assert!(!run.iter().any(|a| a == "--list"));
    }

    #[test]
    fn clap_repeatable_match_and_lib() {
        let args = parse_test(&[
            "turbo",
            "test",
            "--package",
            "xai-grok-tools",
            "--lib",
            "--match",
            "spawn_queues",
            "--match",
            "live_worktree",
            "--",
            "--test-threads=4",
        ]);
        assert_eq!(args.package, "xai-grok-tools");
        assert!(args.lib);
        assert_eq!(args.match_prefixes, vec!["spawn_queues", "live_worktree"]);
        assert_eq!(args.extra, vec!["--test-threads=4"]);
    }

    #[test]
    fn clap_short_package_and_pipe_literal() {
        let args = parse_test(&[
            "turbo",
            "test",
            "-p",
            "xai-grok-shell",
            "--match",
            "foo|bar",
        ]);
        assert_eq!(args.package, "xai-grok-shell");
        assert!(!args.lib);
        assert_eq!(args.match_prefixes, vec!["foo|bar"]);
    }

    #[test]
    fn clap_requires_package_and_match() {
        assert!(PagerArgs::try_parse_from(["turbo", "test"]).is_err());
        assert!(PagerArgs::try_parse_from(["turbo", "test", "-p", "pkg"]).is_err());
        assert!(PagerArgs::try_parse_from(["turbo", "test", "--match", "foo"]).is_err());
    }
}
