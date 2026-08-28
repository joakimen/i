//! Running install steps as child processes.
//!
//! Output is either captured, so concurrent steps never interleave and only
//! failures need to be shown, or streamed line by line behind a prefix that says
//! which step it came from.

use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use crate::plan::Step;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Installed,
    /// The tool is not installed, so there was nothing to run.
    Skipped {
        reason: String,
    },
    Failed {
        code: Option<i32>,
    },
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub name: &'static str,
    pub status: Status,
    /// Combined stdout and stderr of the command, empty when it never ran.
    pub output: String,
    pub duration: Duration,
}

impl Outcome {
    pub fn failed(&self) -> bool {
        matches!(self.status, Status::Failed { .. })
    }
}

/// What to do with a step's output while it runs.
#[derive(Debug, Clone, Copy)]
pub enum Output<'a> {
    /// Collect both streams; the caller decides whether to show them.
    Capture,
    /// Write every line to stderr as it arrives, behind `prefix`.
    Stream { prefix: &'a str },
}

/// A finished child process.
struct Completion {
    status: ExitStatus,
    output: String,
}

pub fn run(step: &Step, directory: &Path, output: Output<'_>) -> Outcome {
    let started = Instant::now();
    let result = match output {
        Output::Capture => capture(step, directory),
        Output::Stream { prefix } => stream(step, directory, prefix),
    };
    let duration = started.elapsed();

    let (status, output) = match result {
        Ok(completion) => {
            let status = if completion.status.success() {
                Status::Installed
            } else {
                Status::Failed {
                    code: completion.status.code(),
                }
            };
            (status, completion.output)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => (
            Status::Skipped {
                reason: format!("{} not found", step.program),
            },
            String::new(),
        ),
        Err(error) => (Status::Failed { code: None }, error.to_string()),
    };

    Outcome {
        name: step.name,
        status,
        output,
        duration,
    }
}

fn command(step: &Step, directory: &Path) -> Command {
    let mut command = Command::new(step.program);
    command.args(&step.args).current_dir(directory);
    command
}

fn capture(step: &Step, directory: &Path) -> io::Result<Completion> {
    let result = command(step, directory).output()?;

    Ok(Completion {
        status: result.status,
        output: merge_output(&result.stdout, &result.stderr),
    })
}

/// Forwards both streams to stderr while the child runs. Reading them on separate
/// threads keeps a child that fills one pipe from blocking on the other.
fn stream(step: &Step, directory: &Path, prefix: &str) -> io::Result<Completion> {
    let mut child = command(step, directory)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    std::thread::scope(|scope| {
        scope.spawn(|| forward(stdout, prefix));
        scope.spawn(|| forward(stderr, prefix));
    });

    Ok(Completion {
        status: child.wait()?,
        output: String::new(),
    })
}

/// Writes each line of `source` to stderr behind `prefix`. One `eprintln!` per
/// line keeps lines from concurrent steps whole.
fn forward(source: impl Read, prefix: &str) {
    let mut reader = BufReader::new(source);
    let mut line = Vec::new();

    while let Ok(read) = reader.read_until(b'\n', &mut line) {
        if read == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&line);
        eprintln!("{prefix}{}", text.trim_end_matches(['\n', '\r']));
        line.clear();
    }
}

/// Joins a command's streams into the single block shown on failure, stdout
/// first, with trailing blank lines removed.
fn merge_output(stdout: &[u8], stderr: &[u8]) -> String {
    let mut merged = String::new();

    for stream in [stdout, stderr] {
        let text = String::from_utf8_lossy(stream);
        let text = text.trim_end();
        if text.is_empty() {
            continue;
        }
        if !merged.is_empty() {
            merged.push('\n');
        }
        merged.push_str(text);
    }

    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merging_puts_stdout_before_stderr() {
        assert_eq!(merge_output(b"out\n", b"err\n"), "out\nerr");
    }

    #[test]
    fn merging_drops_empty_streams() {
        assert_eq!(merge_output(b"", b"err\n"), "err");
        assert_eq!(merge_output(b"out\n", b""), "out");
        assert_eq!(merge_output(b"", b""), "");
    }

    #[test]
    fn merging_keeps_invalid_utf8_readable() {
        assert_eq!(merge_output(&[0xff, b'a'], b""), "\u{fffd}a");
    }
}
