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
need none. (It counts toward the four files of `scenarios/`.)

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
| `rust/workspace` | `01-core-api-break` | edit | a `core` API rename that breaks `app`: the edit phase only checks `core`, so it stays silent (known gap) |
| | `02-stop-core-change` | stop | a `core` change: stop runs the tests of `core` and of `app` |
| | `03-core-api-break-stop` | stop | the same API break, caught at stop in `app` |

`rust/crate/07-test-edit-mutation` records the answer for a machine without `cargo-mutants`, so
its `requires` is `! cargo mutants --version`. `08-test-edit-mutation-survivor` is its sibling for
a machine that has the tool (`requires`: `cargo mutants --version`). Each machine runs exactly one
of the two.
