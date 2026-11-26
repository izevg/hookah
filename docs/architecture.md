# Architecture

## Layers

How `domain`, `application`, `adapters`, `harness`, and `cli` fit together, and which way
dependencies may point. A sixth crate, `setup`, is an install-time adapter: it owns the file formats
of the harness configs (JSON, TOML, YAML, the opencode, Hermes, and OpenClaw plugin files) for `hookah setup` and writes the
files of a multi-file plugin (`hookah plugin orca --out`), which keeps
config-file editing out of the crates that run on every hook call. The rules themselves are in [AGENTS.md](../AGENTS.md#architecture-ddd--clean--hexagonal).

The `harness` crate groups its modules by integration mechanism: `hooks/` holds the command-hook
harnesses (`claude`, `codex`, `cursor`, `kimi`, `qoder`), and `plugins/` holds the code plugins and
extensions (`hermes`, `openclaw`, `opencode`, `orca`, `pi`). `lib.rs` re-exports every harness at
the crate root, so `hookah_harness::claude` and `hookah_harness::plugins::pi` both resolve.

## Light by default: hookah helps, it doesn't gatekeep

`hookah` exists to help development, not to block it. An agent that has to wait minutes for a
full suite, or that is sent back by a check no human would run after every small edit, learns
nothing and loses time. So the default is light: cheap checks on what changed, and a polite
"too big, run it yourself" when a check would not be cheap.

**What runs, per phase.**

| Phase | Runs | On what |
| ----- | ---- | ------- |
| Edit  | Format, Lint, Typecheck, Architecture | the edited file; about 1-3 s |
| Stop  | Lint, Typecheck, Architecture | all files changed in the session |
|       | Unused | whole project (Knip), reported when new; never at edit |
|       | Test | the **light** tests related to the changes, only up to the test budget |
|       | Mutation | only when a test changed **and** the change is small (mutation budget) |

**What is never run.**

- Integration, end-to-end, browser, and other heavy tests. Each ecosystem adapter classifies the
  tests it selects with one domain rule (`TestWeight`: directories `e2e/`, `integration/`,
  `playwright/`, `cypress/`; files `*.e2e.*`, `*.int.*`, `*_integration.*`) and leaves the heavy
  ones out. Tests marked `#[ignore]` stay skipped, and Python's `integration`, `e2e`, and `slow`
  markers are deselected when the project registers them. Per-ecosystem details are in
  [ecosystems.md](ecosystems.md). The exception that is light: a Storybook story that the
  project runs through the Vitest addon, scoped to the changed story file (or the stories that
  import a changed component), with a missing browser reported as a notice. The legacy
  `@storybook/test-runner`, which needs a running Storybook, is never run.
- The full test suite. With no related test, no test runs. With more related test targets than
  the budget, none run either.
- Benches.
- Mutation testing of a large change.
- Anything that is not already part of the project's own configuration, and anything that needs
  an install.

**Budgets.** Going over a budget is never a failure. The check is skipped and the agent gets a
one-line, non-blocking notice, so it knows what did not run and can decide whether to run it.

| Budget | Default | Over it | Environment variable |
| ------ | ------- | ------- | -------------------- |
| Test targets | 25 | `N related tests selected; above the light budget of 25, run them yourself`, no test runs | `HOOKAH_TEST_BUDGET` |
| Mutation: changed files | 3 | `mutation testing skipped: change too large (X files / Y lines, budget 3 / 150)` | `HOOKAH_MUTATION_MAX_FILES` |
| Mutation: changed lines | 150 | the same notice | `HOOKAH_MUTATION_MAX_LINES` |
| Maven dependents compiled with the edited module | 10 | `javac: the N modules that depend on this one were not compiled ...`, only the module and its upstream compile | `HOOKAH_DOWNSTREAM_BUDGET` |

`0` switches the check off for good and silently: `HOOKAH_TEST_BUDGET=0` runs no tests at stop,
`0` for either mutation variable runs no mutation testing, and `HOOKAH_DOWNSTREAM_BUDGET=0`
compiles no dependents. A value that is not a whole number is ignored. [configuration.md](configuration.md) has the details; `.hookah.toml` will take over
the same settings.

- A test target is whatever the ecosystem hands to its runner: a test file for Python, a
  `--package` or `--test` entry for Rust, a changed source file for Node, where the runner finds
  the tests behind it.
- Mutation counts the changed code files of the project (changed tests and changed sources) and
  the lines `git diff HEAD` reports as changed in them; a new file counts as long as the file. The
  limit applies to the change, not to what the tool will end up mutating, because the change
  is what the agent is in control of.
- A mutation run that times out is a degraded, non-blocking result, never a failure: only
  surviving mutants fail.

**Why these numbers.** 25 test targets is what a quick refactor touches; past that the change is
a migration or a rename sweep, and a runner would take minutes. 3 files and 150 lines are about
one function and its tests plus a neighbour: mutation testing cost grows with the code mutated, so
a focused change gets a real answer and a sweeping one is left to the developer. The numbers are
defaults, not policy: raise them in CI or on a fast machine, lower them on a slow one.

## Request flow

Check kinds: `Format`, `Lint`, `Typecheck` (tsc, mypy), `Architecture` (dependency-cruiser,
import-linter), `Unused` (knip: whole-project analysis), `Test`, `Mutation`. The phase decides which run and in what order
(`Phase::check_kinds`):

| Phase | Kinds, in order                                                        |
| ----- | ---------------------------------------------------------------------- |
| Edit  | Format, Lint, Typecheck, Architecture (on the edited file)             |
| Stop  | Lint, Typecheck, Architecture (all changed files of the project), Unused, Test, Mutation |

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
  (project-relative targets), asks the detector to select the related light tests and runs them
  if they fit the test budget, then decides on mutation testing, which the mutation budget may
  skip (see "Light by default"). The `Budgets` value comes from the composition root. A failing verdict blocks the stop once; with `stop_hook_active`
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
narrow. `Lint`, `Typecheck`, `Architecture`, and `Unused` findings follow the baseline when there is one,
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
(`FsLocator`, `Ecosystems`, `ProcessRunner`, `GitChanges`, `FileHasher`, `FileBaselineStore`, `FileStatusStore`,
`BackgroundSpawner`) and hands them to the
use cases as `Ports`.

## Check status

Every finished run leaves a trace for tools that want to show it without running the checks (the
[Orca plugin](setup/plugins/orca.md) is the first). `CheckEdit` and `CheckSession` end by handing
a `Status` (phase, `Standing` of `Pass`/`Degraded`/`Fail`, problem count, the first report lines) to
the `StatusStore` port, keyed by project root; the `FileStatusStore` adapter keeps one
`status.json` per project in the cache directory and stamps it with the time. The domain owns the
rules: a status is derived from a `Verdict`, and the worst of several standings is their maximum.
A failing store never changes a verdict.

- **Latest run wins.** The record is replaced on every run, so it is not a history. A stop-phase
  run in a project with no changed files records a `Pass`: a fixed problem stops showing once the
  tree is checked again.
- **Reading.** `ReadStatus` finds the project of a directory and the projects nested below it (as
  the baseline does) and returns each with its latest record. `hookah status [--root <dir>]
  [--json]` prints it. It runs no checks and exits 0 whatever the standing.
- **Checking a whole project.** `hookah check --changed [--root <dir>]` runs `CheckSession`, the
  stop phase, from the command line.

## Detection cache

Every hook call is a fresh process, so Rust toolchain detection (`cargo metadata`,
`cargo mutants --version`, about 40 ms) would repeat on every edit. The `ecosystems::cache` module
of the `adapters` crate keeps its results between calls. It is a decorator inside the
`Ecosystems` registry: `ToolchainDetector::detect` goes through it, and `hookah detect` prints
whether the result was a `hit`, a `miss`, or `off`.

**Only Rust is cached.** Node, Python, and Java detection reads a few small files and takes about
3 ms, as much as starting the process (the JVM start in the commands that follow costs a
thousand times more), so caching it would add stale-cache risk for no gain. Node, Python, and Java
detect directly on every call (`hookah detect` says `cache: off` for them). The module is
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
The child (`ComputeBaseline`) runs every Lint, Typecheck, Architecture, and Unused tool of the project over
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

**Session start per harness.** Each harness's docs were read on 2026-11-17, except opencode and Pi, which
are our own plugins.

| Harness | Event | Status |
| --- | --- | --- |
| Claude Code | `SessionStart` (matchers `startup`, `resume`, `clear`, `compact`) | verified |
| Codex CLI | `SessionStart` (same matchers, plus `fork`) | verified |
| Qoder | `SessionStart` | verified: the event exists and every payload has `session_id` and `cwd`; its matcher values are not documented |
| Cursor | `sessionStart` (`session_id` equals `conversation_id`; fire-and-forget) | verified in the docs; not run live, and `cursor-agent` is reported not to send `conversation_id` |
| Kimi Code | `SessionStart` (matchers `startup`, `resume`) | verified |
| Antigravity CLI | `SessionStart` | the payload supplies `conversationId` and `workspacePaths` |
| opencode | the plugin's `session.created` event, sent as `event: "start"` (contract v1, backward compatible) | plugin written against the SDK types; not run live |
| Hermes Agent | the plugin's `on_session_start` hook (new sessions only), sent as `event: "start"` | verified live against a mock model server |
| Pi | the extension's `session_start` event (reasons other than `reload`), sent as `event: "start"` (contract v1) | verified live against Pi 0.73.1 (RPC mode, mock model) |
| OpenClaw | the plugin's `session_start` hook, sent as `event: "start"` (same contract as opencode) | verified live through the Gateway; `openclaw agent --local` fires none, so the first edit starts the baseline |

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
static findings (Format, Lint, Typecheck, Architecture, Unused), of any severity and in any file, whose
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

## Self-update

The release binary updates itself from these releases, see [Updates](setup/update.md) for what a user
sees. The layers: the rules (versions, which archive fits a platform, the 24-hour throttle, what is
announced once) are pure code in `domain::update`; the use cases `ScheduleUpdate` (session start) and
`RunUpdate` (the check and install) and their ports (`ReleaseSource`, `Installer`, `UpdateStore`,
`Clock`, `UpdateSpawner`) are in `application`; the HTTP client, the checksum and archive code, the
atomic swap, and the state file are in `adapters::update`; `cli` wires them. A session start only reads
the state and spawns `hookah update --background`; the job is detached and never waited for, like the
baseline job.
