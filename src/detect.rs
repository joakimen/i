//! Detection of the toolchains a project uses, derived from the files it contains.
//!
//! I/O-free: callers pass in the paths they found, so every rule is exercised in
//! tests without touching the filesystem.

use std::collections::BTreeSet;

/// Node package manager, chosen by which lockfile the project commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Bun,
    Pnpm,
    Yarn,
    Npm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PythonMode {
    Project,
    Requirements,
}

/// What detection reads: the files in the project root, plus the contents of the
/// manifests whose presence alone does not settle whether a toolchain applies.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Scan {
    pub paths: BTreeSet<String>,
    pub pyproject: Option<String>,
}

/// A toolchain that applies to a project, with the files that gave it away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub toolchain: Toolchain,
    pub evidence: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toolchain {
    Mise,
    Rust,
    Go,
    Node(PackageManager),
    Deno,
    Clojure,
    Babashka,
    Maven { wrapper: bool },
    Gradle { wrapper: bool },
    Python(PythonMode),
    Terraform,
}

/// Files that make a directory a mise project, including the asdf-style
/// `.tool-versions` that mise also reads.
pub const MISE_CONFIGS: &[&str] = &[
    "mise.toml",
    ".mise.toml",
    "mise.local.toml",
    ".mise.local.toml",
    ".tool-versions",
    ".mise/config.toml",
    "mise/config.toml",
    ".config/mise.toml",
    ".config/mise/config.toml",
];

/// Returns the toolchains the scanned project uses, in a fixed order so runs are
/// reproducible.
pub fn detect(scan: &Scan) -> Vec<Detection> {
    let paths = &scan.paths;
    let has = |name: &str| paths.contains(name);
    let mut found = Vec::new();
    let mut push = |toolchain, evidence: String| {
        found.push(Detection {
            toolchain,
            evidence,
        })
    };

    if let Some(config) = first_match(paths, MISE_CONFIGS) {
        push(Toolchain::Mise, config.to_string());
    }
    if has("Cargo.toml") {
        push(Toolchain::Rust, "Cargo.toml".to_string());
    }
    if has("go.mod") {
        push(Toolchain::Go, "go.mod".to_string());
    }
    if has("package.json") {
        let (manager, lockfile) = package_manager(paths);
        push(Toolchain::Node(manager), joined("package.json", lockfile));
    }
    if let Some(config) = first_match(paths, &["deno.json", "deno.jsonc", "deno.lock"]) {
        push(Toolchain::Deno, config.to_string());
    }
    if has("deps.edn") {
        push(Toolchain::Clojure, "deps.edn".to_string());
    }
    if has("bb.edn") {
        push(Toolchain::Babashka, "bb.edn".to_string());
    }
    if has("pom.xml") {
        let wrapper = has("mvnw");
        push(
            Toolchain::Maven { wrapper },
            joined("pom.xml", wrapper.then_some("mvnw")),
        );
    }
    if let Some(build_file) = first_match(
        paths,
        &[
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
        ],
    ) {
        let wrapper = has("gradlew");
        push(
            Toolchain::Gradle { wrapper },
            joined(build_file, wrapper.then_some("gradlew")),
        );
    }
    if has("uv.lock") {
        push(
            Toolchain::Python(PythonMode::Project),
            "uv.lock".to_string(),
        );
    } else if has("pyproject.toml") && scan.pyproject.as_deref().is_some_and(declares_project) {
        push(
            Toolchain::Python(PythonMode::Project),
            "pyproject.toml".to_string(),
        );
    } else if has("requirements.txt") {
        push(
            Toolchain::Python(PythonMode::Requirements),
            "requirements.txt".to_string(),
        );
    }
    if let Some(module) = paths.iter().find(|path| path.ends_with(".tf")) {
        push(Toolchain::Terraform, module.clone());
    }

    found
}

/// Whether a `pyproject.toml` declares a Python package rather than only
/// configuring tools such as ruff. Without the `[project]` table there is nothing
/// for an installer to install.
fn declares_project(pyproject: &str) -> bool {
    pyproject
        .lines()
        .any(|line| table_header(line) == Some("project"))
}

fn table_header(line: &str) -> Option<&str> {
    let (header, _) = line.trim().strip_prefix('[')?.split_once(']')?;
    Some(header.trim())
}

/// The first of `names` present, so evidence names the file a user can look at.
fn first_match(paths: &BTreeSet<String>, names: &[&'static str]) -> Option<&'static str> {
    names.iter().copied().find(|name| paths.contains(*name))
}

fn joined(first: &str, second: Option<&str>) -> String {
    match second {
        Some(second) => format!("{first} + {second}"),
        None => first.to_string(),
    }
}

/// Picks the package manager from the committed lockfile, falling back to npm
/// when a project has a `package.json` but no lockfile.
fn package_manager(paths: &BTreeSet<String>) -> (PackageManager, Option<&'static str>) {
    let lockfiles: &[(&'static str, PackageManager)] = &[
        ("bun.lock", PackageManager::Bun),
        ("bun.lockb", PackageManager::Bun),
        ("pnpm-lock.yaml", PackageManager::Pnpm),
        ("yarn.lock", PackageManager::Yarn),
        ("package-lock.json", PackageManager::Npm),
    ];

    lockfiles
        .iter()
        .find(|(lockfile, _)| paths.contains(*lockfile))
        .map_or((PackageManager::Npm, None), |(lockfile, manager)| {
            (*manager, Some(lockfile))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(names: &[&str]) -> Scan {
        Scan {
            paths: names.iter().map(|name| name.to_string()).collect(),
            pyproject: None,
        }
    }

    fn toolchains(names: &[&str]) -> Vec<Toolchain> {
        detect(&scan(names))
            .into_iter()
            .map(|detection| detection.toolchain)
            .collect()
    }

    fn evidence(names: &[&str]) -> Vec<String> {
        detect(&scan(names))
            .into_iter()
            .map(|detection| detection.evidence)
            .collect()
    }

    fn python_toolchains(pyproject: &str, names: &[&str]) -> Vec<Toolchain> {
        let scan = Scan {
            pyproject: Some(pyproject.to_string()),
            ..scan(names)
        };
        detect(&scan)
            .into_iter()
            .map(|detection| detection.toolchain)
            .collect()
    }

    #[test]
    fn empty_directory_has_no_toolchains() {
        assert_eq!(toolchains(&[]), vec![]);
    }

    #[test]
    fn unrelated_files_are_ignored() {
        assert_eq!(toolchains(&["README.md", "LICENSE", ".gitignore"]), vec![]);
    }

    #[test]
    fn tool_versions_counts_as_a_mise_project() {
        assert_eq!(toolchains(&[".tool-versions"]), vec![Toolchain::Mise]);
    }

    #[test]
    fn nested_mise_config_counts_as_a_mise_project() {
        assert_eq!(
            toolchains(&[".config/mise/config.toml"]),
            vec![Toolchain::Mise]
        );
    }

    #[test]
    fn lockfile_selects_the_node_package_manager() {
        let cases = [
            (vec!["package.json", "bun.lock"], PackageManager::Bun),
            (vec!["package.json", "bun.lockb"], PackageManager::Bun),
            (vec!["package.json", "pnpm-lock.yaml"], PackageManager::Pnpm),
            (vec!["package.json", "yarn.lock"], PackageManager::Yarn),
            (
                vec!["package.json", "package-lock.json"],
                PackageManager::Npm,
            ),
            (vec!["package.json"], PackageManager::Npm),
        ];
        for (files, expected) in cases {
            assert_eq!(
                toolchains(&files),
                vec![Toolchain::Node(expected)],
                "{files:?}"
            );
        }
    }

    #[test]
    fn bun_wins_over_other_lockfiles() {
        let files = [
            "package.json",
            "bun.lock",
            "pnpm-lock.yaml",
            "yarn.lock",
            "package-lock.json",
        ];
        assert_eq!(
            toolchains(&files),
            vec![Toolchain::Node(PackageManager::Bun)]
        );
    }

    #[test]
    fn a_lockfile_alone_is_not_a_node_project() {
        assert_eq!(toolchains(&["package-lock.json"]), vec![]);
    }

    #[test]
    fn build_wrappers_are_reported_when_present() {
        assert_eq!(
            toolchains(&["pom.xml", "mvnw"]),
            vec![Toolchain::Maven { wrapper: true }]
        );
        assert_eq!(
            toolchains(&["pom.xml"]),
            vec![Toolchain::Maven { wrapper: false }]
        );
        assert_eq!(
            toolchains(&["build.gradle.kts", "gradlew"]),
            vec![Toolchain::Gradle { wrapper: true }]
        );
    }

    #[test]
    fn a_python_project_wins_over_bare_requirements() {
        assert_eq!(
            python_toolchains(
                "[project]\nname = \"x\"",
                &["pyproject.toml", "requirements.txt"]
            ),
            vec![Toolchain::Python(PythonMode::Project)]
        );
        assert_eq!(
            toolchains(&["requirements.txt"]),
            vec![Toolchain::Python(PythonMode::Requirements)]
        );
    }

    #[test]
    fn a_pyproject_that_only_configures_tools_is_not_a_python_project() {
        assert_eq!(
            python_toolchains(
                "[tool.ruff]\ntarget-version = \"py313\"",
                &["pyproject.toml"]
            ),
            vec![]
        );
    }

    #[test]
    fn a_tool_only_pyproject_still_falls_back_to_requirements() {
        assert_eq!(
            python_toolchains("[tool.ruff]", &["pyproject.toml", "requirements.txt"]),
            vec![Toolchain::Python(PythonMode::Requirements)]
        );
    }

    #[test]
    fn a_lockfile_makes_a_python_project_without_reading_the_manifest() {
        assert_eq!(
            toolchains(&["pyproject.toml", "uv.lock"]),
            vec![Toolchain::Python(PythonMode::Project)]
        );
    }

    #[test]
    fn project_tables_are_recognised_around_comments_and_arrays() {
        assert!(declares_project("# comment\n[project]\nname = \"x\""));
        assert!(declares_project("  [project]  "));
        assert!(!declares_project("[project.optional-dependencies]"));
        assert!(!declares_project("[[project]]"));
        assert!(!declares_project("[tool.poetry]\nname = \"x\""));
        assert!(!declares_project(""));
    }

    #[test]
    fn any_terraform_file_makes_a_terraform_project() {
        assert_eq!(toolchains(&["main.tf"]), vec![Toolchain::Terraform]);
        assert_eq!(toolchains(&["notes.tfnotes"]), vec![]);
    }

    #[test]
    fn detection_order_is_stable_across_a_polyglot_project() {
        let files = [
            "main.tf",
            "package.json",
            "bun.lock",
            "Cargo.toml",
            "mise.toml",
            "go.mod",
            "deps.edn",
        ];
        assert_eq!(
            toolchains(&files),
            vec![
                Toolchain::Mise,
                Toolchain::Rust,
                Toolchain::Go,
                Toolchain::Node(PackageManager::Bun),
                Toolchain::Clojure,
                Toolchain::Terraform,
            ]
        );
    }

    #[test]
    fn evidence_names_the_files_that_matched() {
        assert_eq!(
            evidence(&["mise.toml", "package.json", "pnpm-lock.yaml", "main.tf"]),
            vec!["mise.toml", "package.json + pnpm-lock.yaml", "main.tf"]
        );
    }

    #[test]
    fn evidence_reports_a_lockless_node_project_by_its_manifest_alone() {
        assert_eq!(evidence(&["package.json"]), vec!["package.json"]);
    }

    #[test]
    fn evidence_includes_the_build_wrapper_when_it_is_used() {
        assert_eq!(evidence(&["pom.xml", "mvnw"]), vec!["pom.xml + mvnw"]);
        assert_eq!(evidence(&["pom.xml"]), vec!["pom.xml"]);
    }
}
