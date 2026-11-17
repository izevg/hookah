# Ecosystems

> Stub. Expands as each ecosystem adapter is written.

For each ecosystem: how its manifests are recognized, which tools are detected and in what order
of preference, how output is parsed, and how related tests are selected.

## Node (TypeScript / JavaScript)

Planned.

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
