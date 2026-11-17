//! Python toolchain detection.
//!
//! Reads `pyproject.toml` (and legacy `setup.cfg`, `tox.ini`) to find the formatter and linter
//! (`ruff format`, `ruff check`) and the test runner (`pytest`). Runs tools from the project's
//! virtual environment.
