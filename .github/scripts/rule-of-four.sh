#!/usr/bin/env bash
# Checks "the rule of four" from AGENTS.md over the tracked files:
#   - at most 4 files and at most 8 subdirectories in any directory;
#   - dotfiles and dot-directories are not counted;
#   - the repository root holds only the files the ecosystem requires there.
# Usage: .github/scripts/rule-of-four.sh   (run from anywhere inside the repository)
set -eu

cd "$(git rev-parse --show-toplevel)"

git ls-files | awk '
BEGIN {
  split("LICENSE README.md AGENTS.md CLAUDE.md Cargo.toml Cargo.lock", allowed, " ")
  for (i in allowed) root_ok[allowed[i]] = 1
}
{
  n = split($0, part, "/")
  hidden = 0
  for (i = 1; i <= n; i++) if (substr(part[i], 1, 1) == ".") hidden = 1
  if (hidden || n == 0) next
  dir = "."
  for (i = 1; i < n; i++) {
    child = (dir == "." ? part[i] : dir "/" part[i])
    if (!(child in seen)) {
      seen[child] = 1
      dirs[dir]++
    }
    dir = child
  }
  if (dir == ".") {
    if (!($0 in root_ok)) {
      printf "root: %s is not an allowed root file\n", $0
      bad = 1
    }
  } else {
    files[dir]++
  }
}
END {
  for (d in files) if (files[d] > 4) { printf "%s: %d files (max 4)\n", d, files[d]; bad = 1 }
  for (d in dirs) if (dirs[d] > 8) { printf "%s: %d subdirectories (max 8)\n", d, dirs[d]; bad = 1 }
  exit bad
}
' && echo "rule of four: ok"
