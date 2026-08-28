//! Mapping detected toolchains to the commands that install their dependencies.
//!
//! I/O-free: building a plan only rewrites data, so ordering and command
//! construction are tested directly.

use crate::detect::{Detection, PackageManager, PythonMode, Toolchain};

/// One install command, labelled with the name shown while it runs and the files
/// that selected it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub name: &'static str,
    pub evidence: String,
    pub program: &'static str,
    pub args: Vec<&'static str>,
}

impl Step {
    pub fn command_line(&self) -> String {
        if self.args.is_empty() {
            self.program.to_string()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        }
    }
}

/// An ordered install plan: `mise` runs to completion first because it provides
/// the toolchains the remaining steps need, and `parallel` may then run at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub mise: Option<Step>,
    pub parallel: Vec<Step>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.mise.is_none() && self.parallel.is_empty()
    }

    /// Every step in execution order.
    pub fn steps(&self) -> impl Iterator<Item = &Step> {
        self.mise.iter().chain(self.parallel.iter())
    }
}

pub fn plan(detections: &[Detection]) -> Plan {
    let mut mise = None;
    let mut parallel = Vec::new();

    for detection in detections {
        let step = step(detection);
        if detection.toolchain == Toolchain::Mise {
            mise = Some(step);
        } else {
            parallel.push(step);
        }
    }

    Plan { mise, parallel }
}

fn step(detection: &Detection) -> Step {
    let evidence = detection.evidence.clone();
    match detection.toolchain {
        Toolchain::Mise => Step {
            name: "mise",
            evidence,
            program: "mise",
            args: vec!["install"],
        },
        Toolchain::Rust => Step {
            name: "rust",
            evidence,
            program: "cargo",
            args: vec!["fetch"],
        },
        Toolchain::Go => Step {
            name: "go",
            evidence,
            program: "go",
            args: vec!["mod", "download"],
        },
        Toolchain::Node(manager) => match manager {
            PackageManager::Bun => Step {
                name: "bun",
                evidence,
                program: "bun",
                args: vec!["install"],
            },
            PackageManager::Pnpm => Step {
                name: "pnpm",
                evidence,
                program: "pnpm",
                args: vec!["install"],
            },
            PackageManager::Yarn => Step {
                name: "yarn",
                evidence,
                program: "yarn",
                args: vec!["install"],
            },
            PackageManager::Npm => Step {
                name: "npm",
                evidence,
                program: "npm",
                args: vec!["install"],
            },
        },
        Toolchain::Deno => Step {
            name: "deno",
            evidence,
            program: "deno",
            args: vec!["install"],
        },
        Toolchain::Clojure => Step {
            name: "clojure",
            evidence,
            program: "clojure",
            args: vec!["-P"],
        },
        Toolchain::Babashka => Step {
            name: "babashka",
            evidence,
            program: "bb",
            args: vec!["prepare"],
        },
        Toolchain::Maven { wrapper } => Step {
            name: "maven",
            evidence,
            program: if wrapper { "./mvnw" } else { "mvn" },
            args: vec!["-B", "dependency:go-offline"],
        },
        Toolchain::Gradle { wrapper } => Step {
            name: "gradle",
            evidence,
            program: if wrapper { "./gradlew" } else { "gradle" },
            args: vec!["dependencies"],
        },
        Toolchain::Python(mode) => match mode {
            PythonMode::Project => Step {
                name: "python",
                evidence,
                program: "uv",
                args: vec!["sync"],
            },
            PythonMode::Requirements => Step {
                name: "python",
                evidence,
                program: "uv",
                args: vec!["pip", "install", "-r", "requirements.txt"],
            },
        },
        Toolchain::Terraform => Step {
            name: "terraform",
            evidence,
            program: "terraform",
            args: vec!["init", "-input=false"],
        },
    }
}

/// Rewrites a step to run under `mise exec`, so tools that `mise install` just
/// placed in the project are resolved without re-entering the shell.
pub fn through_mise(step: Step) -> Step {
    let mut args = vec!["exec", "--", step.program];
    args.extend(step.args);

    Step {
        name: step.name,
        evidence: step.evidence,
        program: "mise",
        args,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detections(toolchains: &[Toolchain]) -> Vec<Detection> {
        toolchains
            .iter()
            .map(|&toolchain| Detection {
                toolchain,
                evidence: String::new(),
            })
            .collect()
    }

    fn step_for(toolchain: Toolchain) -> Step {
        step(&Detection {
            toolchain,
            evidence: String::new(),
        })
    }

    #[test]
    fn mise_is_separated_from_the_parallel_steps() {
        let plan = plan(&detections(&[
            Toolchain::Rust,
            Toolchain::Mise,
            Toolchain::Go,
        ]));

        assert_eq!(
            plan.mise.map(|step| step.command_line()),
            Some("mise install".to_string())
        );
        assert_eq!(
            plan.parallel
                .iter()
                .map(Step::command_line)
                .collect::<Vec<_>>(),
            vec!["cargo fetch", "go mod download"]
        );
    }

    #[test]
    fn a_plan_without_mise_runs_everything_in_parallel() {
        let plan = plan(&detections(&[Toolchain::Rust]));

        assert_eq!(plan.mise, None);
        assert_eq!(plan.parallel.len(), 1);
    }

    #[test]
    fn an_empty_toolchain_list_yields_an_empty_plan() {
        assert!(plan(&[]).is_empty());
    }

    #[test]
    fn build_wrappers_are_preferred_over_the_system_tool() {
        assert_eq!(
            step_for(Toolchain::Maven { wrapper: true }).command_line(),
            "./mvnw -B dependency:go-offline"
        );
        assert_eq!(
            step_for(Toolchain::Maven { wrapper: false }).command_line(),
            "mvn -B dependency:go-offline"
        );
        assert_eq!(
            step_for(Toolchain::Gradle { wrapper: true }).command_line(),
            "./gradlew dependencies"
        );
    }

    #[test]
    fn wrapping_in_mise_keeps_the_label_and_arguments() {
        let wrapped = through_mise(step_for(Toolchain::Node(PackageManager::Bun)));

        assert_eq!(wrapped.name, "bun");
        assert_eq!(wrapped.command_line(), "mise exec -- bun install");
    }

    #[test]
    fn steps_carry_the_evidence_that_selected_them() {
        let plan = plan(&[Detection {
            toolchain: Toolchain::Node(PackageManager::Bun),
            evidence: "package.json + bun.lock".to_string(),
        }]);

        assert_eq!(plan.parallel[0].evidence, "package.json + bun.lock");
    }

    #[test]
    fn steps_are_listed_with_mise_first() {
        let plan = plan(&detections(&[Toolchain::Rust, Toolchain::Mise]));

        assert_eq!(
            plan.steps().map(|step| step.name).collect::<Vec<_>>(),
            vec!["mise", "rust"]
        );
    }
}
