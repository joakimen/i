//! Terminal reporting: a live status line per install step, or a coloured prefix
//! per step when output is streamed, then a summary.
//!
//! Everything is written to stderr so stdout stays free for machine-readable
//! output. Grouping and formatting are pure; only printing touches the terminal.

use std::time::Duration;

use console::{Color, Style, style};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use crate::run::{Outcome, Status};

const TICK: Duration = Duration::from_millis(80);
/// Cycled in plan order so each step is told apart at a glance. Red is left to
/// failures, and the basic colours follow the terminal's own palette.
const PREFIX_COLOURS: &[Color] = &[
    Color::Cyan,
    Color::Magenta,
    Color::Green,
    Color::Yellow,
    Color::Blue,
];
const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub struct Ui {
    multi: MultiProgress,
    width: usize,
    attended: bool,
}

/// A single step's status line, from start to final state.
pub struct Line {
    bar: Option<ProgressBar>,
    width: usize,
}

impl Ui {
    pub fn new(names: &[&str]) -> Self {
        Ui {
            multi: MultiProgress::new(),
            width: names.iter().map(|name| name.len()).max().unwrap_or(0),
            attended: console::user_attended_stderr(),
        }
    }

    pub fn start(&self, name: &str) -> Line {
        if !self.attended {
            return Line {
                bar: None,
                width: self.width,
            };
        }

        let bar = self.multi.add(ProgressBar::new_spinner());
        bar.set_style(
            ProgressStyle::with_template("{spinner:.blue} {msg}")
                .expect("static template")
                .tick_strings(FRAMES),
        );
        bar.set_message(name.to_string());
        bar.enable_steady_tick(TICK);

        Line {
            bar: Some(bar),
            width: self.width,
        }
    }
}

impl Line {
    pub fn finish(self, outcome: &Outcome) {
        let text = status_line(outcome, self.width);
        match self.bar {
            Some(bar) => {
                bar.set_style(ProgressStyle::with_template("{msg}").expect("static template"));
                bar.finish_with_message(text);
            }
            None => eprintln!("{text}"),
        }
    }
}

fn status_line(outcome: &Outcome, width: usize) -> String {
    let name = format!("{:width$}", outcome.name);
    match &outcome.status {
        Status::Installed => format!(
            "{} {name}  {}",
            style("✔").green(),
            style(format_duration(outcome.duration)).dim()
        ),
        Status::Skipped { reason } => {
            format!(
                "{} {}  {}",
                style("○").dim(),
                style(name).dim(),
                style(reason).dim()
            )
        }
        Status::Failed { code } => format!(
            "{} {name}  {}",
            style("✖").red(),
            style(failure_reason(*code)).red()
        ),
    }
}

fn failure_reason(code: Option<i32>) -> String {
    match code {
        Some(code) => format!("exit {code}"),
        None => "did not run".to_string(),
    }
}

/// Prints the steps that did not install, then the total wall time.
pub fn summary(outcomes: &[Outcome], elapsed: Duration) {
    let groups = group(outcomes);
    let label_width = 11;

    eprintln!();
    for (label, colour, entries) in [
        ("skipped", Colour::Dim, &groups.skipped),
        ("failed", Colour::Red, &groups.failed),
    ] {
        if entries.is_empty() {
            continue;
        }
        let label = format!("{label:label_width$}");
        let label = match colour {
            Colour::Dim => style(label).dim(),
            Colour::Red => style(label).red(),
        };
        eprintln!("{label}{}", entries.join(", "));
    }
    eprintln!(
        "{}{}",
        style(format!("{:label_width$}", "total")).dim(),
        style(format_duration(elapsed)).dim()
    );
}

/// A row of the dry-run listing.
pub struct PlanRow<'a> {
    pub name: &'a str,
    pub evidence: &'a str,
    pub command: String,
}

/// Builds the `[name]` prefix each streamed line is written behind: coloured per
/// step, and padded so the output of every step lines up in one column.
pub fn prefixes(names: &[&str]) -> Vec<String> {
    let width = names.iter().map(|name| name.len()).max().unwrap_or(0);

    names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let (label, padding) = prefix_parts(name, width);
            format!(
                "{}{padding}",
                Style::new().fg(colour(index)).apply_to(label)
            )
        })
        .collect()
}

/// Formats the dry-run listing, giving each toolchain the colour its streamed
/// output is prefixed with so a step looks the same in both modes.
pub fn plan_listing(rows: &[PlanRow<'_>]) -> Vec<String> {
    let name_width = rows.iter().map(|row| row.name.len()).max().unwrap_or(0);
    let evidence_width = rows.iter().map(|row| row.evidence.len()).max().unwrap_or(0);

    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            format!(
                "{}{}  {}{}  {} {}",
                Style::new().fg(colour(index)).apply_to(row.name),
                padding(row.name, name_width),
                style(row.evidence).dim(),
                padding(row.evidence, evidence_width),
                style("$").dim(),
                row.command
            )
        })
        .collect()
}

fn colour(index: usize) -> Color {
    PREFIX_COLOURS[index % PREFIX_COLOURS.len()]
}

/// Splits a prefix into its label and the padding that aligns it, keeping the
/// trailing spaces outside the colour escape.
fn prefix_parts(name: &str, width: usize) -> (String, String) {
    let label = format!("[{name}]");
    let spaces = padding(&label, width + 3);
    (label, spaces)
}

fn padding(text: &str, width: usize) -> String {
    " ".repeat(width.saturating_sub(text.len()))
}

/// Prints the captured output of the given steps under a heading each.
pub fn details<'a>(outcomes: impl IntoIterator<Item = &'a Outcome>) {
    for outcome in outcomes {
        if outcome.output.is_empty() {
            continue;
        }
        eprintln!();
        eprintln!("{}", style(format!("── {} ──", outcome.name)).dim());
        eprintln!("{}", outcome.output);
    }
}

enum Colour {
    Dim,
    Red,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Groups {
    skipped: Vec<String>,
    failed: Vec<String>,
}

/// Buckets the steps that need attention, keeping the order they were planned in
/// and annotating skips with why they were skipped.
fn group(outcomes: &[Outcome]) -> Groups {
    let mut groups = Groups::default();

    for outcome in outcomes {
        match &outcome.status {
            Status::Installed => {}
            Status::Skipped { reason } => {
                groups.skipped.push(format!("{} ({reason})", outcome.name))
            }
            Status::Failed { .. } => groups.failed.push(outcome.name.to_string()),
        }
    }

    groups
}

fn format_duration(duration: Duration) -> String {
    let millis = duration.as_millis();
    if millis < 1000 {
        format!("{millis}ms")
    } else {
        format!("{:.1}s", duration.as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(name: &'static str, status: Status) -> Outcome {
        Outcome {
            name,
            status,
            output: String::new(),
            duration: Duration::from_millis(120),
        }
    }

    #[test]
    fn prefixes_are_padded_to_a_single_column() {
        let (label, padding) = prefix_parts("go", 7);
        assert_eq!(
            (label.len() + padding.len(), label),
            (10, "[go]".to_string())
        );

        let (label, padding) = prefix_parts("clojure", 7);
        assert_eq!(
            (label.len() + padding.len(), label),
            (10, "[clojure]".to_string())
        );
    }

    #[test]
    fn the_plan_listing_shows_every_column() {
        let rows = [
            PlanRow {
                name: "mise",
                evidence: "mise.toml",
                command: "mise install".to_string(),
            },
            PlanRow {
                name: "bun",
                evidence: "package.json + bun.lock",
                command: "bun install".to_string(),
            },
        ];

        let lines = plan_listing(&rows);

        assert_eq!(lines.len(), 2);
        for (line, row) in lines.iter().zip(&rows) {
            assert!(line.contains(row.name), "{line}");
            assert!(line.contains(row.evidence), "{line}");
            assert!(line.contains(&format!("$ {}", row.command)), "{line}");
        }
    }

    #[test]
    fn every_step_gets_a_prefix() {
        let names = ["mise", "rust", "bun"];
        let prefixes = prefixes(&names);

        assert_eq!(prefixes.len(), names.len());
        for (prefix, name) in prefixes.iter().zip(names) {
            assert!(prefix.contains(&format!("[{name}]")), "{prefix}");
        }
    }

    #[test]
    fn durations_switch_from_milliseconds_to_seconds() {
        assert_eq!(format_duration(Duration::from_millis(0)), "0ms");
        assert_eq!(format_duration(Duration::from_millis(999)), "999ms");
        assert_eq!(format_duration(Duration::from_millis(1000)), "1.0s");
        assert_eq!(format_duration(Duration::from_millis(4321)), "4.3s");
    }

    #[test]
    fn grouping_reports_skips_and_failures_in_planned_order() {
        let outcomes = [
            outcome("mise", Status::Installed),
            outcome(
                "maven",
                Status::Skipped {
                    reason: "mvn not found".to_string(),
                },
            ),
            outcome("go", Status::Failed { code: Some(1) }),
            outcome("rust", Status::Installed),
        ];

        assert_eq!(
            group(&outcomes),
            Groups {
                skipped: vec!["maven (mvn not found)".to_string()],
                failed: vec!["go".to_string()],
            }
        );
    }

    #[test]
    fn grouping_an_empty_run_yields_nothing() {
        assert_eq!(group(&[]), Groups::default());
    }

    #[test]
    fn failures_report_the_exit_code_when_there_is_one() {
        assert_eq!(failure_reason(Some(2)), "exit 2");
        assert_eq!(failure_reason(None), "did not run");
    }
}
