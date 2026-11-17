# Architecture

> Stub. Expands as the code is written.

## Layers

How `domain`, `application`, `adapters`, `harness`, and `cli` fit together, and which way
dependencies may point. The rules themselves are in [AGENTS.md](../AGENTS.md#architecture-ddd--clean--hexagonal).

## Request flow

- **Edit phase:** the harness parses the hook payload into an `EditRequest` (the edited file).
  `CheckEdit` asks the `ProjectLocator` for the manifests above the file, and the domain picks
  the nearest one as the `Project`. The `ToolchainDetector` yields its format and lint tools.
  The formatter runs first and may rewrite the file (reported as a notice, so the agent
  re-reads it). The linter then runs through the `CommandRunner`, the detector parses its output
  into findings, and the `ChangeSource` gives the changed lines so only findings on those lines
  count. The resulting `Verdict` goes back to the harness, which renders it (exit code, stderr,
  JSON).
- **Stop phase:** the harness parses the payload into a `StopRequest` (the project root).
  `CheckSession` asks the `ChangeSource` for every file changed in the session, groups them by
  project, asks the detector to select the related tests, runs them, and filters the findings.
  A failing verdict blocks the stop once; with `stop_hook_active` the harness only warns.

The `cli` crate is the composition root: `wiring.rs` builds the real adapters
(`FsLocator`, `Ecosystems`, `ProcessRunner`, `GitChanges`, `FileHasher`) and hands them to the
use cases as `Ports`.

## Baseline

How fingerprints are taken, stored, and compared, so only new findings are reported.

## Failure handling

Timeouts, missing tools, and internal errors, and how each becomes a degraded, non-blocking
outcome.
