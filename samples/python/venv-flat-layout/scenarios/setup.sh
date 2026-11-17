#!/bin/sh
# Creates a plain .venv with uv and installs the pinned requirements-dev.txt (needs network on the
# first run). mutmut is deliberately not installed.
set -eu
export PYTHONHASHSEED=0
uv venv --python 3.13 .venv
uv pip install --python .venv/bin/python --quiet -r requirements-dev.txt
