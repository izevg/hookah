# opencode

## Mechanism

opencode has no command hooks. It loads JavaScript/TypeScript plugins, so `hookah` ships one:
a small, dependency-free TypeScript plugin that runs inside opencode and calls the `hookah`
binary. The plugin source is embedded in the binary and printed by `hookah plugin opencode`.

- **Session start.** The plugin's `event` hook reacts to `session.created` (top-level sessions
  only; subagent sessions get theirs lazily) and runs `hookah hook opencode` with
  `event: "start"` and the session id. `hookah` records the problems the project already has in
  the background, so later reports show only what the agent introduced (the baseline; see
  [architecture](../../architecture.md#baseline)), and answers at once with an empty reply.
- **Edit phase.** The plugin's `tool.execute.after` hook fires after the built-in tools that
  write files: `write` and `edit` (argument `filePath`) and `apply_patch` (argument
  `patchText`; the plugin reads the `*** Add File:`, `*** Update File:`, and `*** Move to:`
  headers, and skips deleted files). The `multiedit` and `patch` tool names of older opencode
  releases are handled too. For each call, the plugin runs `hookah hook opencode`, with a 30 s
  timeout, and appends hookah's report to the tool output (`output.output`). That is the tool
  result the model reads next. If `tool.execute.after` carries no `args`, the plugin falls back
  to the ones it saw in `tool.execute.before` for the same `callID`.
- **Stop phase.** opencode has no stop hook. The plugin listens to the `event` hook and reacts
  to `session.idle`, which fires when a turn ends. If the session edited files since the last
  check (or the last turn was started by a hookah message), it runs the stop phase (checks
  over everything changed in the session, including related tests; 120 s timeout). When that
  fails, the plugin injects the report once as a new user message with
  `client.session.promptAsync`, and the agent keeps working. `promptAsync` returns at once, so
  the event handler never waits for the agent's reply.
- **User-only warnings.** Missing tools, degraded checks, and "still failing" summaries never
  reach the model. The plugin shows them as a toast (`client.tui.showToast`, variant `warning`)
  and writes the same text to opencode's log (`client.app.log`, service `hookah`, level `warn`).
  A toast needs a running TUI; the log line is always written.

The plugin and the binary exchange one JSON document each way, versioned with `"v": 1`:

```json
{"v": 1, "event": "edit", "cwd": "/abs/project", "files": ["src/a.ts"],
 "session_id": "ses_...", "already_continuing": false}
```

```json
{"v": 1, "block": true, "model_text": "hookah: 1 problem ...", "user_text": "..."}
```

`event` is `start`, `edit`, or `stop` (`start` was added to contract v1 without a version bump: an
older binary rejects it as a malformed payload and the plugin only warns); `files` is only used by `edit` (relative paths resolve against
`cwd`). In the reply, `block` means the model has to act on `model_text`. `model_text` without
`block` is advice (re-read a file the formatter rewrote). `user_text` is for the human only. A
plugin and a binary with different contract versions refuse to talk to each other, and the
plugin warns the user to reinstall it.

## Setup

Run this in the project:

```sh
hookah setup opencode                # .opencode/plugins/hookah.ts in this project
hookah setup opencode --scope user   # ~/.config/opencode/plugins/hookah.ts (or under $XDG_CONFIG_HOME)
```

It writes exactly the text of `hookah plugin opencode`. The plugin embeds no command: it runs
`hookah` from `PATH`, or from `HOOKAH_BIN` (see below), so `--command` has no effect here. A file
that already exists is backed up once to `hookah.ts.hookah.bak` and then replaced; run the command
again after upgrading `hookah` to get the matching plugin. `--dry-run` and `--print` work as for
the other harnesses. Then restart opencode.

### Manual setup

1. Install `hookah` somewhere on `PATH` (for example `$HOME/.local/bin/hookah`).
2. Install the plugin, per project or globally:

   ```sh
   mkdir -p .opencode/plugins
   hookah plugin opencode > .opencode/plugins/hookah.ts          # this project
   # or: ~/.config/opencode/plugins/hookah.ts                    # every project
   ```

   opencode loads every `.ts` and `.js` file in these directories at startup; no entry in
   `opencode.json` is needed. Re-run the command after upgrading `hookah` to get the matching
   plugin.
3. Restart opencode. To confirm it loaded, run `opencode run --print-logs "hi"` in the project:
   a missing `hookah` shows up as a `service=hookah` warning on the first file edit, and
   a working install stays silent until there is something to say.

The plugin reads one environment variable: `HOOKAH_BIN`, the binary to run (default: `hookah`
from `PATH`).

### Toolchain on `PATH`

`hookah` runs the project's own tools (`cargo`, `npx`, `ruff`, ...), and it inherits the
environment of opencode, which inherits the environment of the shell that started it. Two ways
to make the right tools available:

- Start opencode through mise: `mise exec -- opencode`, or activate mise in the shell.
- Point `HOOKAH_BIN` at a wrapper script, for example `$HOME/.local/bin/hookah-mise`:

  ```sh
  #!/bin/sh
  exec "$HOME/.local/bin/mise" exec -- "$HOME/.local/bin/hookah" "$@"
  ```

  and start opencode with `HOOKAH_BIN=$HOME/.local/bin/hookah-mise`.

## Response policy

| Situation | What happens |
| --- | --- |
| Edit phase finds problems | Report appended to the tool output the model reads (`block: true`) |
| Formatter rewrote the file | Re-read hint appended to the tool output |
| Tool missing for a check | Advice for the model appended to the tool output, and a toast and log line for the user |
| Stop phase finds problems, first time | Report injected as a user message; the agent keeps working |
| Stop phase finds problems, right after a hookah injection | Not injected again; toast and log line for the user |
| Checks degraded (timeout, tool crashed) | Toast and log line only; never blocks |
| `hookah` missing, crashed, timed out, or answered garbage | Toast and log line only; the tool call is untouched (fail open) |

## Loop guard

The model reacts to the injected message, edits, and the session goes idle again. To avoid an
endless fix loop, the plugin keeps a per-session flag, "the current turn was started by a hookah
injection", and sends it to `hookah` as `already_continuing`:

1. Idle after edits: stop checks fail, no flag: inject the report, set the flag.
2. Idle after the injected turn: checks run again with `already_continuing: true`. If they still
   fail, `hookah` answers `block: false` with a summary for the user; the plugin only warns, and
   clears the flag. If they pass, the flag is cleared silently.
3. The next turn the user starts begins with a clean flag, so a new round of edits can be blocked
   once again.

This mirrors the `stop_hook_active` policy of the Claude Code adapter: block once, then only
warn the user. The decision lives in `hookah` (`crates/harness/src/opencode/response.rs`); the
plugin only keeps the flag. A session that did not edit any file since its last check is not
checked on idle, so plain chat turns cost nothing.

## Limitations

- The tool already ran; a report is advice, not a rollback.
- Only `write`, `edit`, `apply_patch` (and the legacy `multiedit` and `patch`) are checked
  after the edit. Files changed through `bash` are picked up by the stop phase only.
- Injected stop feedback appears in the transcript as a user message, because opencode has no
  way to continue a turn other than adding one. The plugin passes no agent or model, so opencode picks its defaults for the session.
- The stop phase runs on `session.idle`, which fires after every turn that follows edits, so a
  long session can run it several times, once per turn.
- `session.idle` also fires in subagent sessions; each is checked on its own.
- Toasts show only in the TUI. In `opencode run` and `opencode serve` the warning is in the log
  (`--print-logs`).
- `opencode run` disposes the session as soon as it is idle, so a one-shot `opencode run` ends
  before the stop-phase injection can start another turn. Use the TUI or `opencode serve` for the
  stop phase.
- The plugin does not type-check inside opencode; it is compiled by Bun at load time. It is
  written against `@opencode-ai/plugin` 1.19.6.

## Verified against

<https://opencode.ai/docs/plugins> and <https://opencode.ai/v1/docs/plugins/> (the V1 page
documents the hook-based plugin API this plugin uses; the current "latest" page covers the
newer config-driven plugin loading), read on 2026-11-17, plus the `@opencode-ai/plugin` and
`@opencode-ai/sdk` type definitions and the `opencode-ai` binary, all version 1.19.6.

Checked live, with that opencode version against a local mock model server (no API key):

- the plugin loads from `.opencode/plugins/hookah.ts`; `.opencode/plugin/` (singular) also loads
  in this version, but the docs name the plural directory;
- the edit report appended to `output.output` is what the model receives in the tool result;
- `tool.execute.after` carries `args` for `write` (`filePath`, `content`);
- a failing stop phase injects one user message with `promptAsync`, and after the injected turn
  a second failure only warns (no further model request);
- a missing `hookah` binary logs a warning and leaves the tool call untouched.

Not checked live: the `session.created` event (`event.properties.info.id` and `parentID`, from the
SDK types), `apply_patch` and `multiedit` (the tool list of the test model had `write`
and `edit` only; the `apply_patch` argument name `patchText` comes from the opencode binary),
and the toast (no TUI was running).
