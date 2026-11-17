# hookah

**Make coding agents keep the code green.**

`hookah` is a single-binary CLI you plug into the hooks of AI coding agents. After every file
edit, the agent harness calls `hookah` with the path of the changed file. `hookah` works out the
project around that file, finds its formatter, linter, and test runner, and runs them on that
file only. When something breaks, the findings go straight back into the agent's context as hook
feedback, so the agent fixes them right away instead of piling up broken code.

> Status: the MVP works for Rust projects with Claude Code (`hookah check`, `hookah detect`,
> `hookah hook claude`). TypeScript/JavaScript and Python, and the other harnesses, are still to
> come. This repository runs `hookah` on itself; see
> [Claude Code setup](docs/setup/hooks/claude-code.md#dogfooding).

## How it works

1. **Locate.** Walk up from the changed file to the nearest package manifest (`package.json`,
   `pyproject.toml`, `Cargo.toml`, `go.mod`, ...). The nearest manifest wins, so monorepos are
   handled per package.
2. **Detect.** Read the manifest and config files to find the project's own formatter, linter,
   and test runner (for example `biome`/`eslint`/`prettier` + `vitest`/`jest`, `ruff` + `pytest`,
   `rustfmt` + `clippy` + `cargo test`). The project's own scripts and configs come first.
   `.hookah.toml` overrides auto-detection.
3. **Target.** Narrow every tool to the changed file: format and lint the file, then select only
   the related tests (`vitest related`, `jest --findRelatedTests`, name conventions,
   per-package/per-crate test runs). If no related test can be found, run no tests rather than
   the whole suite.
4. **Report.** Report only findings that this change introduced, compared against a baseline, so
   old warnings don't send the agent off fixing unrelated code. Output is compact
   (`file:line: message`) and capped, with a "+N more" line when it is cut.

Two phases keep each hook fast:

| Phase   | Agent event                      | Runs                                | Budget     |
|---------|----------------------------------|-------------------------------------|------------|
| `edit`  | after a file write/edit          | format + lint of the changed file   | ~1–3 s     |
| `stop`  | when the agent declares it's done | tests related to all files changed in the session | bounded by timeout |

If the agent edits five files during a refactor, the tests don't run on each half-finished step.
They run once, when the agent says it's done, and a red result blocks the finish.

`hookah` fails open. If `hookah` itself crashes or hits its timeout, it shows a warning and never
blocks the agent.

## Supported agent harnesses

Each harness gets a thin adapter that reads that harness's hook payload, calls the core, and
reports results in the format that harness expects. The table shows the planned integration; each
row will be checked against the harness's current docs when its adapter is built.

| Harness     | Integration point                                   | Feedback channel                         |
|-------------|-----------------------------------------------------|------------------------------------------|
| Claude Code | `PostToolUse` (`Edit\|Write\|MultiEdit`), `Stop`    | exit code 2 + stderr                     |
| Codex CLI   | `PostToolUse` (`apply_patch`), `Stop` in `config.toml` / `hooks.json` | JSON `decision: block` + `reason` |
| opencode    | plugin, `tool.execute.after`                        | appended tool output                     |
| Kimi Code   | `PostToolUse` (`Write\|Edit`) `[[hooks]]` in `config.toml` (beta); no stop phase | stdout (may reach the model) |
| Qoder       | `PostToolUse` (`Write\|Edit`), `Stop` in `.qoder/settings.json` | `additionalContext` (edit), exit 2 + stderr (stop) |
| Orca        | through the hooks of the agent it runs; Orca plugin for workspace-level events | per hosted agent |

For the MVP, each harness is supported through written setup instructions. A setup wizard that
detects installed harnesses is planned after the MVP.

## Supported ecosystems (MVP)

- TypeScript / JavaScript
- Python
- Rust

## Samples

[`samples/`](samples/README.md) holds small projects with scenarios ("the agent makes this edit, `hookah` answers this") that double as end-to-end tests: `samples/run.sh`.

## License

[Apache License 2.0](LICENSE).
