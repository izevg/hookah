# Claude Code

> Stub. Written when this harness adapter is built, and checked against the harness's current
> docs.

## Mechanism

Command hooks: `PostToolUse` with an `Edit|Write|MultiEdit|NotebookEdit` matcher runs the edit phase, and `Stop` runs the stop phase. Feedback goes through exit code 2 + stderr.

## Setup

Config file location, the snippet to add, and how to confirm the hook fires.

## Limitations

What this harness lets `hookah` do and what it doesn't.
