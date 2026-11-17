# Ecosystems

For each ecosystem: how its manifests are recognized, which tools are detected and in what order
of preference, how output is parsed, and how related tests are selected.

## Node (TypeScript / JavaScript)

Implemented. The project root is the directory of the nearest `package.json`. Files with the
extensions `ts tsx js jsx mjs cjs mts cts` are checked.

**Detection.**

- Workspace root: the nearest ancestor directory (the package itself included) with a
  `pnpm-workspace.yaml` or a `package.json` that has `workspaces`. The search stops at the first
  directory that contains `.git`. Without one, the package is its own root.
- Package manager: the nearest lockfile from the package up to the workspace root
  (`pnpm-lock.yaml`, `yarn.lock`, `bun.lock`/`bun.lockb`, `package-lock.json`), else the
  `packageManager` field, else npm. It only shapes install hints.
- A tool is *available* only when its package is in `dependencies` or `devDependencies` of the
  package or the workspace root **and** its binary exists in `node_modules/.bin`, looked up from
  the package up to the workspace root (so hoisted installs work). Binaries run directly from
  `.bin` with the project root as working directory, never through `npx`, so each tool reads the
  project's own config. `node` must be on `PATH`, as for any `.bin` script.
- Tools that are declared but not installed, or not configured, are simply absent. The one
  exception is mutation testing (see below).

| Check        | Tool       | Used when                              | Command (cwd = project root)                                                                                  | Timeout |
|--------------|------------|----------------------------------------|---------------------------------------------------------------------------------------------------------------|---------|
| format       | biome      | `@biomejs/biome` and `biome.json(c)`   | `biome format --write --reporter=json --no-errors-on-unmatched <files>`                                        | 30 s    |
| format       | prettier   | otherwise, `prettier` present          | `prettier --write --ignore-unknown <files>`                                                                    | 30 s    |
| lint         | biome      | as for format, Biome 1.9 or newer      | `biome lint --reporter=json --no-errors-on-unmatched --max-diagnostics=200 <files>`                            | 60 s    |
| lint         | eslint     | flat or legacy config found            | `eslint --format json <files>`                                                                                 | 60 s    |
| typecheck    | tsc        | `typescript` and a `tsconfig.json`     | `tsc --noEmit --pretty false -p tsconfig.json --incremental --tsBuildInfoFile <cache>/tsc.tsbuildinfo`          | 120 s   |
| typecheck    | tsc        | same, with project `references`        | `tsc -b --pretty false tsconfig.json`                                                                          | 120 s   |
| architecture | depcruise  | `dependency-cruiser` and a `.dependency-cruiser.{js,cjs,mjs,json}` | `depcruise --config <abs config> --output-type json <files>`                       | 60 s    |
| test         | vitest     | `vitest` present (preferred)           | `vitest related --run --reporter=json --outputFile=<cache>/vitest-report.json --passWithNoTests <files>`       | 300 s   |
| test         | jest       | `jest` present                         | `jest --ci --json --no-watchman --passWithNoTests --findRelatedTests <files>`                                  | 300 s   |
| mutation     | stryker    | `@stryker-mutator/core` present        | `stryker run --incremental --reporters json --mutate <comma-joined files>`                                     | 480 s   |

`<cache>` is `$XDG_CACHE_HOME/hookah/<hash of project root>/` (macOS fallback
`~/Library/Caches`, elsewhere `~/.cache`), created with mode `0700`; without it the incremental
flags are dropped.

**Preference rules.**

- Format: Biome when `@biomejs/biome` is present and a `biome.json`/`biome.jsonc` exists in the
  package or up to the workspace root, else Prettier. It is `biome format`, not `biome check
  --write`, so the formatter never reorders imports or applies lint fixes.
- Lint: Biome (configured) and ESLint (any config file, or `eslintConfig` in `package.json`) both
  run when both are configured. Biome older than 1.9 has no JSON reporter and is skipped for lint.
- Test: Vitest, unless `scripts.test` names jest and not vitest. Both rely on the runner's own
  import graph; hookah never guesses which tests to run.
- A project may be TypeScript without `tsc`, or have tests without a runner. Nothing is invented
  for it, and the only missing-tool notice is the one for mutation testing.

**Typecheck.** `tsc` checks the whole program, so errors in other files are reported too: an edit
can break its callers. Plain projects run `--noEmit` with the incremental state in the cache
(TypeScript 4.0 or newer). A tsconfig with `references` (including a solution-style
`"files": []` root) runs `tsc -b`: a plain `-p` run fails with `TS6305` for every import of a
referenced project that is not built, and checks nothing for a solution root. Build mode is
incremental by itself and rejects `--noEmit` for referenced projects (`TS6310`), so it writes the
build outputs the projects declare (`outDir`, `*.tsbuildinfo`), exactly like the project's own
`tsc -b`. Only the project's `tsconfig.json` is considered; test-only configs such as
`tsconfig.spec.json` are not.

**Output.**

- Biome (v1.9 and v2) and ESLint JSON; Prettier syntax errors; `tsc` `path(line,col): error
  TSxxxx` lines (no-location diagnostics sit at `tsconfig.json`). Biome `information` and `hint`
  are dropped.
- dependency-cruiser: `error` becomes an error, `warn` a warning, `info` and `ignore` are
  dropped. The location is the `from` module (the JSON carries no line) and the message is
  `rule: from -> to`.
- Vitest and Jest: every failed assertion is an error at the test file, on the line of the first
  stack frame inside it. A suite that cannot run (syntax or import error) is one error. Vitest 5
  writes its report to a file and says where; older versions print it. Jest runs with
  `--no-watchman` because it waits a minute for a missing watchman otherwise.
- A tool that exits non-zero without a readable report is degraded with the tail of its output,
  never a pass.

**Test selection.** The changed files with a Node extension under the project (not in
`node_modules`) become the targets, project-relative. The runner's `related` mode finds the tests
that import them. No runner, or no such file, selects nothing.

**Mutation.** Changed tests map to sources by name, keeping files that exist: `foo.test.ts` and
`foo.spec.ts` to `foo.ts` next to them; `__tests__/foo.ts` to `../foo.ts`; `test/foo.spec.ts` or
`tests/foo.spec.ts` to `src/foo.ts`, then `lib/foo.ts`, then `foo.ts` at the sibling level, keeping
the sub-path below the test directory. Extensions tried: `ts tsx js jsx mts cts mjs cjs`. Changed
sources count directly; `.d.ts` files, files with glob characters or commas in the name, and
anything past the first 10 files are dropped. The result is one comma-joined target for Stryker's
`--mutate` list.

- Stryker runs with `--incremental`, so only mutants of changed code are re-run. The incremental
  file stays where the project's Stryker config puts it, by default `reports/stryker-incremental.json`
  in the project; the JSON report is `reports/mutation/mutation.json` unless the project sets
  `jsonReporter.fileName`. The parser reads the report path from Stryker's own
  `Your report can be found at: file://...` log line, so it is never a stale file. A crashed run
  can leave `.stryker-tmp/` behind. `--reporters json` replaces the project's reporters for this
  run only.
- Only `Survived` mutants become findings, as `mutant survived: <mutator> <replacement>` errors at
  the mutant's line. `NoCoverage` mutants are left out: they say "no test reaches this code",
  not "the tests are weak", the whole file is mutated (not just the changed lines), so they
  would flood the report with pre-existing gaps; the test runner and a coverage tool answer that
  question better. `Timeout` counts as killed.
- Stryker needs a test-runner plugin and, usually, a `stryker.config.json`; without one it falls
  back to `npm test`. When `@stryker-mutator/core` is not available and the project has a test
  runner, the notice is `stryker` with the hint `<pm> add -D @stryker-mutator/core
  @stryker-mutator/<vitest|jest>-runner` (`npm i -D`, `pnpm add -D`, `yarn add -D`, `bun add -d`
  by package manager; `run <pm> install` when it is declared but not installed).

**File roles.** Test: `*.test.*`, `*.spec.*`, and files under a `__tests__`, `test`, or `tests`
directory below the project root. Everything else is source.

**Known gaps.** The nearest `package.json` wins, so a marker file such as `src/esm/package.json`
(`{"type":"module"}`) becomes a project with no tools. Nested ESLint or Biome configs are found
only from the package up to the workspace root. `tsc -b` writes build outputs. Windows `.cmd`
shims are not resolved.

## Python

Implemented. The project root is the directory of the nearest `pyproject.toml`. Tool
configuration comes from `pyproject.toml`, `setup.cfg`, `tox.ini`, `ruff.toml`/`.ruff.toml`,
`mypy.ini`/`.mypy.ini`, and `.importlinter`. Both the flat layout (`pkg/` at the root) and the src
layout (`src/pkg/`) work, and every `pyproject.toml` in a monorepo is its own project.

### Environment

Tools run from the project's own environment, the first of:

1. A virtual environment: `$UV_PROJECT_ENVIRONMENT` if set, else `.venv/` or `venv/` (a directory
   with `pyvenv.cfg`) in the project, or in a parent directory up to the repository root (a
   directory with `.git`) or the uv workspace root (`[tool.uv.workspace]`). Tools run as
   `<venv>/bin/<tool>`. This is what `uv sync`, `python -m venv`, and Poetry with
   `virtualenvs.in-project` produce. A member's own venv shadows the root venv.
2. `poetry.lock` and `poetry` on `PATH`: tools run as `poetry run <tool>`. A tool is available when
   its package is in the lock file. Poetry keeps its environments in its cache directory, so this
   is the only way to reach them. If no environment exists yet, `poetry run` may create one there.
3. The tools on `PATH`.

A tool is **available** only if it resolves in that environment: its executable exists in the
venv, its package is in `poetry.lock`, or its executable is on `PATH`. There is no fallback: a
project whose `.venv` lacks `ruff` has no ruff, even if one is installed globally. Nothing is ever
installed, synced, or created.

`uv run --frozen --no-sync` is not used. It does not install, but with no `.venv` uv creates an
empty one before running anything, and hookah must not create environments in the project. A uv
project's environment is always a `.venv` (or `$UV_PROJECT_ENVIRONMENT`), which step 1 finds. An
unsynced uv project (no `.venv`) falls through to `PATH`. `uv.lock` only decides the install hint.

### Tools

Paths are relative to the project root, which is the working directory. `{files}` are the changed
files (edit hook) or all changed files of the project (stop hook).

| Check        | Tool          | Command                                                                              | Timeout |
|--------------|---------------|--------------------------------------------------------------------------------------|---------|
| format       | `ruff`        | `ruff format --force-exclude {files}`                                                | 30 s    |
| format       | `black`       | `black {files}`; only without ruff, and only if `[tool.black]` exists or black is a declared dependency | 30 s |
| lint         | `ruff`        | `ruff check --output-format json --force-exclude --no-fix {files}`                   | 30 s    |
| typecheck    | `mypy`        | `mypy [-O json] --show-column-numbers --show-error-codes --no-error-summary --no-color-output {files}` | 120 s |
| architecture | `lint-imports`| `lint-imports` (project-wide; `env PYTHONPATH=src` is prepended in the src layout)   | 60 s    |
| test         | `pytest`      | `pytest [--testmon] -rfE --tb=short -q --color=no --rootdir=. {tests}`               | 300 s   |
| mutation     | `mutmut`      | `mutmut run {globs}` (mutmut 3 only)                                                 | 480 s   |

- **ruff** is preferred for format and lint when available. `--no-fix` keeps a project's
  `fix = true` from rewriting files during lint; `--force-exclude` makes ruff honor the project's
  `exclude` for explicitly passed files. Ruff has no warnings, so every diagnostic is an `Error`
  and its code (`F401`) is the rule. Syntax errors come through as findings.
- **mypy** runs when it is available and configured (`[tool.mypy]`, `mypy.ini`, `.mypy.ini`,
  `setup.cfg [mypy]`, `tox.ini [mypy]`, which gets `--config-file tox.ini`) or declared as a
  dependency. `-O json` is used only when the installed mypy is known to be 1.11 or newer (read
  from the venv's `dist-info`); otherwise the text output is parsed. mypy keeps its default
  `.mypy_cache`. It checks imported modules too, so findings can come from other files; they are
  reported on purpose.
- **import-linter** runs when configured (`[tool.importlinter]`, `.importlinter`,
  `setup.cfg [importlinter]`). Each illegal chain of a broken contract is one finding, such as
  `src/shop/domain/cart.py:2: [architecture] shop.domain.cart -> shop.infra.db [Domain must not
  import infra]`: located at the first importing module's file and line (`(l.N)`), message the
  chain, rule the contract name. Module names map to files in the src or flat layout. The root
  package must be importable; the src layout gets `PYTHONPATH=src` for that.
- **pytest** runs `-rfE --tb=short -q`; no JUnit file is written. Failures come from the
  `short test summary info` lines plus the short tracebacks: a failing test is located at the last
  traceback frame inside its own test file, so an exception raised deep in the code under test is
  reported at the test line that triggered it. Collection errors (import errors, syntax errors)
  are located the same way. `--rootdir=.` keeps node ids relative to the project root.
  pytest's default `.pytest_cache` stays.
- **mutmut** is read from the `Mutant results` list that `mutmut run <globs>` prints, which covers
  exactly the selected mutants, including those answered from mutmut's own cache in `mutants/`.
  Each survivor is an `Error`: `mutant survived: shop.cart.x_total__mutmut_9 (`...else 0` ->
  `...else 1`)`, located at the changed line of the original source. The line is found by
  comparing the mutant with the original function in `mutants/<file>`. mutmut 2.x is not
  supported.

### Missing tools

A `MissingTool` notice (never an install) is raised for: mutmut, when pytest is available but
mutmut is not (or is older than 3); and ruff, mypy, or import-linter, when the project configures
it but the environment lacks it. The hint matches the package manager, judged by lock files:
`uv add --dev <pkg>`, `poetry add --group dev <pkg>`, or `pip install <pkg>`.

### File roles and selection

- **Roles.** `test_*.py`, `*_test.py`, `conftest.py`, and any file below a `tests/` or `test/`
  directory (below the project root) are tests; everything else is source.
- **Tests.** With `pytest-testmon` installed in the venv (or in `poetry.lock`), pytest runs with
  `--testmon` and without file arguments: testmon selects the affected tests, and the targets are
  just the changed Python files. Otherwise a changed test selects itself; a changed
  `pkg/foo.py` selects every `test_foo.py` and `foo_test.py` in the project (`__init__.py` maps
  to `test_<package>.py`); a changed `conftest.py` or test helper selects the test files in its own
  directory. With no match nothing runs, and the whole suite never does.
- **Mutants.** The sources are the changed sources plus the sources of the changed tests
  (`test_foo.py` and `foo_test.py` map to `foo.py` anywhere in the project; among several
  same-named files, those whose directories overlap most with the test's win). Each source becomes
  one glob `<module>.x[_ǁ]*`, where the module is mutmut's name for the file (`src/shop/cart.py`
  is `shop.cart`). Files that do not exist are dropped, and nothing selected means no run.

### Caveats

- The first `pytest --testmon` run executes the whole suite to build `.testmondata`. The first
  `mutmut run` runs the suite once to collect stats and copies the project into `mutants/`. Both
  are project-local caches; add them (and `.mypy_cache`, `.ruff_cache`, `.pytest_cache`,
  `.import_linter_cache`) to `.gitignore`.
- mutmut only mutates files under its configured `source_paths` (by default `src/`, `lib/`, or the
  package named like the directory). A glob that matches nothing is a clean result, not an error.
- mutmut aborts when the baseline test run fails (`failed to collect stats`); that is reported as
  a degraded run, not as a finding.
- `pytest` and `mypy` need the project's packages importable in the environment, as for the user
  (editable install, `pythonpath`, or `mypy_path`). Only import-linter gets `PYTHONPATH=src`.
- If pytest's `rootdir` was meant to be a parent directory, `--rootdir=.` changes what it
  considers the root; keep a `pyproject.toml` per project.

## Rust

Implemented. The project root is the directory of the nearest `Cargo.toml`; `cargo metadata
--no-deps` gives the package name, edition, and workspace root. A virtual workspace manifest has
no toolchain, and a missing `cargo` degrades the run instead of failing it.

| Check  | Tool         | Command (cwd = project root)                                                      |
|--------|--------------|-----------------------------------------------------------------------------------|
| format | `rustfmt`    | `rustfmt --edition <edition> <file>` (also formats the file's out-of-line modules) |
| lint   | `clippy`     | `cargo clippy -p <pkg> [-p <dependent>]... --all-targets --message-format=json --quiet` |
| test   | `cargo-test` | `cargo test --message-format=json --quiet --package=<pkg> ... --no-fail-fast`      |

- Output: clippy and cargo JSON compiler messages (error and warning, primary span, rule from the
  diagnostic code, duplicates across targets merged, external-crate spans dropped); rustfmt parse
  errors from stderr; failed libtest tests located at their `panicked at` position. A tool that
  fails without any finding degrades the run.
- Paths are relative to the project root, so a sibling crate shows as `../sibling/src/x.rs`.
- Lint scope: the package plus the workspace members that depend on it by path, so an API change
  that breaks a dependent is caught in the edit phase. Errors in dependents always count; their
  warnings follow the usual changed-lines filter.
- Test selection: any changed `.rs` file or the `Cargo.toml` selects the package plus workspace
  members that depend on it by path; otherwise no tests run.

### Rust mutation testing

| Check    | Tool            | Command (cwd = project root)                                                                                              |
|----------|-----------------|---------------------------------------------------------------------------------------------------------------------------|
| mutation | `cargo-mutants` | `cargo mutants --package <pkg> --file=<path>... --no-shuffle --output <ws>/target/hookah-mutants --iterate -j <n> --timeout-multiplier 3` |

- Available when `cargo mutants --version` succeeds in the project root, so a mise-pinned tool
  counts. When it is missing and the crate has tests (a `#[test]` under `src/` or a `tests/`
  directory), a non-blocking notice says `cargo install --locked cargo-mutants (or pin it in
  mise)`. Nothing is installed. Timeout 480 s. `-j` is half the CPUs, between 1 and 4.
- File roles: files under `tests/`, `benches/`, and `examples/` are tests. In any other `.rs`
  file, the first line starting at column 0 with `#[cfg(test)]` begins the test module; if every
  changed line is at or after it, the edit is a test edit, otherwise (and for a new file) it is
  source. The scan is line-based and never parses Rust.
- Targets: an inline test edit selects its own file; `tests/foo.rs` selects `src/foo.rs` or
  `src/foo/mod.rs`, falling back to the changed sources and then `src/lib.rs` / `src/main.rs`;
  changed `src/**/*.rs` sources are always included; benches and examples select nothing.
  Targets are `--file=<workspace-relative path>` entries, one argv entry each. Files that do not
  exist are dropped.
- Incremental and cached: the output directory is `<workspace>/target/hookah-mutants`, so it is
  ignored by version control and reused between runs. `--iterate` skips mutants an earlier run
  caught or found unviable and re-tests the missed ones. `--in-diff` is not used: mutation runs
  because tests changed, so whole files are the useful scope. `hookah` creates the workspace
  `target/` directory if it is absent, since `cargo-mutants` does not.
- Output: reads `mutants.out/outcomes.json`. A missed mutant is an `Error`,
  `mutant survived: <description>`, at the mutated line. A timed-out mutant is a `Warning`
  (`mutant timed out: ...`): it is usually an endless loop, which is a kind of catch. A failing
  baseline (exit code 4), a whole-run timeout, or any other tool failure degrades the run.
