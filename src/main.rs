//! Command-line entry point: scans a directory, plans the installs, runs them
//! and reports. All filesystem and process I/O lives here.

mod detect;
mod plan;
mod run;
mod ui;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};
use console::style;

use crate::detect::Scan;
use crate::plan::{Plan, Step};
use crate::run::{Outcome, Status};
use crate::ui::{Node, PlanRow, Ui};

#[derive(Parser)]
#[command(
    name = "i",
    version,
    about = "Install project dependencies for the toolchains detected in a directory"
)]
struct Cli {
    /// Directory to install in
    #[arg(
        short = 'C',
        long,
        value_name = "DIR",
        default_value = ".",
        global = true
    )]
    directory: PathBuf,

    /// Print the commands that would run, without running them
    #[arg(short = 'n', long)]
    dry_run: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Clone, Copy)]
enum Command {
    /// Install, streaming every command's output live behind a prefix naming its toolchain
    Log,
}

/// Parses the command line, rejecting combinations clap cannot express: `-C`
/// is accepted on either side of a subcommand, so the conflict between
/// `--dry-run` and `log` is checked after parsing.
fn parse<I, T>(args: I) -> Result<Cli, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = Cli::try_parse_from(args)?;
    if cli.dry_run && cli.command.is_some() {
        return Err(Cli::command().error(
            ErrorKind::ArgumentConflict,
            "--dry-run cannot be used with 'log'",
        ));
    }
    Ok(cli)
}

fn main() -> ExitCode {
    let cli = parse(std::env::args_os()).unwrap_or_else(|error| error.exit());
    match run_cli(&cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{}: {error:#}", style("error").red().bold());
            ExitCode::FAILURE
        }
    }
}

fn run_cli(cli: &Cli) -> Result<ExitCode> {
    let directory = &cli.directory;
    let scan =
        scan(directory).with_context(|| format!("failed to scan {}", directory.display()))?;
    let plan = plan::plan(&detect::detect(&scan));

    if plan.is_empty() {
        eprintln!(
            "{}",
            style(format!(
                "no recognised project files in {}",
                directory.display()
            ))
            .dim()
        );
        return Ok(ExitCode::SUCCESS);
    }

    if cli.dry_run {
        print_plan(&plan);
        return Ok(ExitCode::SUCCESS);
    }

    let log = matches!(cli.command, Some(Command::Log));
    let started = Instant::now();
    let outcomes = install(plan, directory, log);

    if !log {
        ui::details(outcomes.iter().filter(|outcome| outcome.failed()));
    }
    ui::summary(&outcomes, started.elapsed());

    if outcomes.iter().any(Outcome::failed) {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

/// How a run is reported: the plan as a live tree with output held back, or
/// each step's output streamed behind a coloured prefix.
enum Report {
    Tree(Ui),
    Log { prefixes: Vec<String> },
}

impl Report {
    /// Lays out the whole plan before anything runs, so every step is visible
    /// from the start, including those that have to wait.
    fn new(plan: &Plan, log: bool) -> Self {
        let nodes: Vec<Node> = plan
            .dependencies()
            .map(|(step, after)| Node {
                name: step.name,
                after,
            })
            .collect();

        if !log {
            return Report::Tree(Ui::new(&nodes));
        }

        let names: Vec<&str> = nodes.iter().map(|node| node.name).collect();
        let prefixes = ui::prefixes(&names);
        for (node, prefix) in nodes.iter().zip(&prefixes) {
            if let Some(after) = node.after {
                eprintln!("{prefix}{}", style(format!("waiting for {after}")).dim());
            }
        }
        Report::Log { prefixes }
    }

    fn run(&self, index: usize, step: &Step, directory: &Path) -> Outcome {
        match self {
            Report::Tree(ui) => {
                let line = ui.start(index);
                let outcome = run::run(step, directory, &|output| line.activity(output));
                line.finish(&outcome);
                outcome
            }
            Report::Log { prefixes } => {
                let prefix = &prefixes[index];
                eprintln!(
                    "{prefix}{}",
                    style(format!("$ {}", step.command_line())).dim()
                );
                let outcome = run::run(step, directory, &|output| eprintln!("{prefix}{output}"));
                if let Status::Skipped { reason } = &outcome.status {
                    eprintln!("{prefix}{}", style(format!("skipped: {reason}")).dim());
                }
                outcome
            }
        }
    }
}

/// Runs mise to completion first, then the remaining toolchains concurrently.
fn install(plan: Plan, directory: &Path, log: bool) -> Vec<Outcome> {
    let report = Report::new(&plan, log);
    let offset = usize::from(plan.mise.is_some());
    let mut outcomes = Vec::new();
    let mut mise_installed = false;

    if let Some(step) = &plan.mise {
        let outcome = report.run(0, step, directory);
        mise_installed = outcome.status == Status::Installed;
        outcomes.push(outcome);
    }

    let steps: Vec<Step> = plan
        .parallel
        .into_iter()
        .map(|step| {
            if mise_installed {
                plan::through_mise(step)
            } else {
                step
            }
        })
        .collect();

    let report = &report;
    let concurrent = std::thread::scope(|scope| {
        let handles: Vec<_> = steps
            .iter()
            .enumerate()
            .map(|(index, step)| scope.spawn(move || report.run(offset + index, step, directory)))
            .collect();

        handles
            .into_iter()
            .map(|handle| handle.join().expect("install thread panicked"))
            .collect::<Vec<_>>()
    });

    outcomes.extend(concurrent);
    outcomes
}

/// Prints each detected toolchain, the files that gave it away, and the command
/// that would install it.
fn print_plan(plan: &Plan) {
    let rows: Vec<PlanRow> = plan
        .steps()
        .map(|step| PlanRow {
            name: step.name,
            evidence: &step.evidence,
            command: step.command_line(),
        })
        .collect();

    for line in ui::plan_listing(&rows) {
        println!("{line}");
    }
}

/// Collects what detection reads: every entry in the project root, the nested
/// locations mise also reads its config from, and the manifests that have to be
/// looked inside. An unreadable manifest is treated as absent.
fn scan(directory: &Path) -> Result<Scan> {
    let mut paths = BTreeSet::new();

    for entry in std::fs::read_dir(directory)? {
        if let Some(name) = entry?.file_name().to_str() {
            paths.insert(name.to_string());
        }
    }

    for config in detect::MISE_CONFIGS
        .iter()
        .filter(|path| path.contains('/'))
    {
        if directory.join(config).exists() {
            paths.insert((*config).to_string());
        }
    }

    let pyproject = paths
        .contains("pyproject.toml")
        .then(|| std::fs::read_to_string(directory.join("pyproject.toml")).ok())
        .flatten();

    Ok(Scan { paths, pyproject })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        super::parse(std::iter::once("i").chain(args.iter().copied()))
    }

    #[test]
    fn the_command_line_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn a_bare_invocation_installs_with_the_tree() {
        let cli = parse(&[]).unwrap();

        assert!(cli.command.is_none());
        assert!(!cli.dry_run);
    }

    #[test]
    fn log_accepts_a_directory_after_the_subcommand() {
        let cli = parse(&["log", "-C", "repo"]).unwrap();

        assert!(matches!(cli.command, Some(Command::Log)));
        assert_eq!(cli.directory, PathBuf::from("repo"));
    }

    #[test]
    fn log_accepts_a_directory_before_the_subcommand() {
        let cli = parse(&["-C", "repo", "log"]).unwrap();

        assert!(matches!(cli.command, Some(Command::Log)));
        assert_eq!(cli.directory, PathBuf::from("repo"));
    }

    #[test]
    fn a_dry_run_cannot_be_combined_with_log() {
        for args in [&["-n", "log"][..], &["log", "-n"]] {
            let error = parse(args).err().expect("conflict rejected");
            assert!(
                matches!(
                    error.kind(),
                    ErrorKind::ArgumentConflict | ErrorKind::UnknownArgument
                ),
                "{args:?}: {error}"
            );
        }
    }
}
