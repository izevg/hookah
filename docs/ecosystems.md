# Ecosystems

For each ecosystem (Node, Python, Rust, Java): how its manifests are recognized, which tools are detected and in what order
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
| typecheck    | tsc        | `typescript` and a `tsconfig.json`     | `tsc --noEmit --pretty false -p tsconfig.json --composite false --incremental --tsBuildInfoFile <cache>/tsc.tsbuildinfo` | 120 s   |
| typecheck    | tsc        | same, with project `references`        | the same, `-p <owning tsconfig>` for each project that owns the edited file                                    | 120 s   |
| architecture | depcruise  | `dependency-cruiser` and a `.dependency-cruiser.{js,cjs,mjs,json}` | `depcruise --config <abs config> --output-type json <files>`                       | 60 s    |
| unused       | knip       | `knip` declared and installed (stop phase only) | `knip --reporter json --no-progress --no-exit-code --no-config-hints [--workspace <package dir>]`, cwd = workspace root | 120 s |
| test         | vitest     | `vitest` present (preferred)           | `vitest related --run --reporter=json --outputFile=<cache>/vitest-report.json --passWithNoTests --exclude=<heavy glob>... <files>` | 300 s |
| test         | jest       | `jest` present                         | `jest --ci --json --no-watchman --passWithNoTests --testPathIgnorePatterns=<heavy regex>... --findRelatedTests <files>` | 300 s |
| mutation     | stryker    | `@stryker-mutator/core` present        | `stryker run --incremental --reporters json --mutate <comma-joined files>`                                     | 480 s   |

`<cache>` is `<hookah cache>/<hash of project root>/` (see `configuration.md`; macOS fallback
`~/Library/Caches`, elsewhere `~/.cache`), created with mode `0700`; without it the incremental
flags are dropped.

**Preference rules.**

- Format: Biome when `@biomejs/biome` is present and a `biome.json`/`biome.jsonc` exists in the
  package or up to the workspace root, else Prettier. It is `biome format`, not `biome check
  --write`, so the formatter never reorders imports or applies lint fixes.
- Lint: Biome (configured) and ESLint (any config file, or `eslintConfig` in `package.json`) both
  run when both are configured. Biome older than 1.9 has no JSON reporter and is skipped for lint.
- Unused: Knip, when `knip` is declared and installed. It needs no config file; it reads its own
  (`knip.json`, `knip.jsonc`, `knip.ts`, a `knip` key in `package.json`, ...) and finds entry
  files from `package.json` and its plugins. See "Unused code" below.
- Test: Vitest, unless `scripts.test` names jest and not vitest. Both rely on the runner's own
  import graph; hookah never guesses which tests to run.
- A project may be TypeScript without `tsc`, or have tests without a runner. Nothing is invented
  for it, and the only missing-tool notice is the one for mutation testing.

**Typecheck.** `tsc` checks the whole program, so errors in other files are reported too: an edit
can break its callers. The command never writes into the project: `--noEmit` writes no outputs,
`--composite false` drops what a composite project forces (declarations, a `tsconfig.tsbuildinfo`
next to the outputs, the "every file must be listed" rule), and the incremental state goes to
`--tsBuildInfoFile <cache>/...` (or `--incremental false` without a cache directory). It needs
TypeScript 4.0 or newer, and was checked with 5.4, 5.9, 6.0, and 7.0.

A tsconfig with `references` (a solution-style `"files": []` root, as in the Vite template) is
not built: `tsc -b` writes the declared outputs of every referenced project, and rejects
`--noEmit` (`TS5094` before TypeScript 5.6, `TS6310` since) while still writing `*.tsbuildinfo`.
Instead the edited file picks its project. The references are walked recursively, and each config
owns the files its `files`, `include`, and `exclude` select, with `extends` followed (relative
files, packages in `node_modules`, and lists), the `node_modules`, `bower_components`,
`jspm_packages`, and `outDir` defaults, and JavaScript counted with `allowJs`. `tsc -p` then runs
for each owning project, and for each project that references an owner (an API change breaks its
users): an edit in `src/` checks `tsconfig.app.json` only, and the errors of `tsconfig.node.json`
(the Vite config) stay out of it. A file no project owns runs nothing. At
most eight projects run, one incremental state file each. The whole-project baseline runs every
project that lists files.

A project that references another composite project would see that project's declarations, which
exist only after a build: a plain `-p` run reports `TS6305` for each such import and types it as
`any`. So a project with references is checked through a small overlay config that hookah writes
to its cache directory (never into the project). It extends the project's config, drops the
`references` and `composite`, and sets `rootDir` and `typeRoots` explicitly, which otherwise
default to the config's location. The imports of the referenced projects then resolve to their
sources and are checked in full, so an API change in a referenced project is reported in the
project that uses it. Without a writable cache directory the plain `-p` run stands, and hookah
drops its `TS6305` diagnostics, so the project's own code is checked and those imports are not.
Only the project's `tsconfig.json` is the entry point; test-only configs such as
`tsconfig.spec.json` are found when it references them.

**Unused code.** Knip builds the import graph of the whole project (and, in a workspace, of
all packages), which takes seconds, so it runs in the **stop phase only**, never after a single
edit. Its own check kind, `unused`, keeps it apart from dependency-cruiser's `architecture`
(which runs per edit). In a workspace it runs from the workspace root with `--workspace
<package dir>`: from inside a package it cannot see the other packages and would flag their
files and dependencies as unused. The workspace root package runs without `--workspace`.
Every issue is a **warning** with the knip issue type as its rule: `files`, `exports`, `types`,
`enumMembers`, `namespaceMembers`, `duplicates`, `dependencies`, `devDependencies`, `unlisted`,
`binaries`, `unresolved`, ... A message names the symbol and no position, so a baseline still
matches it after lines shift: `src/money.ts:5: warning: unused export: format [exports]`.
Dependency issues point into the `package.json` that declares the dependency. Knip cannot be
limited to changed files, so "light" comes from the reporting rules: with a session baseline only
issues new since the session started are reported, anywhere (an edit that drops the last import
of a dependency reports it in `package.json`); without one, only warnings in changed files and on
changed lines. Exports of entry files are not reported, as in knip itself, and a failed run (such
as a broken `knip.json`) is a non-blocking notice.

**Workspace dependents.** An API change breaks the packages that use it, not the package, so
the edit's checks also cover its dependents, like the Rust adapter does for path dependents.
Members come from `pnpm-workspace.yaml` (`packages:`) or the root `package.json` `workspaces`
(`*`, `**`, `!` exclusions; `node_modules` and hidden directories are skipped). A member is a
direct dependent when one of `dependencies`, `devDependencies`, `peerDependencies`, or
`optionalDependencies` names the package with a `workspace:` spec, `*`, `latest`, or a `^`, `~`,
or exact version the package's own `version` satisfies.

- Typecheck: a workspace-root `tsconfig.json` whose `references` include the package or a
  dependent is a solution config, checked from the workspace root instead of the per-package
  command: the projects that own the edited file, plus the projects of the dependents. Otherwise
  the package's own command runs plus one `tsc --noEmit -p tsconfig.json` per dependent that has a
  `tsconfig.json`, each from the dependent's directory (a dependent with references runs all its
  projects). Nothing is built or emitted.
- Test: every dependent with a test runner gets its own `vitest related` / `jest
  --findRelatedTests` run from its directory, so the tests of a dependent that import the
  changed files run in the stop phase. The changed files are passed as absolute paths, which
  mean the same file to every package's runner.
- Errors found in a dependent are reported as for any other file (never line-filtered); paths
  are shown relative to the agent's directory, such as `packages/app/src/main.ts`.

**Output.**

- Biome (v1.9 and v2) and ESLint JSON; Prettier syntax errors; `tsc` `path(line,col): error
  TSxxxx` lines (no-location diagnostics sit at `tsconfig.json`). Biome `information` and `hint`
  are dropped.
- dependency-cruiser: `error` becomes an error, `warn` a warning, `info` and `ignore` are
  dropped. The location is the `from` module and the message is `from -> to` (the rule name is
  the finding's rule, shown as `[rule]`). The JSON carries no position, so the line is a best
  effort: the first line of the `from` file that holds the violating import's specifier
  (`unresolvedTo`) in quotes; without a match the finding has no line.
- Vitest and Jest: every failed assertion is an error at the test file, on the line of the first
  stack frame inside it (a failed Storybook story: the line of its export, see below). A suite that
  cannot run (syntax or import error) is one error. Vitest 5
  writes its report to a file and says where; older versions print it. Jest runs with
  `--no-watchman` because it waits a minute for a missing watchman otherwise.
- A tool that exits non-zero without a readable report is degraded with the tail of its output,
  never a pass.

**Test selection.** The changed files with a Node extension under the project (not in
`node_modules`) become the targets, absolute. The runner's `related` mode finds the tests
that import them. No runner, or no such file, selects nothing.

**Light tests only.** Integration and end-to-end tests are never run (see "Light by default" in
`architecture.md`). A test is heavy when a directory below the project root is `e2e`,
`integration`, `integrations`, `playwright`, or `cypress`; when a dot segment of the file name is
`e2e`, `int`, or `integration` (`checkout.e2e.ts`, `db.int.test.ts`); or when the stem has the
word `integration`, `e2e`, `playwright`, or `cypress` between `-` and `_` (`api-e2e.test.ts`,
`playwright.config.ts`). Two things follow:

- A changed heavy file is not handed to the runner, and does not select mutation targets.
- The runner is told to skip heavy tests it finds through the import graph: Vitest gets one
  `--exclude=<glob>` per pattern, Jest one `--testPathIgnorePatterns=<regex>` per pattern. Both
  were verified against Vitest 5.1.0 and Jest 30.5.2. Vitest adds the excludes to the project's
  own. Jest replaces the project's `testPathIgnorePatterns` when the flag is given, so hookah
  repeats `/node_modules/`; a project that ignores more (such as `/dist/`) loses that ignore for
  hookah's run. Its patterns are anchored at `<rootDir>`, so a project that lives below a
  directory called `e2e` is not excluded whole. Both are approximations of the word rule, and
  Jest's are case-sensitive.
- The number of targets is the number of changed files, not of tests, because only the runner
  knows the tests; the test budget applies to it.

**Storybook stories.** The Storybook Vitest addon (`@storybook/addon-vitest`, earlier
`@storybook/experimental-addon-test`) turns every `*.stories.*` file into Vitest tests: its
`storybookTest` plugin sits in a Vitest project, usually named `storybook`, that runs in browser
mode (Playwright). hookah adds no tool and no flag for it. It is the same `vitest related` call,
and the project's own Vitest config decides whether stories are tests, so nothing runs unless
the project configured it. Verified with Storybook 10.6, Vitest 5.1 and Playwright Chromium:

- A changed story file selects exactly that file: `vitest related src/Button.stories.tsx` runs
  the stories of that file and nothing else.
- A changed component selects the stories that import it, through the same import graph that
  finds unit tests. No sibling-file naming guess is made. A component used by many stories
  selects many story files, which is what the runner reports; the test budget counts changed
  files, as for every test.
- A story file belongs to the `storybook` project only, so a unit test and a story are never run
  twice, and the unit project is unaffected. An edit that no story imports starts no browser:
  `related` finds nothing, and Vitest does not launch Playwright for it.
- A failed story is an error at the stories file, on the line of its `export const` (the
  browser's stack frames point into transformed code, so they are not used). The message is
  `<story name>: <assertion message>`. The Vitest JSON report already carries stories as test
  results, so the Vitest parser reads them as is.
- Fail open. When Playwright cannot launch its browser (not installed, wrong path), Vitest
  reports the stories suite as failed with no test and no message. hookah turns that into a
  non-blocking notice, `story tests skipped: Playwright could not launch its browser (run `npx
  playwright install`)`, instead of a finding. When a unit test fails in the same run, the
  finding is reported and the notice waits for the next run. hookah never installs browsers.
- Stories are source files for the file role, never test files: editing one alone does not start
  mutation testing, and a story is never a mutation target. Stryker would have to run the
  browser project, which is not light (see `mutation.md`).
- Not supported: the legacy `@storybook/test-runner`. It needs a running Storybook server, so it
  is a heavy, integration-style check and hookah never runs it. Stories in `.vue` and `.svelte`
  files are not selected, because the Node ecosystem only checks JavaScript and TypeScript files.
- The addon recommends `playwright install chromium` for its browser. A CI machine that replays
  the Storybook sample needs that too.

**Mutation.** Changed tests map to sources by name, keeping files that exist: `foo.test.ts` and
`foo.spec.ts` to `foo.ts` next to them; `__tests__/foo.ts` to `../foo.ts`; `test/foo.spec.ts` or
`tests/foo.spec.ts` to `src/foo.ts`, then `lib/foo.ts`, then `foo.ts` at the sibling level, keeping
the sub-path below the test directory. Extensions tried: `ts tsx js jsx mts cts mjs cjs`. Changed
sources count directly; heavy tests map to nothing; `.d.ts` files, files with glob characters or commas in the name, and
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
only from the package up to the workspace root. The owning tsconfig is found from the files and
`include`/`exclude` patterns alone; a package that is imported through its build output (`dist/*.d.ts`)
needs that output to exist, as it does for any plain `tsc` run.

**Windows.** The tool is the `node_modules\.bin\<tool>.cmd` shim that npm, pnpm, and Yarn write
(`<tool>.exe` is the fallback). Rust's standard library runs a `.cmd` through `cmd.exe` and
escapes the arguments itself (the fix for CVE-2024-24576, "BatBadBut"); an argument it cannot
escape safely makes the spawn fail, which `hookah` reports as a tool that could not start and
never as a block. The `sh` scripts of the same names are not used.

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
   `<venv>/bin/<tool>` (`<venv>\Scripts\<tool>.exe`, `.cmd`, or `.bat` on Windows). This is what `uv sync`, `python -m venv`, and Poetry with
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
| typecheck    | `mypy`        | `mypy [-O json] --show-column-numbers --show-error-codes --no-error-summary --no-color-output [{packages} {files outside them}]` | 120 s |
| architecture | `lint-imports`| `lint-imports` (project-wide; `PYTHONPATH=src` is set in the src layout)   | 60 s    |
| unused       | `unref`       | `unref --config pyproject.toml --output-format concise` (preferred)                    | 60 s    |
| unused       | `vulture`     | `vulture` (configured paths)                                                         | 60 s    |
| test         | `pytest`      | `pytest [--testmon --ignore-glob=<heavy>...] [-m "not (<heavy markers>)"] -rfE --tb=short -q --color=no --rootdir=. {tests}` | 300 s |
| mutation     | `mutmut`      | `mutmut run {globs}` (mutmut 3 only)                                                 | 480 s   |

- **ruff** is preferred for format and lint when available. `--no-fix` keeps a project's
  `fix = true` from rewriting files during lint; `--force-exclude` makes ruff honor the project's
  `exclude` for explicitly passed files. Ruff has no warnings, so every diagnostic is an `Error`
  and its code (`F401`) is the rule. Syntax errors come through as findings.
- **mypy** runs when it is available and configured (`[tool.mypy]`, `mypy.ini`, `.mypy.ini`,
  `setup.cfg [mypy]`, `tox.ini [mypy]`, which gets `--config-file tox.ini`) or declared as a
  dependency. `-O json` is used only when the installed mypy is known to be 1.11 or newer (read
  from the venv's `dist-info`); otherwise the text output is parsed. mypy keeps its default
  `.mypy_cache`, which keeps whole-project runs fast. An edit can break the modules that import
  it, and mypy only reports a module it is asked to check, so it checks the whole project: when
  its configuration sets `files`, `packages`, or `modules` (in `[tool.mypy]` or a `[mypy]`
  section of `mypy.ini`, `.mypy.ini`, `setup.cfg`, or `tox.ini`) it runs with no arguments and
  checks exactly that; otherwise it gets the project's top-level packages (`src/<pkg>` in the
  src layout, the root directories with an `__init__.py` in the flat layout) followed by the
  changed files that lie outside those packages (a file given twice is a mypy error). Errors in
  any file are reported;
  warnings only on changed lines.
- **import-linter** runs when configured (`[tool.importlinter]`, `.importlinter`,
  `setup.cfg [importlinter]`). Each illegal chain of a broken contract is one finding, such as
  `src/shop/domain/cart.py:2: [architecture] shop.domain.cart -> shop.infra.db [Domain must not
  import infra]`: located at the first importing module's file and line (`(l.N)`), message the
  chain, rule the contract name. Module names map to files in the src or flat layout. The root
  package must be importable; the src layout gets `PYTHONPATH=src` for that.
- **Unused code.** Hookah prefers Unref when the project declares it as a dependency or provides a
  `[tool.unref]` table in `pyproject.toml`. Unref scans the project root and applies its own
  exclusions, Git ignore rules, and framework settings. Hookah requests concise output so it can
  report each finding with its file, line, and column.

  Use `[tool.vulture]` to opt into Vulture. Hookah runs Vulture if Unref has not been opted into
  or is missing from the project environment. If the project configures both and Unref is available,
  only Unref runs. When a configured Unref is missing, Hookah reports that alongside any Vulture result.

  For Vulture, set `paths` to all sources and tests that should count as uses.

  ```toml
  [tool.vulture]
  paths = ["src", "tests"]
  ```

  Vulture reads its own exclusions, minimum confidence, name and decorator ignores, and any
  whitelist files included in `paths`. Leave `make_whitelist` disabled for diagnostic output.

  Both analyzers check cross-file references during baseline collection and the light stop phase.
  The baseline keeps only new findings, including when a removed use makes a symbol unused in
  another file. Without a ready baseline, only warnings on changed lines are reported. A tool
  failure, incomplete scan, or timeout produces a non-blocking warning.
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
mutmut is not (or is older than 3); and ruff, mypy, import-linter, Unref, or Vulture, when the project configures
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
- **Light tests only.** A test is heavy when a directory below the project root is `integration`,
  `integrations`, `e2e`, `playwright`, or `cypress`, or when the file stem has the word
  `integration`, `e2e`, `playwright`, or `cypress` between `_` and `-` (`test_cart_integration.py`).
  Heavy files are dropped from every selection and select no mutants. Under testmon, which picks
  the tests itself, pytest gets `--ignore-glob=<root>*/<dir>/*` and file patterns, anchored at the
  escaped project root so a project below a directory called `e2e` is not ignored whole (verified
  on pytest 9.2). Markers: when the project registers `integration`, `e2e`, or `slow` under
  `markers` in the one config pytest reads (`pytest.ini`, else `pyproject.toml` with
  `[tool.pytest.ini_options]`, else `tox.ini` `[pytest]`, else `setup.cfg` `[tool:pytest]`),
  pytest gets `-m "not (integration or e2e or slow)"` naming only the registered ones. A marker the
  project does not register never appears in the expression, so `--strict-markers` cannot object.
- **Mutants.** The sources are the changed sources plus the sources of the changed tests
  (`test_foo.py` and `foo_test.py` map to `foo.py` anywhere in the project; among several
  same-named files, those whose directories overlap most with the test's win; heavy tests map to
  nothing). Each source becomes
  one glob `<module>.x[_ǁ]*`, where the module is mutmut's name for the file (`src/shop/cart.py`
  is `shop.cart`). Files that do not exist are dropped, and nothing selected means no run.

### Caveats

- The first `pytest --testmon` run executes the whole suite to build `.testmondata`. The first
  `mutmut run` runs the suite once to collect stats and copies the project into `mutants/`. Both
  are project-local caches; add them (and `.mypy_cache`, `.ruff_cache`, `.pytest_cache`,
  `.import_linter_cache`) to `.gitignore`.
- Python's bytecode cache (`__pycache__`) trusts the source's mtime and size. A file rewritten
  within the same second to a different version of the same size keeps its stale `.pyc`, and
  pytest then runs the old code. Scripts that restore files quickly (such as `samples/run.sh`)
  should set `PYTHONDONTWRITEBYTECODE=1` and delete `__pycache__`.
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
- Light tests only: `#[ignore]` tests are skipped by libtest as usual, benches are never run
  (`cargo test` skips `benches/`), and heavy integration targets are left out. A `tests/` target
  is heavy when its file or directory name is `integration`, `e2e`, or has that word between `_`
  and `-` (`tests/integration_db.rs`, `tests/checkout_e2e.rs`, `tests/e2e/main.rs`, and also a
  `tests/integration/` directory). Cargo cannot skip a target, only pick targets, so when the
  package has a heavy one the selection becomes explicit: `--package=<pkg>` ... `--lib` `--bins`
  `--test=<light target>`... (`--lib` and `--bins` only when the package has them; nothing light
  at all selects nothing). The price: doctests do not run in that mode (cargo cannot combine
  `--doc` with other target flags), and only the package's own `tests/` directory is inspected,
  not those of the dependents, which get `--lib`, `--bins`, and the same test names. Without a
  heavy target the selection stays the plain `--package` list. A Rust target is a package or one
  test binary, and the test budget counts them.

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
- Heavy tests select nothing: a changed `tests/integration_*.rs` or `tests/e2e/...` file is a test
  edit but maps to no source.
- Small changes only: see the mutation budget in `mutation.md`.
- Incremental and cached: the output directory is `<workspace>/target/hookah-mutants`, so it is
  ignored by version control and reused between runs. `--iterate` skips mutants an earlier run
  caught or found unviable and re-tests the missed ones. `--in-diff` is not used: mutation runs
  because tests changed, so whole files are the useful scope. `hookah` creates the workspace
  `target/` directory if it is absent, since `cargo-mutants` does not.
- Output: reads `mutants.out/outcomes.json`. A missed mutant is an `Error`,
  `mutant survived: <description>`, at the mutated line. A timed-out mutant is a `Warning`
  (`mutant timed out: ...`): it is usually an endless loop, which is a kind of catch. A failing
  baseline (exit code 4), a whole-run timeout, or any other tool failure degrades the run.

## Java (Maven and Gradle)

Implemented. Files with the extension `java` are checked. The project root is the directory of the
nearest `pom.xml`, `build.gradle`, `build.gradle.kts`, `settings.gradle`, or `settings.gradle.kts`:
a module of a multi-module build, or the whole of a single-module one. `hookah` reads the build
files as data (XML for Maven, plain text for the Groovy and Kotlin DSLs of Gradle) and never runs
a build to detect anything. Detection costs about as much as starting the process, so it is not
cached (`hookah detect` says `cache: off`). Only Java sources are checked: Kotlin, Groovy, and
Scala files in the same build are not.

**Detection.**

- Build tool: Maven when the module directory has a `pom.xml`, else Gradle.
- Where commands run: at the root of the build, addressing the module from there. Maven: the
  topmost `pom.xml` that lists the module in its `<modules>` (through intermediate aggregators, and
  with the `<module>dir/pom.xml</module>` spelling too), with `-pl <module>`; a module whose parent
  does not aggregate it is its own root. Gradle: the nearest directory with a `settings.gradle(.kts)`,
  with tasks addressed as `:<project>:<task>` (project path = directory relative to the root;
  `projectDir` overrides are not followed). The search stops at the first directory that holds a
  `.git`.
- Program: the project's wrapper (`mvnw`, `gradlew`; `.cmd` and `.bat` on Windows) at the root or in
  the module when it is executable, else `mvn` or `gradle` from `PATH`. A missing program or JDK
  makes the tool fail to start or exit without a result, which is a degraded run (never a block).
  The wrapper downloads its distribution on first use; that is the wrapper's business, not
  `hookah`'s.
- Offline: every command runs with `mvn -o` or `gradle --offline`. `hookah` never downloads
  anything, so a project whose dependencies and plugins are not in `~/.m2` or the Gradle cache yet
  is reported as degraded, not as a failure. Run one online build first (`mvn verify`,
  `gradle build`), or prefetch with `mvn dependency:go-offline`. When the build stops because an
  artifact is not in the local repository (`... in offline mode and the artifact g:a:jar:1 has not
  been downloaded from it before` for Maven, `No cached version of g:a:1 available for offline
  mode` for Gradle), the notice is one line naming the first missing artifact and the fix (run the
  build once online, for example `mvn -q install -DskipTests`, or set the environment of the build
  in the `[env]` table, see `configuration.md`), not the build's own error text.
- Tools: only what the build files configure. A Maven plugin counts when it is in
  `<build><plugins>` or `<reporting><plugins>` of the module's pom, of an aggregator above it, or of
  a parent reached through `<relativePath>` (a parent that exists only in a remote repository is not
  read). A Gradle plugin counts when its id is quoted in the module's script or the root script (so
  `subprojects {}` configuration is seen), or, for the built-in `checkstyle` and `pmd`, when it is
  named bare in a `plugins {}` block or configured by a `checkstyle {` block. Convention plugins in
  `buildSrc` or an included build are not read.

| Check     | Tool         | Used when                          | Timeout |
|-----------|--------------|------------------------------------|---------|
| format    | `spotless`   | the Spotless plugin                | 60 s    |
| lint      | `checkstyle` | the Checkstyle plugin              | 120 s   |
| lint      | `pmd`        | the PMD plugin                     | 120 s   |
| lint      | `spotbugs`   | the SpotBugs plugin                | 180 s   |
| typecheck | `javac`      | always (the build compiles)        | 180 s   |
| test      | `surefire` (Maven), `gradle-test` | always        | 480 s   |
| mutation  | `pitest`     | the PIT plugin                     | 600 s   |

There is no separate architecture tool: ArchUnit rules are ordinary tests, so they run, and fail,
with the tests at stop. google-java-format and palantir-java-format are used through Spotless
(`googleJavaFormat()`, `palantirJavaFormat()`), which is how projects configure them; standalone
formatters and Gradle's `com.palantir.java-format` plugin are not run.

**Maven commands.** All start with `mvn -B -o`. `-pl <m>` appears only in a multi-module build,
where `<m>` is the module path relative to the reactor root.

| Tool         | Command                                                                                              |
|--------------|------------------------------------------------------------------------------------------------------|
| `spotless`   | `-q spotless:apply -pl <m> -DspotlessFiles=.*/<project-relative file>`                               |
| `checkstyle` | `checkstyle:checkstyle`, then `target/checkstyle-result.xml` of the module                           |
| `pmd`        | `pmd:pmd`, then `target/pmd.xml`                                                                     |
| `spotbugs`   | `test-compile spotbugs:spotbugs`, then `target/spotbugsXml.xml`                                      |
| `javac`      | `-q test-compile -pl <m>,<d>... -am`                                                                 |
| `surefire`   | `test -pl <m> -am -Dtest=<classes> -Dsurefire.failIfNoSpecifiedTests=false -DfailIfNoTests=false [-DexcludedGroups=integration,slow,e2e]` |
| `pitest`     | `test-compile org.pitest:pitest-maven:mutationCoverage -pl <m> -am -DtargetClasses=<fqcns> -DtargetTests=<fqcns> -DoutputFormats=XML -DtimestampedReports=false -DwithHistory=true -DhistoryInputFile=<m>/target/pit-history.bin -DhistoryOutputFile=<same> -DfailWhenNoMutations=false` |

In a reactor, `checkstyle` and `pmd` run as `test-compile <goal> -pl <m>,<d>... -am`, and `spotbugs`
and `javac` likewise (a module resolves its siblings from the reactor, which has them only after
they compiled). `-am` makes the modules the selected ones need part of the run, so they resolve
from the reactor and not from `~/.m2`, which `-o` cannot fill. `<d>...` are the dependents of the
module: the modules that need it, so an API change that breaks them is found in the edit phase.
They are named in `-pl` and not asked for with `-amd`, because `-amd` adds the dependents but `-am`
does not follow it: what a dependent needs besides the changed module (another reactor module, a
`test-jar`) would be looked up in `~/.m2`, and the offline build would fail. `hookah` computes the
dependents itself from the poms of the reactor: `<modules>` (nested aggregators included), then
every module whose `<dependencies>` (any type or classifier, so `test-jar` counts), `<parent>`,
`<build><plugins>` (with their dependencies), or `<build><extensions>` name the module or a
dependent already found. Edges match on `groupId:artifactId` without the version;
`${project.groupId}` and `${project.parent.groupId}` are resolved, any other property in a
`groupId` matches every group, and an `artifactId` that is a property never matches.
`<dependencyManagement>`, `<profiles>`, and BOM imports are not read, and a pom that cannot be
parsed has no edges, so a dependency only a profile adds is not followed. This is the same set
Maven's own `-amd` finds, minus those profile and property edges; unlike `-amd` it is followed by
the upstream of the whole set.

The number of dependents is limited by a budget (`HOOKAH_DOWNSTREAM_BUDGET`, default 10; see
`configuration.md`). Over it, only the module and its upstream compile (`-pl <m> -am`), and every
compile check carries the notice `javac: the N modules that depend on this one were not compiled
(over the HOOKAH_DOWNSTREAM_BUDGET limit); run the full reactor build to check them`. `0` compiles
no dependents and says nothing. Their errors are reported at their own paths (`../app/src/...`
relative to the module, shown relative to where the agent stands). The compiler plugin notices that a
dependency changed only when it changed during the same Maven run, so every command that compiles
in a reactor covers the same set of modules (the test and PIT runs, which come after the compile
check, take `-pl <m> -am` only); otherwise a lint run that compiled the module first would hide the
breakage of its dependents from the compile check. `-DexcludedGroups` is passed only for JUnit 5
projects (a `junit-jupiter*` or `junit-bom` dependency): the JUnit 4 provider would read the tag
names as category classes. The failsafe plugin is never invoked. Spotless' file filter is a regular
expression over absolute paths, so on Windows (backslash separators) the formatter may not match
the file and then does nothing.

**Gradle commands.** All start with `gradle --console=plain --offline`, plus `--continue` except
for the formatter and PIT. `:m:` is the project path (empty for the root project).

| Tool          | Tasks                                                                                          |
|---------------|------------------------------------------------------------------------------------------------|
| `spotless`    | `:m:spotlessApply -PspotlessIdeHook=<absolute file>` (Spotless formats just that file)         |
| `checkstyle`  | `:m:checkstyleMain :m:checkstyleTest`, then `build/reports/checkstyle/{main,test}.xml`         |
| `pmd`         | `:m:pmdMain :m:pmdTest`, then `build/reports/pmd/{main,test}.xml`                              |
| `spotbugs`    | `:m:spotbugsMain`, then `build/reports/spotbugs/main.xml` (needs the XML report enabled)       |
| `javac`       | `:m:compileTestJava` and `:d:compileTestJava` for every dependent project `d`                  |
| `gradle-test` | `:m:test --tests=<class>...`                                                                   |
| `pitest`      | `:m:pitest --targetTests=<classes>`, then `build/reports/pitest/mutations.xml`                 |

Dependents are the included projects whose scripts mention `project(":<path>")` of the module or
of a dependent (transitively). Gradle takes neither PIT's `targetClasses`, nor its output format,
nor tag exclusion on the command line, so those come from the project. The `pitest {}` block must
set `outputFormats = ["XML"]` (otherwise the tool is reported as missing, with that hint). It
decides the classes to mutate through `targetClasses`; the run is narrowed by tests only, so a
survivor in another class that a selected test covers is reported too. It is incremental only with
`enableDefaultIncrementalAnalysis = true` in that block. SpotBugs needs `reports { xml.required =
true }` likewise.

**Typecheck.** The build compiles the module's main and test sources and the dependents, so a
broken test is caught at edit time too. Findings come from javac's own diagnostics: Maven's
`path:[line,col] message` and Gradle's `path:line: error: message` (with the column taken from the
caret line); the `symbol:`, `location:`, `required:`, `found:`, and `reason:` lines are folded into
the message. Duplicates (Maven prints each error twice) are merged. A build that fails without a
diagnostic (an unresolved dependency offline, a missing JDK) degrades the run, quoting the build
tool's own error line.

**Lint.** Checkstyle, PMD, and SpotBugs run per module and write XML reports in the build
directory; `hookah` reads those, and trusts a report only when the console shows the tool ran (an
older report survives a build that fails before the tool starts). The findings are everything the
tool reports for the module, so the use case's usual filter decides what shows (the session
baseline, else errors anywhere and warnings on changed lines). A build that stopped on a compile
error is not a lint failure: the compile check reports it. Severities: Checkstyle `error` is an
error, `warning` and `info` are warnings, `ignore` is dropped; PMD priorities 1 and 2 are errors,
the rest warnings; SpotBugs priority 1 is an error, the rest warnings. The rule is the check or bug
pattern name. A file that Spotless cannot parse is a syntax error that the compile check reports
with its own location, so the formatter adds nothing for it.

**Light tests.** Tests run at stop only, and only light unit tests that relate to the change. A
changed test selects itself. A changed `src/main/java/a/b/Foo.java` selects the files of
`src/test/java/a/b/` named `FooTest`, `FooTests`, `TestFoo`, and `FooTestCase` that exist.
Nothing related selects nothing: the whole suite never runs. The number of classes and the size
of a mutation run are limited by the shared budgets, not by Java rules (see the light-by-default
section of architecture.md: 25 test targets, and 3 files or 150 changed lines for mutation by default; Maven
gets one target per class, joined into one `-Dtest=` option). Heaviness starts from the shared
name rules (`TestWeight`: `integration`, `e2e`, and similar directories and file-name words) and
adds the Java ones. These never run, however they are named or tagged:

- classes named `*IT`, `*ITCase`, `*IntegrationTest(s)`, `*E2E`, `*E2ETest`;
- everything under `src/it/` and `src/integrationTest/` (only `src/test/` is selected from);
- classes whose source carries `@Tag("integration")`, `@Tag("slow")`, `@Tag("e2e")`,
  `@SpringBootTest`, or `@Testcontainers`;
- anything the failsafe plugin would run: the plugin is never invoked.

For Maven and JUnit 5 projects `-DexcludedGroups=integration,slow,e2e` also covers tags on single
methods; Gradle's `excludeTags` is a build-script setting, so a tag on a method of a selected class
is not excluded there. A changed heavy test does not start a mutation run. Test files are the ones
under `src/test`, `src/it`, and `src/integrationTest`, and any named `*Test`, `*Tests`, or `*IT`;
everything else is source.

Failed tests are error findings at the test source, on the line of the first stack frame in the
test class: `CartTest.totalsItems failed: expected: <1001> but was: <1000>` (`errored: <exception
type>: <message>` for exceptions). Surefire reports are read only for the classes the console says
ran (it never deletes old ones); Gradle's results directory belongs to the run. If the tests do not
compile, the compiler errors are the findings (`compile error: ...`); a Gradle selection that
matches nothing is not a failure.

**Mutation (PIT).** Available when the PIT plugin is configured. With tests but without it, a
non-blocking notice names the coordinates: `org.pitest:pitest-maven`, plus
`org.pitest:pitest-junit5-plugin` for JUnit 5, or the `info.solidsoft.pitest` plugin for Gradle.
It runs at stop, after green tests, and only when a light test changed (the shared mutation
rules). The sources to mutate are the changed sources plus those the changed tests cover by name
(`FooTest` covers `Foo`); the tests are the light tests related to them. For Maven
these become `-DtargetClasses=a.Foo,a.Foo$*,...` and `-DtargetTests=a.FooTest,...` (the `$*` keeps
nested classes in), so only the changed code and its own tests run. History is kept at
`<module>/target/pit-history.bin` (`withHistory`), which makes the next run incremental: unchanged
code with unchanged tests is not mutated again. A `SURVIVED` mutant is an error,
`mutant survived: <what PIT changed>`, at the source line, with the mutator as the rule.
`NO_COVERAGE` mutants are not reported: the selected tests are a deliberate subset, so what they do
not reach is expected to be uncovered (the Node policy). "No mutations found" is an empty result.
PIT needs green tests; a failing run degrades.

**Caveats.**

- JVM start dominates. A Maven call takes 1 to 3 s warm and a Gradle call about the same with the
  daemon, so the edit phase (format, lint, compile) takes several seconds when all tools are
  configured. Gradle's daemon is on by default and survives between hook calls, which is the
  biggest speed-up; do not disable it (`org.gradle.daemon=false`). For Maven, a resident daemon such
  as [mvnd](https://github.com/apache/maven-mvnd) helps the same way. `hookah` calls `mvn` or
  `mvnw` and does not choose `mvnd` itself; to use it, put a `mvn` executable that forwards to
  `mvnd` first on the `PATH` of the hook.
- Timeouts are generous (see the table) because of JVM start and first-run compilation; a timeout
  is a degraded run, never a block.
- Multi-module reactors cost more: `-pl <m>,<d>... -am` compiles the neighbors (incrementally; at
  most `HOOKAH_DOWNSTREAM_BUDGET` dependents). The test and
  PIT runs cover the module and what it needs, not the dependents' tests.
- Source layouts other than `src/<set>/java/<package>/<Class>.java` are not mapped to tests.
