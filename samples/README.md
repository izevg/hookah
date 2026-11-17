# Samples

Small, realistic projects with scenarios of the form "the agent makes this edit, and `hookah`
answers this". Each scenario is both documentation and an end-to-end test: one command replays
every scenario through `hookah hook claude` and diffs the answer against a committed
`expected.out`.

```sh
samples/run.sh                 # run everything, exit non-zero on any FAIL
samples/run.sh rust/workspace  # only scenarios whose "<eco>/<config>/<scenario>" contains this
samples/run.sh --update        # rewrite expected.out files; review the git diff before committing
HOOKAH_BIN=/path/to/hookah samples/run.sh   # use another binary instead of building one
```

Without `HOOKAH_BIN`, the runner builds `target/release/hookah` once. Tools come from the
repository's pinned toolchain through `mise exec` (`MISE=/path/to/mise` overrides
`~/.local/bin/mise`; without mise the runner uses the `PATH` as is).

## Layout

```
samples/
  run.sh
  <ecosystem>/                 rust, node, python
    <config>/                  one project configuration
      project/                 a self-contained project
      scenarios/
        setup.sh               optional, runs once per sample
        <NN-name>/
          change.patch         unified diff, applied with `git apply` at the project root
          event                one line: `edit <project-relative path>` or `stop`
          expected.out         normalized output of the hook call
          requires             optional, see below
```

Everything obeys the rule of four from `AGENTS.md` (at most 4 files and 8 directories per
directory, dotfiles aside), including the projects inside `project/`.

## What the runner does

1. Copies `project/` to a temp dir, runs `scenarios/setup.sh` there if it exists, then makes it a
   git repository with one commit (the pristine state). Git is configured with a local identity
   and `GIT_CONFIG_GLOBAL=/dev/null`.
2. For each scenario: resets the copy (`git checkout -- . && git clean -fdq`; ignored dirs such as
   `target/`, `node_modules/`, `.venv/` survive), applies `change.patch`, builds a Claude Code
   payload from `event`, and runs `hookah hook claude` with the copy as cwd.
   - `edit <path>` becomes `PostToolUse` + `Edit` with the absolute `file_path`.
   - `stop` becomes `Stop` with `stop_hook_active: false`.
3. Writes `exit: <code>`, `--- stdout` (JSON pretty-printed with `jq` if present), and
   `--- stderr`, with the copy path replaced by `<root>`, and compares it with `expected.out`.

### `setup.sh`

`scenarios/setup.sh` runs once per sample, inside the copy, before the initial commit. Use it for
`npm ci`, `uv sync`, and the like. Generated directories must be git-ignored by the project's own
`.gitignore`, so scenario resets keep them. A failing setup fails the whole sample. Rust samples
need none. The Node samples install from the npm registry (`npm ci`, `pnpm install
--frozen-lockfile`), so the first run needs network access; later runs hit the package-manager
cache. Node and pnpm come from the pinned `.mise.toml`. (It counts toward the four files of `scenarios/`.)

### `requires`

A scenario may hold a `requires` file: a shell snippet run in the copy. If it exits non-zero the
scenario is reported as `SKIP` instead of running. Use it when the expected output depends on what
is installed on the machine.

## Adding a sample

1. Create `samples/<eco>/<config>/project/` with a `.gitignore` for generated dirs. A Rust crate
   needs an empty `[workspace]` table so the repository workspace does not capture it.
2. Make a scenario directory: edit a copy of the project, save `git diff` as `change.patch`, write
   the `event`, then run `samples/run.sh --update <eco>/<config>`.
3. Read `expected.out` critically. If hookah's answer looks wrong, fix hookah or file a bug; do not
   commit a wrong answer as the expectation.

## Matrix

| Sample | Scenario | Event | Shows |
| --- | --- | --- | --- |
| `rust/crate` | `01-clean-edit` | edit | a harmless edit stays silent (exit 0) |
| | `02-compile-error` | edit | call of an undefined function: exit 2, error at the line |
| | `03-new-clippy-warning` | edit | a warning on a changed line is reported |
| | `04-old-warning-untouched` | edit | a committed warning in the same file is not reported |
| | `05-format-only` | edit | rustfmt rewrites the file; the agent is told to re-read it |
| | `06-broken-test-stop` | stop | a broken assertion fails the stop and names the test |
| | `07-test-edit-mutation` | stop | a weakened test without `cargo-mutants` installed: silent; skipped when it is installed (see below) |
| | `08-test-edit-mutation-survivor` | stop | the same weakened test with `cargo-mutants`: surviving mutants of `clamp` fail the stop; skipped without the tool |
| `rust/workspace` | `01-core-api-break` | edit | a `core` API rename that breaks `app`: the edit phase also lints dependents, so the error in `app` is reported |
| | `02-stop-core-change` | stop | a `core` change: stop runs the tests of `core` and of `app` |
| | `03-core-api-break-stop` | stop | the same API break, caught at stop in `app`; clippy and `cargo test` both report E0599 there, counted once |
| `python/uv-src-layout` | `01-clean-edit` | edit | a docstring edit stays silent (exit 0) |
| | `02-lint-error` | edit | ruff `F401` on a changed line |
| | `03-type-error` | edit | a wrong return type: `[typecheck]` finding from mypy |
| | `04-architecture-violation` | edit | `domain` imports `infra`: `[architecture]` finding from import-linter |
| | `05-format-only` | edit | ruff rewrites the file; the agent is told to re-read it |
| | `06-broken-test-stop` | stop | a broken assertion fails the stop and names the test (pytest-testmon) |
| | `07-weak-test-mutation` | stop | a weakened test: surviving mutants of `cart.py` via mutmut 3 |
| `python/venv-flat-layout` | `01-black-rewrite` | edit | black (no ruff) rewrites the file; re-read hint |
| | `02-type-error` | edit | mypy 1.10 has no JSON output, so the text parser reads the error |
| | `03-failing-test-stop` | stop | a source change breaks `test_stats.py`, found by the naming convention (no testmon) |
| | `04-test-edit-mutmut-missing` | stop | a weakened test without mutmut: passes, with a `tool not installed` warning carrying the `pip install mutmut` hint |
| | `05-test-edit-failing-mutmut-missing` | stop | a failing test edit without mutmut: the failure plus the `pip install mutmut` hint |
| `node/biome-vitest` | `01-clean-edit` | edit | a harmless edit stays silent |
| | `02-lint-error` | edit | a Biome lint error on a changed line is reported |
| | `03-type-error` | edit | a wrong argument type: `[typecheck]` error from `tsc` |
| | `04-architecture-violation` | edit | `domain` imports `infra`: `[architecture]` error from dependency-cruiser |
| | `05-format-only` | edit | Biome rewrites the file; the agent is told to re-read it |
| | `06-broken-test-stop` | stop | a broken assertion fails the stop and names the Vitest test |
| | `07-weakened-test-stop` | stop | a weakened test: Stryker's surviving mutants fail the stop |
| `node/eslint-prettier-jest` | `01-eslint-error` | edit | an ESLint (flat config) error is reported |
| | `02-prettier-rewrite` | edit | Prettier rewrites the file; the agent is told to re-read it |
| | `03-failing-test-stop` | stop | a failing Jest test fails the stop; the missing Stryker notice carries the `npm i -D` hint |
| | `04-test-edit-no-stryker` | stop | a test edit without Stryker installed: passes, with a `tool not installed` warning carrying the `npm i -D` hint |
| `node/pnpm-workspace` | `01-lib-api-break` | edit | a `lib` type change that breaks `app`: the workspace solution `tsc -b` also covers the dependent, so the break is reported in `app` |
| | `02-lib-api-break-stop` | stop | the same change at stop: the `lib` tests, the `app` tests (run from `packages/app`), and the `app` type error are all reported |
| | `03-app-type-error` | edit | a type error in `app` against `lib` through `tsc -b`, shown with its workspace-relative path |

`rust/crate/07-test-edit-mutation` records the answer for a machine without `cargo-mutants`, so
its `requires` is `! cargo mutants --version`. `08-test-edit-mutation-survivor` is its sibling for
a machine that has the tool (`requires`: `cargo mutants --version`). Each machine runs exactly one
of the two.

## Python samples

Both Python samples need network access on the first `setup.sh` (PyPI through `uv`); later runs
reuse uv's cache. Python 3.13 and uv are pinned in the repository's `.mise.toml`. The tools live
in each sample's `.venv`, which hookah finds first (see `docs/ecosystems.md`).

- `python/uv-src-layout` is a uv project with a src layout and a committed `uv.lock`; `setup.sh`
  runs `uv sync --frozen`. It uses ruff, strict mypy, import-linter, pytest with pytest-testmon,
  and mutmut 3. testmon's first run executes the whole suite, so `setup.sh` runs
  `pytest --testmon` once before the pristine commit; scenario runs are incremental.
- `python/venv-flat-layout` is a flat layout with a plain venv made by `uv venv` and `uv pip
  install -r requirements-dev.txt` (all versions pinned). It uses black, mypy configured in
  `setup.cfg` (pinned to 1.10 on purpose, to exercise the text-output parser), and plain pytest.
  mutmut is deliberately not installed.
- `PYTHONHASHSEED=0` is set in both setups. No poetry sample exists yet.
