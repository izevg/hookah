#!/usr/bin/env bash
# Builds the documentation site into target/site/book (the directory GitHub Pages publishes).
# One command, from anywhere in the repository:  .github/scripts/build-site.sh
# Needs mdbook (pinned in .mise.toml) and python3. Add `--serve` to preview it on localhost:8000.
set -eu

cd "$(git rev-parse --show-toplevel)"
site=.github/scripts/site.py

python3 -I "$site" assemble
mdbook build target/site
python3 -I "$site" finish
python3 -I "$site" check

echo "site built: target/site/book"
if [ "${1:-}" = "--serve" ]; then
  cd target/site/book
  exec python3 -m http.server 8000
fi
