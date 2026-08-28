# i

[![ci](https://github.com/joakimen/i/actions/workflows/ci.yml/badge.svg)](https://github.com/joakimen/i/actions/workflows/ci.yml)

Installs a project's dependencies without you having to remember which toolchain it uses.

`i` looks at the files in a directory, works out which package managers the project needs, and
runs their install commands — mise first, everything else at once. Command output is hidden
unless something fails.

```
✔ mise     2.1s
✔ rust     3.4s
✔ bun      812ms
○ maven    mvn not found

skipped    maven (mvn not found)
total      3.5s
```

## Install

```sh
make install
```

## Usage

```sh
i                  # install dependencies in the current directory
i -C path/to/repo  # ...or in another one
i -n               # print what was detected, why, and the commands that would run
i -v               # stream every command's output as it runs
```

`--dry-run` names each toolchain, the files that gave it away, and its install command:

```
mise       mise.toml                $ mise install
go         go.mod                   $ go mod download
bun        package.json + bun.lock  $ bun install
terraform  main.tf                  $ terraform init -input=false
```

Each toolchain keeps its colour across both modes.

`--verbose` drops the spinners and streams both output streams of every command as they
arrive, behind a per-step coloured prefix:

```
[mise]    $ mise install
[mise]    mise all tools are installed
[go]      $ mise exec -- go mod download
[bun]     $ mise exec -- bun install
[bun]     bun install v1.4.0
[go]      go: no module dependencies to download
[bun]     Checked 1 install across 2 packages
```

Status lines, streamed output and the summary go to stderr; `--dry-run` writes the plan to
stdout. The exit status is non-zero if any install command failed — a missing tool is a skip,
not a failure.

## What it detects

Only the project root is scanned.

| Toolchain | Detected by | Runs |
| --- | --- | --- |
| mise | `mise.toml`, `.mise.toml`, `.tool-versions`, nested mise configs | `mise install` |
| Rust | `Cargo.toml` | `cargo fetch` |
| Go | `go.mod` | `go mod download` |
| Node | `package.json`, package manager from the lockfile | `bun`/`pnpm`/`yarn`/`npm install` |
| Deno | `deno.json`, `deno.jsonc`, `deno.lock` | `deno install` |
| Clojure | `deps.edn` | `clojure -P` |
| Babashka | `bb.edn` | `bb prepare` |
| Maven | `pom.xml` | `mvn -B dependency:go-offline` |
| Gradle | `build.gradle[.kts]`, `settings.gradle[.kts]` | `gradle dependencies` |
| Python | `uv.lock`, or a `pyproject.toml` with a `[project]` table | `uv sync` |
| Python | `requirements.txt` | `uv pip install -r requirements.txt` |
| Terraform | any `*.tf` | `terraform init -input=false` |

Maven and Gradle use the project's wrapper script when one is committed. A `pyproject.toml`
that only configures tools such as ruff is not a Python project — there is nothing to install.

`mise install` runs to completion before the others, and they then run under `mise exec` so
toolchains it just installed are picked up without starting a new shell.
