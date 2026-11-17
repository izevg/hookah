# Ecosystems

> Stub. Expands as each ecosystem adapter is written.

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

Planned.

## Rust

Implemented. The project root is the directory of the nearest `Cargo.toml`; `cargo metadata
--no-deps` gives the package name, edition, and workspace root. A virtual workspace manifest has
no toolchain, and a missing `cargo` degrades the run instead of failing it.

| Check  | Tool         | Command (cwd = project root)                                                      |
|--------|--------------|-----------------------------------------------------------------------------------|
| format | `rustfmt`    | `rustfmt --edition <edition> <file>` (also formats the file's out-of-line modules) |
| lint   | `clippy`     | `cargo clippy -p <pkg> --all-targets --message-format=json --quiet`                |
| test   | `cargo-test` | `cargo test --message-format=json --quiet --package=<pkg> ... --no-fail-fast`      |

- Output: clippy and cargo JSON compiler messages (error and warning, primary span, rule from the
  diagnostic code, duplicates across targets merged, external-crate spans dropped); rustfmt parse
  errors from stderr; failed libtest tests located at their `panicked at` position. A tool that
  fails without any finding degrades the run.
- Paths are relative to the project root, so a sibling crate shows as `../sibling/src/x.rs`.
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
