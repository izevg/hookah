#!/bin/sh
# Creates .venv from uv.lock (needs network on the first run) and warms up pytest-testmon,
# so scenario runs are incremental instead of running the whole suite.
set -eu
export PYTHONHASHSEED=0
uv sync --frozen
.venv/bin/pytest --testmon -q --color=no
