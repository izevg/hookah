# Pi

## Mechanism

[Pi](https://pi.dev) (`pi`, npm `@mariozechner/pi-coding-agent`) has no command hooks. It loads
TypeScript extensions, so `hookah` ships one: a small, dependency-free extension that runs inside
Pi and calls the `hookah` binary. The source is embedded in the binary and printed by
`hookah plugin pi`. Pi loads extensions with [jiti](https://github.com/unjs/jiti), so the file
needs no build step.

- **Session start.** The extension's `session_start` handler (reasons `startup`, `new`, `resume`,
  and `fork`; a `reload` is skipped) runs `hookah hook pi` with `event: "start"` and the session
  id. `hookah` records the problems the project already has in the background, so later reports
  show only what the agent introduced (the baseline; see
  [architecture](../../architecture.md#baseline)), and answers at once with an empty reply.
- **Edit phase.** The `tool_result` handler fires after every tool. For the built-in tools that
  write files, `write` and `edit` (argument `path` in `event.input`), it runs `hookah hook pi`,
  with a 30 s timeout, and returns the tool's content with hookah's report appended as one more
  text block. That is the tool result the model reads next. Results with `isError` are skipped
  (nothing was written). Pi has no other built-in tool that writes files; files changed through
  `bash` are picked up by the stop phase only.
- **Stop phase.** The `agent_end` handler fires when a run ends, once per user prompt. If the
  session edited files since the last check (or the last run was started by a hookah message), it
  runs the stop phase (checks over everything changed in the session, including related tests;
  120 s timeout). When that fails, the extension injects the report once as a new message with
  `pi.sendMessage({customType: "hookah", display: true}, {triggerTurn: true})`, and the agent keeps
  working. `agent_end` fires while Pi still counts the agent as busy, and a message queued then
  (`deliverAs: "steer"` or `"followUp"`) would only wait for the user's next prompt, so the
  extension waits until `ctx.isIdle()` and then triggers a turn. A run that was aborted or ended
  in an error is not checked; the edits stay pending for the next run.
- **User-only warnings.** Missing tools, degraded checks, and "still failing" summaries never
  reach the model. The extension shows them with `ctx.ui.notify(text, "warning")`. Without a UI
  (print and JSON mode) it writes them to stderr instead.

The extension and the binary exchange one JSON document each way, versioned with `"v": 1`. It is
the same contract shape as the [opencode plugin](opencode.md):

```json
{"v": 1, "event": "edit", "cwd": "/abs/project", "files": ["src/a.ts"],
 "session_id": "...", "model": "anthropic/claude-sonnet-4-6", "already_continuing": false}
```

```json
{"v": 1, "block": true, "model_text": "hookah: 1 problem ...", "user_text": "..."}
```

`event` is `start`, `edit`, or `stop`; `files` is only used by `edit` (relative paths resolve
against `cwd`). In the reply, `block` means the model has to act on `model_text`. `model_text`
without `block` is advice (re-read a file the formatter rewrote). `user_text` is for the human
only. An extension and a binary with different contract versions refuse to talk to each other, and
the extension warns the user to reinstall it.

## Setup

Run this in the project:

```sh
hookah setup pi                # .pi/extensions/hookah.ts in this project
hookah setup pi --scope user   # ~/.pi/agent/extensions/hookah.ts (or under $PI_CODING_AGENT_DIR)
```

It writes exactly the text of `hookah plugin pi`. The extension embeds no command: it runs
`hookah` from `PATH`, or from `HOOKAH_BIN` (see below), so `--command` has no effect here. A file
that already exists is backed up once to `hookah.ts.hookah.bak` and then replaced; run the command
again after upgrading `hookah` to get the matching extension. `--dry-run` and `--print` work as
for the other harnesses. Then restart Pi, or run `/reload` in it.

### Manual setup

1. Install `hookah` somewhere on `PATH` (for example `$HOME/.local/bin/hookah`).
2. Install the extension, per project or globally:

   ```sh
   mkdir -p .pi/extensions
   hookah plugin pi > .pi/extensions/hookah.ts        # this project
   # or: ~/.pi/agent/extensions/hookah.ts             # every project
   ```

   Pi auto-discovers `*.ts` files in these directories (and `*/index.ts` in their
   subdirectories); no entry in `settings.json` is needed. For a quick trial without installing,
   run `pi -e ./hookah.ts`.
3. Restart Pi, or run `/reload`. A working install stays silent until there is something to say;
   a missing `hookah` shows up as a notification on the first session start or edit.

The extension reads one environment variable: `HOOKAH_BIN`, the binary to run (default: `hookah`
from `PATH`).

### Toolchain on `PATH`

`hookah` runs the project's own tools (`cargo`, `npx`, `ruff`, ...), and it inherits the
environment of Pi, which inherits the environment of the shell that started it. Two ways to make
the right tools available:

- Start Pi through mise: `mise exec -- pi`, or activate mise in the shell.
- Point `HOOKAH_BIN` at a wrapper script, for example `$HOME/.local/bin/hookah-mise`:

  ```sh
  #!/bin/sh
  exec "$HOME/.local/bin/mise" exec -- "$HOME/.local/bin/hookah" "$@"
  ```

  and start Pi with `HOOKAH_BIN=$HOME/.local/bin/hookah-mise`.

## Response policy

| Situation | What happens |
| --- | --- |
| Edit phase finds problems | Report appended to the tool result the model reads (`block: true`) |
| Formatter rewrote the file | Re-read hint appended to the tool result |
| Tool missing for a check | Advice for the model appended to the tool result, and a notification for the user |
| Stop phase finds problems, first time | Report injected as a message that starts a new run; the agent keeps working |
| Stop phase finds problems, right after a hookah injection | Not injected again; notification for the user |
| Checks degraded (timeout, tool crashed) | Notification only; never blocks |
| `hookah` missing, crashed, timed out, or answered garbage | Notification only; the tool result is untouched (fail open) |

## Loop guard

The model reacts to the injected message, edits, and the run ends again. To avoid an endless fix
loop, the extension keeps a flag, "the run that just ended was started by a hookah injection", and
sends it to `hookah` as `already_continuing`:

1. Run ends after edits, stop checks fail, no flag: inject the report, set the flag.
2. Run ends after the injected run: checks run again with `already_continuing: true`. If they
   still fail, `hookah` answers `block: false` with a summary for the user; the extension only
   notifies, and clears the flag. If they pass, the flag is cleared silently.
3. The next prompt the user sends begins with a clean flag, so a new round of edits can be
   blocked once again.

This mirrors the `stop_hook_active` policy of the Claude Code adapter: block once, then only warn
the user. The decision lives in `hookah` (`crates/harness/src/plugins/pi/response.rs`); the
extension only keeps the flag. A run that did not edit any file since the last check is not
checked, so plain chat turns cost nothing.

## Limitations

- The tool already ran; a report is advice, not a rollback.
- Only `write` and `edit` are checked after the edit. Files changed through `bash` are picked up
  by the stop phase only. Custom tools that write files are not recognized.
- Pi's print mode (`pi -p`) and JSON mode end the process once the run is over, so a stop-phase
  injection cannot start another run. There the extension prints the stop report on stderr
  instead (best effort: Pi does not wait for `agent_end` handlers before it exits, so a slow stop
  check can be cut off). The agent continues on the report only in the interactive TUI and in RPC
  mode.
- Injected stop feedback appears in the transcript as a custom `hookah` message that Pi sends to
  the model as a user turn. The extension passes no model, so Pi keeps the current one.
- The stop phase runs on `agent_end`, once per user prompt that follows edits, so a long session
  can run it several times.
- The extension state (the loop flag) lives in the extension instance, so it starts clean on
  `/new`, `/resume`, `/fork`, and `/reload`.
- Pi extensions run with full system permissions; the extension only starts the `hookah` binary
  and sends it file paths and the session id.
- The extension is type-checked against `@mariozechner/pi-coding-agent` 0.73.1 and run on that
  version. The events it uses (`session_start` with `reason`, `tool_result` with result patches,
  `agent_end`, `pi.sendMessage` with `triggerTurn`, `ctx.isIdle()`) appear in the changelog of
  0.65.0 (`session_start` with `reason`) and earlier, so 0.65.0 is the expected minimum; older
  versions were not tried. Pi 0.73.1 has no `agent_settled` event and no `ctx.actions`:
  `agent_end` and `pi.sendMessage` are the current API.

## Related

[`@hsingjui/pi-hooks`](https://www.npmjs.com/package/@hsingjui/pi-hooks) (0.0.2) is a community
package that runs Claude Code-style command hooks inside Pi. `hookah` does not depend on it: the
bundled extension talks to the binary directly, and works without any other Pi package.

## Verified against

<https://pi.dev/docs/latest/extensions> (the same text as `docs/extensions.md` in the npm
package), plus the `docs/` folder, `CHANGELOG.md`, and the `dist/core/extensions/types.d.ts` type
definitions and `dist/` code of `@mariozechner/pi-coding-agent` 0.73.1, the `latest` dist-tag on
the npm registry. Read on 2026-11-18.

Checked live, with that Pi version against a local mock OpenAI-compatible model server (no API
key), in RPC mode (`pi --mode rpc`) and in print mode (`pi -p`):

- the extension loads from `.pi/extensions/hookah.ts` through auto-discovery;
- `session_start` runs `hookah hook pi` with `event: "start"` and the session id;
- the edit report appended in `tool_result` is what the model receives in the tool result
  (`write` of a Rust file with a type error: `src/lib.rs:2:5: mismatched types [E0308]`);
- a failing stop phase injects one `hookah` message that starts a new run, and after that run a
  second failure only notifies (no further model request); when the injected run fixes the file,
  the next stop check passes and nothing more is sent;
- a missing `hookah` binary produces a notification and leaves the tool result untouched;
- in print mode the stop report goes to stderr and the process exits normally.

Not checked live: the `edit` tool (the mock model only called `write`; `edit` takes the same
`path` argument per the type definitions), `/reload`, `/new`, and `/resume`, and the TUI
notification itself (RPC mode forwarded it as an `extension_ui_request`, but no terminal UI was
running).
