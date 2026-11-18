# Mutation testing of hookah itself

This repository is checked with [cargo-mutants](https://mutants.rs). The version is pinned in
`.mise.toml` (`mise install` puts it under `~/.local`; there is no prebuilt arm64 macOS release,
so mise compiles it once). Settings live in `.cargo/mutants.toml`, and `[profile.mutants]` in
`Cargo.toml` builds mutants without debug info.

## Where the cache lives

`target/hookah-mutants/mutants.out/` (git-ignored with `target/`). It is the same directory
`hookah` uses when it runs mutation testing on a Rust project, so a manual run and a hook run
share one `--iterate` cache. `mutants.out/outcomes.json` is the machine-readable result;
`missed.txt`, `caught.txt`, `timeout.txt`, and `unviable.txt` list each class.

## Full baseline

Run it once, with about half the cores:

```sh
mise exec -- cargo mutants --workspace -j 5
```

It takes a while. Every mutant is built and tested in a copy of the tree.

## Iterating

`--iterate` skips mutants that earlier runs caught or found unviable and re-tests only the
missed and new ones. After adding tests for the survivors:

```sh
mise exec -- cargo mutants --workspace -j 5 --iterate
```

Edited code gets new mutants (their names include line and column), so a moved function is
re-tested once and then cached again.

For a pull-request-sized run, mutate only what the branch changed:

```sh
git diff main... > target/pr.diff
mise exec -- cargo mutants --workspace -j 5 --in-diff target/pr.diff
```

Combine `--in-diff` with `--iterate` to also skip mutants a previous run already settled.

## What `hookah` does on a project

This page is about mutation-testing `hookah` itself. When `hookah` runs mutation testing on your
project, it does so only for small changes, so it helps and does not block: at stop, a mutation
run needs a changed test, at most 3 changed code files, and at most 150 changed lines in them. A
larger change is skipped with the notice `mutation testing skipped: change too large (X files / Y
lines, budget 3 / 150)`. Set `HOOKAH_MUTATION_MAX_FILES` and `HOOKAH_MUTATION_MAX_LINES` to change
the limits (`0` switches mutation off). A run that times out is a degraded, non-blocking result.
See "Light by default" in [architecture.md](architecture.md).
