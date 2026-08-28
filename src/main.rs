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
use clap::Parser;
use console::style;

use crate::detect::Scan;
use crate::plan::{Plan, Step};
use crate::run::{Outcome, Output, Status};
use crate::ui::{PlanRow, Ui};

#[derive(Parser)]
#[command(
    name = "i",
    version,
    about = "Install project dependencies for the toolchains detected in a directory"
)]
struct Cli {
    /// Directory to install in
    #[arg(short = 'C', long, value_name = "DIR", default_value = ".")]
    directory: PathBuf,

    /// Print the commands that would run, without running them
    #[arg(short = 'n', long)]
    dry_run: bool,

    /// Stream every command's output as it runs, prefixed with its source
    #[arg(short, long)]
    verbose: bool,
}

fn main() -> ExitCode {
    match run_cli(&Cli::parse()) {
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

    let started = Instant::now();
    let outcomes = install(plan, directory, cli.verbose);

    if !cli.verbose {
        ui::details(outcomes.iter().filter(|outcome| outcome.failed()));
    }
    ui::summary(&outcomes, started.elapsed());

    if outcomes.iter().any(Outcome::failed) {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

/// How a run is reported: a live status line per step with output held back, or
/// each step's output streamed behind a coloured prefix.
struct Report {
    ui: Option<Ui>,
    prefixes: Vec<String>,
}

/// One step's reporting, claimed in plan order before the step is spawned so
/// status lines and prefixes stay in that order.
enum Task<'a> {
    Status(ui::Line),
    Stream(&'a str),
}

impl Report {
    fn new(names: &[&str], verbose: bool) -> Self {
        if verbose {
            Report {
                ui: None,
                prefixes: ui::prefixes(names),
            }
        } else {
            Report {
                ui: Some(Ui::new(names)),
                prefixes: Vec::new(),
            }
        }
    }

    fn begin(&self, index: usize, step: &Step) -> Task<'_> {
        match &self.ui {
            Some(ui) => Task::Status(ui.start(step.name)),
            None => {
                let prefix = &self.prefixes[index];
                eprintln!(
                    "{prefix}{}",
                    style(format!("$ {}", step.command_line())).dim()
                );
                Task::Stream(prefix)
            }
        }
    }
}

impl Task<'_> {
    fn run(self, step: &Step, directory: &Path) -> Outcome {
        match self {
            Task::Status(line) => {
                let outcome = run::run(step, directory, Output::Capture);
                line.finish(&outcome);
                outcome
            }
            Task::Stream(prefix) => run::run(step, directory, Output::Stream { prefix }),
        }
    }
}

/// Runs mise to completion first, then the remaining toolchains concurrently.
fn install(plan: Plan, directory: &Path, verbose: bool) -> Vec<Outcome> {
    let names: Vec<&str> = plan.steps().map(|step| step.name).collect();
    let report = Report::new(&names, verbose);
    let offset = usize::from(plan.mise.is_some());
    let mut outcomes = Vec::new();
    let mut mise_installed = false;

    if let Some(step) = &plan.mise {
        let outcome = report.begin(0, step).run(step, directory);
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

    outcomes.extend(run_concurrently(&steps, offset, directory, &report));
    outcomes
}

fn run_concurrently(
    steps: &[Step],
    offset: usize,
    directory: &Path,
    report: &Report,
) -> Vec<Outcome> {
    let tasks: Vec<Task> = steps
        .iter()
        .enumerate()
        .map(|(index, step)| report.begin(offset + index, step))
        .collect();

    std::thread::scope(|scope| {
        let handles: Vec<_> = steps
            .iter()
            .zip(tasks)
            .map(|(step, task)| scope.spawn(move || task.run(step, directory)))
            .collect();

        handles
            .into_iter()
            .map(|handle| handle.join().expect("install thread panicked"))
            .collect()
    })
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
