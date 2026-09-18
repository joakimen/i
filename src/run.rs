//! Running install steps as child processes.
//!
//! Every line a step writes, on either stream, is handed to the caller as it
//! arrives and is also kept, so a failure can be shown in full afterwards.

use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Mutex;
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
    /// Every line the command wrote to stdout or stderr, in the order they
    /// arrived; empty when it never ran.
    pub output: String,
    pub duration: Duration,
}

impl Outcome {
    pub fn failed(&self) -> bool {
        matches!(self.status, Status::Failed { .. })
    }
}

/// A finished child process.
struct Completion {
    status: ExitStatus,
    output: String,
}

/// Runs `step` in `directory`, calling `on_line` with each line of output as it
/// is written. The child gets no stdin, so a tool that prompts fails instead of
/// waiting for an answer that never comes.
pub fn run(step: &Step, directory: &Path, on_line: &(dyn Fn(&str) + Sync)) -> Outcome {
    let started = Instant::now();
    let result = execute(step, directory, on_line);
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

/// Reads both streams on their own threads while the child runs, so a child
/// that fills one pipe never blocks on the other.
fn execute(
    step: &Step,
    directory: &Path,
    on_line: &(dyn Fn(&str) + Sync),
) -> io::Result<Completion> {
    let mut child = Command::new(step.program)
        .args(&step.args)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let output = Mutex::new(String::new());
    let record = |line: &str| {
        on_line(line);
        let mut output = output.lock().expect("output lock poisoned");
        output.push_str(line);
        output.push('\n');
    };

    std::thread::scope(|scope| {
        scope.spawn(|| lines(stdout, &record));
        scope.spawn(|| lines(stderr, &record));
    });

    let output = output.into_inner().expect("output lock poisoned");
    Ok(Completion {
        status: child.wait()?,
        output: output.trim_end().to_string(),
    })
}

/// Calls `on_line` with each line of `source`, without its line ending. Bytes
/// that are not UTF-8 are replaced rather than dropped.
fn lines(source: impl Read, on_line: &dyn Fn(&str)) {
    let mut reader = BufReader::new(source);
    let mut line = Vec::new();

    while let Ok(read) = reader.read_until(b'\n', &mut line) {
        if read == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&line);
        on_line(text.trim_end_matches(['\n', '\r']));
        line.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn collect(source: &[u8]) -> Vec<String> {
        let seen = RefCell::new(Vec::new());
        lines(source, &|line| seen.borrow_mut().push(line.to_string()));
        seen.into_inner()
    }

    #[test]
    fn lines_are_reported_without_their_endings() {
        assert_eq!(collect(b"one\r\ntwo\n\nthree"), ["one", "two", "", "three"]);
    }

    #[test]
    fn an_empty_stream_reports_nothing() {
        assert!(collect(b"").is_empty());
    }

    #[test]
    fn invalid_utf8_stays_readable() {
        assert_eq!(collect(&[0xff, b'a', b'\n']), ["\u{fffd}a"]);
    }
}
