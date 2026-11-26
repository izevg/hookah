# Antigravity CLI (agy)

Install [Antigravity CLI](https://antigravity.google/docs/cli-features) and put `hookah` on
`PATH`. From the project root, run:

```sh
hookah setup antigravity
hookah setup --detect
```

Restart `agy` and use `/hooks` to inspect the registrations. Setup writes
`.agents/hooks.json` by default. Add `--scope user` to use
`~/.gemini/config/hooks.json`, the global hook file shared by Antigravity surfaces.
Choose one scope per workspace.

The [Antigravity hook contract](https://antigravity.google/docs/hooks) has four events
that hookah uses:

- `SessionStart` records the baseline.
- `PostToolUse` runs formatting and lint after successful file edits, including the CLI
  tools `write_file` and `edit_file`.
- `PreInvocation` gives the model any pending edit feedback, including which files it
  needs to re-read after formatting. Each batch is delivered once.
- `Stop` checks the changed files when the model finishes and background tasks are idle.
  Findings can request one continuation for the current prompt. A clean stop resets that
  limit. Shell edits are covered by the stop phase.

To print the manual JSON configuration, run:

```sh
hookah setup antigravity --print
```

Copy the `hookah` group into the hook file. To remove the managed entries, run
`hookah uninstall antigravity` with the same scope you chose at setup.
