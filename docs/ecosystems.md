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
