# Architecture

## Layers

How `domain`, `application`, `adapters`, `harness`, and `cli` fit together, and which way
dependencies may point. The rules themselves are in [AGENTS.md](../AGENTS.md#architecture-ddd--clean--hexagonal).

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
  into findings, and the `ChangeSource` gives the changed lines so only findings on those lines
  count. The resulting `Verdict` goes back to the harness, which renders it (exit code, stderr,
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
narrow. `Lint`, `Typecheck`, and `Architecture` findings follow the usual rule (errors always,
warnings on changed lines only).

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
(`FsLocator`, `Ecosystems`, `ProcessRunner`, `GitChanges`, `FileHasher`) and hands them to the
use cases as `Ports`.

## Baseline

Post-MVP. Until then the changed-lines filter stands in for a baseline. Fingerprints will be
taken, stored, and compared so only new findings are reported.

## Failure handling

Timeouts, missing tools, and internal errors, and how each becomes a degraded, non-blocking
outcome.
