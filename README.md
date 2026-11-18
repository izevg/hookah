<div align="center">

<img src="docs/assets/logo.svg" width="180" alt="hookah logo: a caterpillar smoking a hookah on a mushroom">

# hookah

**Relax. The hooks catch the bugs.**

A single-binary CLI that AI coding agents call from their hooks after every file edit. It runs your
project's own format, lint, typecheck, architecture, test, and mutation checks on what changed and
feeds failures straight back to the agent, so it fixes them immediately.

[![CI](https://github.com/izevg/hookah/actions/workflows/ci.yml/badge.svg)](https://github.com/izevg/hookah/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/izevg/hookah?include_prereleases&sort=semver)](https://github.com/izevg/hookah/releases)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust edition 2024](https://img.shields.io/badge/rust-edition%202024-orange.svg)](Cargo.toml)
![Platforms](https://img.shields.io/badge/platforms-Linux%20%C2%B7%20macOS%20%C2%B7%20Windows-lightgrey.svg)
![mutants: 0 survived](https://img.shields.io/badge/mutants-0%20survived-brightgreen.svg)

</div>

> **AI agents:** to install and configure hookah, follow
> <https://izevg.github.io/hookah/setup/agents.html> (raw markdown:
> <https://izevg.github.io/hookah/setup/agents.md>, index for agents:
> <https://izevg.github.io/hookah/llms.txt>).

> **Status:** works for Rust, Node (TypeScript/JavaScript), and Python projects, with the Claude
> Code, Codex, Cursor, Qoder, Kimi, opencode, and Pi harnesses and the Hermes Agent assistant
> (`hookah check`, `hookah detect`, `hookah hook <harness>`, `hookah plugin <harness>`, `hookah
> setup`), including mutation testing at stop, and with [Orca](docs/setup/plugins/orca.md)
> (`hookah plugin orca`, `hookah status`).
> End-to-end [samples](samples/README.md) cover the behavior. Prebuilt binaries and the install
> scripts ship with each [release](https://github.com/izevg/hookah/releases). This repository runs `hookah` on itself; see
> [Claude Code setup](docs/setup/hooks/cli/claude-code.md#dogfooding).

## Why

Agents write code fast and leave a trail: a type error three files away, a new lint warning, a
test they quietly weakened. `hookah` closes the loop. Each time the agent edits a file, the
harness calls `hookah`, which runs the project's own tools on that change and answers in a few
lines. What the agent sees after an edit that breaks a call site:

```text
hookah: 1 problem after editing src/lib.rs - fix before continuing:
src/lib.rs:28:5: cannot find function `sum_all` in this scope [E0425]
```

And when the agent finishes with tests that no longer prove anything (mutation testing at stop):

```text
hookah: 9 problems in the files changed in this session - tests or checks are failing, fix before finishing:
src/lib.rs:17: [mutation] mutant survived: replace < with <= in clamp
src/lib.rs:17: [mutation] mutant survived: replace clamp -> i64 with 0
src/lib.rs:19: [mutation] mutant survived: replace > with < in clamp
...
```

The agent reads that as hook feedback and fixes the code before moving on. No raw tool output, no
old warnings from code it did not touch.

## Install

| Platform      | Command |
|---------------|---------|
| Linux / macOS | `curl -fsSL https://izevg.github.io/hookah/install.sh \| sh` |
| Windows (PowerShell) | `irm https://izevg.github.io/hookah/install.ps1 \| iex` |
| From source   | `cargo install --git https://github.com/izevg/hookah --locked hookah` |

> **Let your agent do it.** Paste `https://github.com/izevg/hookah` into your coding agent and
> say: *"install and configure hookah for me."* The agent follows the deterministic procedure at
> [izevg.github.io/hookah/setup/agents.html](https://izevg.github.io/hookah/setup/agents.html)
> (source: [docs/setup/agents.md](docs/setup/agents.md); the whole site as plain text for agents:
> [llms.txt](https://izevg.github.io/hookah/llms.txt)).

## Quick start

1. Put `hookah` on your `PATH` (see Install).
2. Configure your harness, one command, no hand-editing:

   ```sh
   hookah setup --detect     # which harnesses are installed and configured
   hookah setup claude       # or: codex, cursor, qoder, kimi, opencode, hermes, pi (add --scope user for every project)
   ```

   Setup merges into existing settings, is safe to repeat, and backs the original up once. Use
   `--dry-run` to preview and `--command "mise exec -- hookah"` for a mise-pinned toolchain.
3. Restart the harness, or approve the new hooks in it. To try it by hand: `hookah check <file>`
   runs the edit phase on one file, and `hookah detect <file>` shows which tools it found.

Per-harness details and the manual snippets:

| Harness     | Setup |
|-------------|-------|
| Claude Code | [docs/setup/hooks/cli/claude-code.md](docs/setup/hooks/cli/claude-code.md) |
| Codex CLI   | [docs/setup/hooks/cli/codex.md](docs/setup/hooks/cli/codex.md) |
| Cursor      | [docs/setup/hooks/ide/cursor.md](docs/setup/hooks/ide/cursor.md) |
| Qoder       | [docs/setup/hooks/cli/qoder.md](docs/setup/hooks/cli/qoder.md) |
| Kimi Code   | [docs/setup/hooks/cli/kimi.md](docs/setup/hooks/cli/kimi.md) |
| opencode    | [docs/setup/plugins/opencode.md](docs/setup/plugins/opencode.md) |
| Hermes Agent | [docs/setup/assistants/hermes.md](docs/setup/assistants/hermes.md) |
| Pi          | [docs/setup/plugins/pi.md](docs/setup/plugins/pi.md) |
| Orca        | [docs/setup/plugins/orca.md](docs/setup/plugins/orca.md) (planned) |

An agent doing this for you follows [docs/setup/agents.md](docs/setup/agents.md).

## Supported ecosystems

`hookah` runs only the tools your project already configures (or `.hookah.toml` names). It never
installs anything.

| Ecosystem | Format | Lint | Typecheck | Architecture | Test | Mutation |
|-----------|--------|------|-----------|--------------|------|----------|
| TypeScript / JavaScript | Biome, Prettier | Biome, ESLint | `tsc` | dependency-cruiser | Vitest, Jest | Stryker |
| Python | Ruff, Black | Ruff | mypy | import-linter | pytest | mutmut |
| Rust | rustfmt | Clippy | (compiler errors via Clippy) | n/a | `cargo test` | cargo-mutants |

Details, commands, and test-selection rules: [docs/ecosystems.md](docs/ecosystems.md).

## Supported agent harnesses

Each harness gets a thin adapter that reads that harness's hook payload, calls the core, and
reports results in the format that harness expects. The table shows the integration points.

| Harness     | Integration point                                   | Feedback channel                         |
|-------------|-----------------------------------------------------|------------------------------------------|
| Claude Code | `PostToolUse` (`Edit\|Write\|MultiEdit`), `Stop`    | exit code 2 + stderr                     |
| Codex CLI   | `PostToolUse` (`apply_patch`), `Stop` in `config.toml` / `hooks.json` | JSON `decision: block` + `reason` |
| opencode    | bundled plugin (`hookah plugin opencode`): `tool.execute.after`, `session.idle` | appended tool output (edit), injected message (stop) |
| Hermes Agent | bundled plugin (`hookah setup hermes`): `transform_tool_result` (`write_file`, `patch`), `pre_verify`, `on_session_start` | tool result field (edit), `continue` directive (stop) |
| Pi          | bundled extension (`hookah plugin pi`): `tool_result`, `agent_end` | appended tool result (edit), injected message (stop) |
| Kimi Code   | `PostToolUse` (`Write\|Edit`) `[[hooks]]` in `config.toml` (beta); no stop phase | stdout (may reach the model) |
| Qoder       | `PostToolUse` (`Write\|Edit`), `Stop` in `.qoder/settings.json` | `additionalContext` (edit), exit 2 + stderr (stop) |
| Cursor      | `afterFileEdit`, `stop`, `sessionStart` in `.cursor/hooks.json` (IDE and `cursor-agent`) | `followup_message` (stop); the edit hook cannot reach the model |
| Orca        | the hooks of the agent it runs, plus a bundled Orca plugin (`hookah plugin orca --out <dir>`): status bar, sidebar panel, palette commands | per hosted agent; plugin shows the verdict per worktree |

Each harness is configured by `hookah setup <harness>`, and the manual steps are written down per
harness.

## How it works

1. **Locate.** Walk up from the changed file to the nearest package manifest (`package.json`,
   `pyproject.toml`, `Cargo.toml`). The nearest manifest wins, so monorepos are
   handled per package.
2. **Detect.** Read the manifest and config files to find the project's own formatter, linter,
   and test runner. The project's own scripts and configs come first; `.hookah.toml` overrides
   auto-detection.
3. **Target.** Narrow every tool to the changed file: format and lint the file, then select only
   the related tests (`vitest related`, `jest --findRelatedTests`, per-package/per-crate test
   runs). If no related test can be found, run no tests rather than the whole suite.
4. **Report.** Report only findings that this change introduced, compared against a baseline, so
   old warnings don't send the agent off fixing unrelated code. Output is compact
   (`file:line: message`) and capped, with a "+N more" line when it is cut.

Two phases keep each hook fast:

| Phase   | Agent event                      | Runs                                | Budget     |
|---------|----------------------------------|-------------------------------------|------------|
| `edit`  | after a file write/edit          | format + lint of the changed file   | ~1-3 s     |
| `stop`  | when the agent declares it's done | tests related to all files changed in the session | bounded by timeout |

If the agent edits five files during a refactor, the tests don't run on each half-finished step.
They run once, when the agent says it's done, and a red result blocks the finish.

<details>
<summary>Guarantees</summary>

- **Fail open.** If `hookah` crashes or hits its timeout, it shows a warning and never blocks the
  agent.
- **Safe formatting.** When a formatter rewrites the file, the output says so and the agent
  re-reads it before its next edit.
- **No surprises.** Only the project's own tools run. Nothing is installed, and nothing is
  written outside the project and `hookah`'s cache.

</details>

## Samples

[`samples/`](samples/README.md) holds small projects with scenarios ("the agent makes this edit,
`hookah` answers this") that double as end-to-end tests: `samples/run.sh`.

## Documentation

| Page | Covers |
|------|--------|
| [Architecture](docs/architecture.md) | layers, request flow, baseline, failure handling |
| [Ecosystems](docs/ecosystems.md) | detection, commands, output parsing, test selection per ecosystem |
| [Configuration](docs/configuration.md) | `.hookah.toml` and the cache (in progress) |
| [Mutation testing](docs/mutation.md) | mutation-testing `hookah` itself |
| [Changelog](https://izevg.github.io/hookah/changelog.html) | every release, generated from Conventional Commits |
| [Harness setup](docs/setup/hooks/cli/claude-code.md) | one page per harness, linked in Quick start |
| [Setup for agents](docs/setup/agents.md) | the exact procedure an agent follows to install and configure `hookah` |

The same pages are published as a searchable site at <https://izevg.github.io/hookah/>.

## Contributing

Read [AGENTS.md](AGENTS.md) first. It holds the binding rules for humans and agents: English
only, Conventional Commits, the rule of four for the file tree, the DDD + hexagonal layering,
and the quality gates (`cargo fmt`, `cargo clippy`, `cargo test`).

## License

[Apache License 2.0](LICENSE).
