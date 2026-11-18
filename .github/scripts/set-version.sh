#!/usr/bin/env bash
# Sets the release version for this build only, without committing anything: rewrites `version`
# under [workspace.package] in Cargo.toml and the workspace crates' entries in Cargo.lock, so
# `cargo build --locked` still passes and `hookah --version` prints the release version.
# The repository keeps 0.0.0; the version of a release lives in its `vX.Y.Z` tag.
# Usage: .github/scripts/set-version.sh X.Y.Z   (run from anywhere inside the repository)
set -eu

version="${1:?usage: set-version.sh X.Y.Z}"
case "$version" in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) echo "set-version: '$version' is not a semantic version" >&2; exit 2 ;;
esac

cd "$(git rev-parse --show-toplevel)"

# The workspace crates in Cargo.lock are the ones without a `source` line: they are the path
# members, every registry package has `source = "registry+..."`.
awk -v v="$version" '
  /^\[/ { p = ($0 == "[workspace.package]") }
  p && !done && /^version[[:space:]]*=/ { print "version = \"" v "\""; done = 1; next }
  { print }
  END { if (!done) exit 3 }
' Cargo.toml > Cargo.toml.new || { rm -f Cargo.toml.new; echo "set-version: no version under [workspace.package]" >&2; exit 1; }
mv Cargo.toml.new Cargo.toml

awk -v v="$version" '
  function flush(   i) { for (i = 1; i <= n; i++) print buf[i]; n = 0 }
  /^\[\[package\]\]/ { flush(); inpkg = 1; buf[++n] = $0; next }
  inpkg { buf[++n] = $0
          if ($0 ~ /^source[[:space:]]*=/) hasSource = 1
          if ($0 == "") { if (!hasSource) { for (i = 1; i <= n; i++) if (buf[i] ~ /^version = /) buf[i] = "version = \"" v "\"" } flush(); inpkg = 0; hasSource = 0 }
          next }
  { print }
  END { if (n) { if (!hasSource) for (i = 1; i <= n; i++) if (buf[i] ~ /^version = /) buf[i] = "version = \"" v "\""; flush() } }
' Cargo.lock > Cargo.lock.new
mv Cargo.lock.new Cargo.lock
