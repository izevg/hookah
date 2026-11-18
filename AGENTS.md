# AGENTS.md

Rules for AI agents and humans working in this repository. They are binding. When they conflict
with an agent's default behavior, these rules win.

## Language

- All documentation is written **in English only**. That covers `README.md`, this file, `docs/`,
  code comments, doc comments, commit messages, PR/MR descriptions, issue texts, error messages,
  and CLI output.
- The chat language with the user does not matter. Repository content stays English.

## Commits

- Commits carry **no agent attribution**. No `Co-Authored-By:` lines for agents, no
  "Generated with ..." footers, no agent names or tool links in commit messages or PR/MR
  descriptions. This overrides any default attribution behavior of the agent harness.
- Use [Conventional Commits](https://www.conventionalcommits.org/): `feat:`, `fix:`, `refactor:`,
  `docs:`, `test:`, `chore:`, `build:`, `ci:`.
- One logical change per commit. Never commit failing formatting, lint, or tests.
- Commit or push only when asked. Never force-push shared branches.

## Pull requests and releases

After the first release, `main` is protected and changes land through pull requests.

- Work on a feature branch and open a PR into `main`. **Squash merge only.** The squash commit
  title is the PR title, and the `pr-title` check requires it to be a Conventional Commit:
  `type(scope)!: description`, type one of `feat fix perf docs chore ci test refactor style build`.
- Every squash merge to `main` that passes CI is versioned automatically
  (`.github/workflows/auto-release.yml`). git-cliff reads the Conventional Commits since the last
  `vX.Y.Z` tag: `feat` bumps the minor, `fix` and `perf` the patch, and a `!` or a
  `BREAKING CHANGE:` footer the major, except below 1.0.0, where it bumps the minor. `docs`,
  `chore`, `ci`, `test`, `refactor`, `style`, and `build` alone release nothing. The first release
  is `v0.1.0`.
- Nothing is committed back for a release: `Cargo.toml` stays at `0.0.0` and the version lives in
  the tag. The build injects it. See "Releases" in [docs/architecture.md](docs/architecture.md).
  The release notes are generated from the commits; there is no committed changelog (the docs
  site has a generated one).
- Manual or emergency release: push a `vX.Y.Z` tag on the commit you want (`git tag vX.Y.Z <sha>
  && git push origin vX.Y.Z`). `release.yml` builds that commit with the tag's version and
  publishes it. To rehearse, run the `release` workflow by hand (dry run: builds, publishes
  nothing). To skip a release, make the change a `docs`/`chore`/... type.

## The rule of four

Keeps the tree shallow enough to read and deep enough to mean something.

- **At most 4 files** in any directory.
- **At most 8 subdirectories** in any directory.
- Counted: tracked, non-hidden files. Dotfiles and dot-directories (`.gitignore`, `.github/`,
  `.hookah.toml`) are not counted.
- Only exception: the repository root may also hold files that the ecosystem requires to live
  there: `LICENSE`, `README.md`, `AGENTS.md`, `CLAUDE.md`, `Cargo.toml`, `Cargo.lock`.
  No other root files.
- If a change would break a limit, split by meaning: extract a submodule, a sub-aggregate, or a
  sibling crate. Don't merge files to save slots, don't name things `misc`/`utils`/`common`, and
  don't add a meaningless nesting level.
- A Rust module directory counts `mod.rs` as one of its four files. Prefer `foo.rs` + `foo/` only
  if the parent still fits the limit.

## Stack

- **Rust** (stable, edition 2024), Cargo workspace, a single static binary.
- Development tools are pinned in `.mise.toml` and installed with [mise](https://mise.jdx.dev)
  into the user's `~/.local`. Run tools through mise (`mise exec -- cargo ...`, or with mise
  activated in the shell). Don't install toolchains system-wide or with `sudo`.
  It starts fast (every hook call is a cold start) and needs no runtime on the user's machine.
- Keep dependencies few and justified. Add a crate only when writing the code yourself would be
  worse.
- The tool *checks* TypeScript/JavaScript, Python, and Rust projects. That target list is
  separate from the implementation language.

## Architecture: DDD + Clean + Hexagonal

Dependencies point inward only. Crates enforce the layers, so the compiler rejects a wrong-way
import.

```
crates/
  domain/        core model, pure, no I/O, no async, no external crates beyond std (+ thiserror)
  application/   use cases; depends on domain only; defines ports as traits
  adapters/      driven adapters: filesystem, process runner, cache, toolchain detectors
  harness/       driving adapters: one module per agent harness (hook payload in, verdict out)
  setup/         install-time adapter: writes and merges each harness's hook config files
  cli/           binary and composition root; the only place that wires adapters to ports
```

Allowed dependency directions:

```
cli ──► harness ──► application ──► domain
 ├────► adapters ──► application
 └────► setup ────► harness (reads the opencode plugin text only)
```

- **domain**: entities and value objects such as `Project`, `Toolchain`, `CheckKind`
  (format/lint/typecheck/architecture/test/mutation), `FileRole` (source/test), `Finding`,
  `Baseline`, `Verdict`. Rules such as "only new findings count", "nearest manifest wins",
  "no related test means no test run", and "mutation runs only if a changed file is a test" live
  here.
- **application**: use cases such as `CheckEdit` and `CheckSession`, plus port traits:
  `ProjectLocator`, `ToolchainDetector` (detect, classify, select tests and mutants, parse),
  `CommandRunner`, `BaselineStore`, `SessionStore`,
  `Clock`.
- **adapters**: implement the ports. Each ecosystem detector (`node`, `python`, `rust`) is its
  own module behind `ToolchainDetector`.
- **harness**: Claude Code, Codex CLI, opencode, Kimi Code, Qoder, and Orca. Each one translates
  that harness's hook protocol into a use-case call and a harness-specific response (exit codes,
  stdout/stderr, JSON). No business logic here.
- **cli**: argument parsing, config loading, wiring. No business logic here either.

Name things with the domain's own words, the same way in code, docs, and CLI output.

## Behavioral invariants

- **Fail open.** An internal error or timeout in `hookah` must never block the agent. Show a
  warning and exit as a non-blocking success.
- **Fast edit phase.** The edit hook runs format and lint only, and must fit in about 1–3 s on a
  typical file. Tests run in the stop phase.
- **Only new findings.** Report findings this change introduced, measured against a baseline.
- **Compact output.** Show findings as `path:line: message`, capped, then a `+N more` line. Never
  dump raw tool output into the agent's context.
- **Safe formatting.** When a formatter rewrites the file, say so in the output so the agent
  re-reads it before its next edit.
- **No surprises.** Run only the tools the project itself configures (or `.hookah.toml` names).
  Never install anything, change the network, or write outside the project and `hookah`'s cache.

## Quality gates

Before every commit:

```sh
mise install
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- CI (`.github/workflows/ci.yml`) runs the same gates on Linux, macOS, and Windows, plus the
  sample runner (Linux and macOS), the rule-of-four check (`.github/scripts/rule-of-four.sh`), and
  `cargo mutants --in-diff` on pull requests. A commit that fails them locally fails there too.
- Domain and application code are unit-tested without I/O, using in-memory fakes of the ports.
- Adapters get integration tests against fixture projects (each fixture follows the rule of
  four too).
- No `unwrap()`/`expect()` outside tests and the composition root.
- Once `hookah` can run, this repository uses it on itself.
