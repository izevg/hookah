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
