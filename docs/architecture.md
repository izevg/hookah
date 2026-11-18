# Architecture

## Layers

How `domain`, `application`, `adapters`, `harness`, and `cli` fit together, and which way
dependencies may point. A sixth crate, `setup`, is an install-time adapter: it owns the file formats
of the harness configs (JSON, TOML, the opencode plugin file) for `hookah setup`, which keeps
config-file editing out of the crates that run on every hook call. The rules themselves are in [AGENTS.md](../AGENTS.md#architecture-ddd--clean--hexagonal).

## Request flow

Check kinds: `Format`, `Lint`, `Typecheck` (tsc, mypy), `Architecture` (dependency-cruiser,
import-linter), `Test`, `Mutation`. The phase decides which run and in what order
(`Phase::check_kinds`):

| Phase | Kinds, in order                                                        |
| ----- | ---------------------------------------------------------------------- |
| Edit  | Format, Lint, Typecheck, Architecture (on the edited file)             |
| Stop  | Lint, Typecheck, Architecture (all changed files of the project), Test, Mutation |

Static checks run again at stop to catch files changed outside edit hooks, for example by a
shell `sed`. Format never runs at stop.

- **Edit phase:** the harness parses the hook payload into an `EditRequest` (the edited file and
  the display root). `CheckEdit` asks the `ProjectLocator` for the manifests above the file, and
  the domain picks the nearest one as the `Project`. The `ToolchainDetector` yields its tools.
  The formatter runs first and may rewrite the file (reported as a notice, so the agent
  re-reads it). The other tools run through the `CommandRunner`, the detector parses their output
  into findings, and the reporting filter decides which count: the session baseline when there is
  one (only new findings), else the changed lines from the `ChangeSource` (see Baseline). The
  resulting `Verdict` goes back to the harness, which renders it (exit code, stderr,
  JSON).
- **Stop phase:** the harness parses the payload into a `StopRequest` (the project root and the
  display root). `CheckSession` asks the `ChangeSource` for every file changed in the session and
  groups them by project. Per project it runs the static checks over all changed files
  (project-relative targets), asks the detector to select the related tests and runs them, then
  decides on mutation testing. A failing verdict blocks the stop once; with `stop_hook_active`
  the harness only warns.

### File roles and mutation testing

`ToolchainDetector::classify` labels each changed file `FileRole::Source` or `FileRole::Test`. It
receives the changed lines so an adapter can treat an edit inside a trailing Rust
`#[cfg(test)]` module as a test edit. Domain rule: mutation testing runs at stop **only if at
least one changed file of the project is a Test**. The adapter then maps the changed tests to the
sources they cover (`select_mutants`) and returns tool-specific targets; `MutationSelection::None`
means nothing runs. A surviving mutant is an `Error` finding, `mutant survived: <description>`,
at the source path and line.

`Test` and `Mutation` findings are never filtered by changed lines: their scope is already
narrow. `Lint`, `Typecheck`, and `Architecture` findings follow the baseline when there is one,
else the usual rule (errors always, warnings on changed lines only).

### Missing tools

A `Toolchain` lists the `missing` tools the project should have but does not, each with a name,
a kind, and an install hint. The use cases turn them into non-blocking `Notice::MissingTool`
messages for the kinds that would run (for example, a changed test without a mutation tester).
A missing tool never fails a check, and `hookah` never installs anything.

### Display paths

Findings carry project-relative paths and are filtered that way. Only at the end are they
rendered relative to the display root, the directory where the agent stands (the harness `cwd`;
the process cwd for `hookah check`), for example `crates/domain/src/lib.rs:24:5`. A path outside
the display root is shown absolute. One rule owns this, `hookah_domain::path::display_path`, used by
both phases and by the edit header: normalize lexically (`.` and `..`, no filesystem access), then
strip the normalized display root, else keep the normalized absolute path. Aliases such as `/var`
vs `/private/var` are resolved once at the edge (`Invocation::canonical`), because `git` reports
real paths in the stop phase. Kinds `Typecheck`, `Architecture`, and `Mutation` are tagged
in reports (`[typecheck]`).

The `cli` crate is the composition root: `wiring.rs` builds the real adapters
(`FsLocator`, `Ecosystems`, `ProcessRunner`, `GitChanges`, `FileHasher`, `FileBaselineStore`,
`BackgroundSpawner`) and hands them to the
use cases as `Ports`.

## Detection cache

Every hook call is a fresh process, so Rust toolchain detection (`cargo metadata`,
`cargo mutants --version`, about 40 ms) would repeat on every edit. The `ecosystems::cache` module
of the `adapters` crate keeps its results between calls. It is a decorator inside the
`Ecosystems` registry: `ToolchainDetector::detect` goes through it, and `hookah detect` prints
whether the result was a `hit`, a `miss`, or `off`.

**Only Rust is cached.** Node and Python detection reads a few small files and takes about 3 ms,
as much as starting the process, so caching it would add stale-cache risk for no gain. Node and
Python detect directly on every call (`hookah detect` says `cache: off` for them). The module is
generic: to cache another ecosystem, give it `detection_inputs` and `derived_inputs` functions
like Rust's and add an arm in `Ecosystems::detect_traced`.

- **Key**: project root plus ecosystem, one file per ecosystem in the project cache directory
  (`detect-rust.json`). The entry stores its root and is a miss when the root differs, so two
  copies of a project never share a result.
- **Value**: the detected `Toolchain` (tools with their command specs, and the missing tools),
  as JSON through a mirror type in `cache/codec.rs`; the domain stays free of serialization.
  `metadata.json` holds the `cargo metadata` result, the most expensive step. It is used by every
  use case that needs the package, not only by detection.
- **Validity**: a fingerprint over the *detection inputs*, hookah's version, and the values of
  `PATH`, `RUSTUP_TOOLCHAIN`, and `CARGO_HOME`. An input is observed by existence, modification
  time, and size, never by content; a directory input covers the names of its subdirectories.
  The fingerprint is taken before detection runs and stored only if the inputs did not move
  while it ran.
- **Inputs** (in `rust/detect.rs`) are chosen conservatively: when unsure whether something
  matters, it is listed, because a false hit that hides a newly installed tool is worse than a
  miss. In the package directory and every ancestor: `Cargo.toml`, `Cargo.lock`,
  `rust-toolchain*`, `.mise.toml`, `mise.toml`, `.tool-versions`, `.cargo/config*`. Also `cargo`
  and `cargo-mutants` in each `PATH` directory, and the `tests` directory. Derived from
  `cargo metadata` and stored in the entry: the manifest of every workspace member and the
  directories holding them, so a new member is noticed.
- **Failure handling**: the cache never gets in the way. A missing, corrupt, or other-schema
  file is a miss and is rewritten. Writes go to a temporary file and are renamed into place;
  a failed write, an unwritable directory, or an unresolvable cache location is ignored and
  the result is computed as without a cache. Errors of detection are never cached.
- **Known limits**: a change that keeps both the size and the modification time of a file is
  not seen, and Rust's "crate has tests" hint reads `#[test]` in `src/`, which is not watched.
- **Escape hatch**: `HOOKAH_NO_CACHE=1` switches the cache off, see `configuration.md`.

## Baseline

The baseline makes `hookah` report only what the agent introduced. It is the state of the
project's static findings when the agent session started.

**Taking it.** A session-start event (see the table below) makes `StartSession` find the project
of the agent's working directory, plus the projects nested up to three levels below it (a monorepo
root holds none itself; at most 8), and claim a baseline for each. The hook then spawns a detached
child, `hookah baseline --session=<id> --root <project>` (`BaselineSpawner`), and returns at once.
The child (`ComputeBaseline`) runs every Lint, Typecheck, and Architecture tool of the project over
the whole project and stores the fingerprints of what they find. Format does not run (it rewrites
files), and Test and Mutation are never baseline-filtered. The detector chooses the targets that
make a tool cover the whole project (`ToolchainDetector::baseline_targets`): the root `.` for file
tools such as eslint, biome, and ruff; none for tools whose command is already project-wide
(clippy, `tsc`, mypy, import-linter); `src` for dependency-cruiser when the package has one.

Spawning: on Unix the child leads its own process group and its three standard streams are the null
device, so a harness waiting for the hook's pipes never waits for it. On Windows the same code
uses `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` (cross-compiled and linted in CI, not exercised by a test).
`HOOKAH_BASELINE_FOREGROUND=1` makes the hook wait for the child instead; tests and
`samples/run.sh` use it to get a baseline synchronously.

**Session start per harness.** Each harness's docs were read on 2026-11-17, except opencode, which
is our own plugin.

| Harness | Event | Status |
| --- | --- | --- |
| Claude Code | `SessionStart` (matchers `startup`, `resume`, `clear`, `compact`) | verified |
| Codex CLI | `SessionStart` (same matchers, plus `fork`) | verified |
| Qoder | `SessionStart` | verified: the event exists and every payload has `session_id` and `cwd`; its matcher values are not documented |
| Kimi Code | `SessionStart` (matchers `startup`, `resume`) | verified |
| opencode | the plugin's `session.created` event, sent as `event: "start"` (contract v1, backward compatible) | plugin written against the SDK types; not run live |

Where a harness has no session start, or the hook is not configured, the **first edit** of a
session starts the job lazily: the first edit of a (project, session) pair finds no baseline,
claims one, spawns the job, and is itself reported with the changed-lines filter. A project the
session start could not see (deeper than three levels, or outside the working directory) gets its
baseline the same way. A session id is required: a harness payload without one gets no baseline.

**Fingerprint.** `kind | path | rule | normalized message`, for example
`lint|src/a.rs|clippy::needless_return|unneeded return statement`. The path is relative to the
project root. There is no line or column, so a finding survives unrelated edits that shift it. The
normalization removes `:<line>` and `:<line>:<col>` references and the word pairs `line N` and
`column N`, and collapses whitespace. Numbers and quoted names stay: masking them would merge
different problems.

**The rule.** With a ready baseline for the project and session, a fresh run reports exactly the
static findings (Format, Lint, Typecheck, Architecture), of any severity and in any file, whose
fingerprints occur more often than in the baseline. The comparison is a multiset: with two in the
baseline and three now, one is new, and the one reported is the last by line and column. Test and
Mutation findings pass through unfiltered. Without a baseline (none, still running, or failed) the
changed-lines filter described above applies. Both phases use it: the edit phase filters the
edited file's findings, the stop phase the findings over all changed files.

**Lifetime.** Baselines live in `<project cache dir>/baselines/<session>.json` (see
`project_cache_dir`), one file per (project root, session). One JSON record holds the state:
`in_progress`, `failed` with a reason, or `ready` with the fingerprint counts. Claiming creates the
file exclusively, so only one job runs per pair; results are written to a temporary file and
renamed. An unreadable or corrupt file counts as absent. An `in_progress` record older than 30
minutes belongs to a dead job and reads as failed. Any failure of any tool fails the whole
baseline, because a baseline missing a tool's findings would make old problems look new; so does a
project without any static tool. Records older than 7 days are deleted whenever a baseline is
claimed.

## Failure handling

Timeouts, missing tools, and internal errors, and how each becomes a degraded, non-blocking
outcome.

## Windows

`hookah` builds for `x86_64-pc-windows-msvc` and `-gnu`. CI builds, lints, and tests on Windows;
the sample runner (`samples/run.sh`) is a bash script and runs on Linux and macOS only. The
platform-specific parts, each behind a portable function so Unix hosts can unit-test the logic:

- **Killing a timed-out tool.** Every check command is started by `process::tree`. On Unix the
  child leads its own process group and a timeout sends `SIGKILL` to the group (`nix`). On
  Windows the child is assigned to a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, and
  dropping the job kills the whole tree, grandchildren included. The `win32job` crate provides a
  safe wrapper, which keeps `unsafe` out of the workspace; `command-group` would also work but
  would replace the Unix path that already works, and `taskkill /T` is racy and needs a second
  process. The job is created right after the spawn, so a grandchild started in that window could
  escape; for test runners that is not a practical risk. If the job cannot be created the child
  alone is killed (fail open).
- **Detaching the baseline job.** `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`, see Baseline.
- **Environment variables for a tool** (`PYTHONPATH=src` for import-linter) travel in
  `CommandSpec::env`, not through `env(1)`, which Windows does not have.
- **Paths.** Canonical paths come from `dunce::canonicalize`, which drops the `\\?\` verbatim
  prefix `std::fs::canonicalize` adds, so they compare equal to the paths tools print. Paths git
  prints always use `/`, which Windows accepts everywhere. Anything shown to the agent goes
  through `display_path`, which writes `/` separators on Windows too: reports read
  `src/lib.rs:3: message` and `C:/other/a.rs` on every platform.
- **Home boundary.** The project walk stops at `%USERPROFILE%` where Unix uses `$HOME`.

## Releases

Every squash merge to `main` that passes CI can become a release, with no commit back to `main`.

- `auto-release` waits for `ci` to succeed on a push to `main`, runs `git-cliff` (rules and the
  first version in `.github/cliff.toml`, wrapper `.github/scripts/next-version.sh`) over the
  Conventional Commits since the last `v*` tag, and stops when nothing releasable is there.
- It calls `release.yml`, which builds all targets, creates the `vX.Y.Z` tag on the built commit,
  and publishes the GitHub Release with notes from `git-cliff`. It then rebuilds this site, whose
  [Changelog](changelog.md) page is generated the same way. Tags and releases made with
  `GITHUB_TOKEN` start no other workflow, so each step is called, not triggered.
- The version lives in the tag. The repository keeps `version = "0.0.0"` under
  `[workspace.package]`; before building, `.github/scripts/set-version.sh` rewrites it (and
  `Cargo.lock`) in the CI checkout, so a release binary reports its version through
  `hookah --version` while local builds report `0.0.0`.
