# Codex CLI

## Mechanism

Command hooks, stable since Codex CLI v0.124. Codex runs `hookah hook codex` and sends one JSON
object on stdin. Hooks live in `hooks.json` or inline under `[hooks]` in `config.toml`.

- `SessionStart` records the problems the project already has, in the background, so later
  reports show only what the agent introduced (the baseline; see
  [architecture](../../architecture.md#baseline)). The hook returns at once and prints nothing.
- `PostToolUse` with matcher `^apply_patch$` runs the edit phase. One patch can touch several
  files; `hookah` reads the `*** Add File:`, `*** Update File:`, and `*** Move to:` headers from
  the patch text (`tool_input.command`), checks each file, and merges the results into one
  response. Deleted files are skipped.
- `Stop` runs the stop phase (checks over the files changed in the session, including related
  tests).

| Situation | Response |
| --- | --- |
| Edit phase finds problems | exit 0, `{"decision":"block","reason":...}`; Codex replaces the tool result with the report |
| Formatter rewrote a file | exit 0, `hookSpecificOutput.additionalContext` asking the model to re-read it |
| Stop phase finds problems | exit 0, `{"decision":"block","reason":...}`; Codex continues with the report as a new prompt |
| Stop phase finds problems and `stop_hook_active` is `true` | exit 0, `systemMessage` for the user; never blocks twice in a row |
| `hookah` cannot finish (timeout, missing tool, internal error, bad payload) | exit 0, warning only (fail open) |

Codex requires JSON on stdout for `Stop` hooks when they print anything; `hookah` only prints JSON
there, and prints nothing when there is nothing to say.

## Setup

Run this in the project:

```sh
hookah setup codex                # .codex/config.toml in this project
hookah setup codex --scope user   # ~/.codex/config.toml, for every project
```

Both write the same configuration as the manual snippet below. Existing settings are kept, a
second run changes nothing, and the original file is backed up once to `<file>.hookah.bak`. A file
that does not parse is refused and left untouched. `--dry-run` shows the result without writing,
`--print` prints only the snippet, and `--command "mise exec -- hookah"` replaces the default command
(the absolute path of the running binary).
Comments and formatting of an existing `config.toml` are kept. Setup writes `config.toml`, not
`hooks.json`.

Then restart Codex and run `/hooks` in it to review and trust the new hooks.

### Windows

`hookah setup codex` writes the path of `hookah.exe` with forward slashes and quotes it when it
contains a space, for example `"C:/Program Files/hookah/hookah.exe" hook codex`. Codex starts hook
commands as `%COMSPEC% /C "<command>"` (`cmd.exe`; read from the Codex source, `command_runner.rs`
in `codex-rs/hooks`), which runs a drive-qualified path written with `/` and keeps the quotes
around a path with spaces. The hooks page also lists `commandWindows`, an optional Windows-only
override of `command`; setup does not need it.

### Manual setup

Put `hookah` at `$HOME/.local/bin/hookah`. Add this to `.codex/config.toml` in the project (or
`~/.codex/config.toml`):

```toml
[[hooks.SessionStart]]
matcher = "startup|resume"

[[hooks.SessionStart.hooks]]
type = "command"
command = '$HOME/.local/bin/hookah hook codex'
timeout = 30

[[hooks.PostToolUse]]
matcher = "^apply_patch$"

[[hooks.PostToolUse.hooks]]
type = "command"
command = '$HOME/.local/bin/hookah hook codex'
timeout = 30

[[hooks.Stop]]

[[hooks.Stop.hooks]]
type = "command"
command = '$HOME/.local/bin/hookah hook codex'
timeout = 120
```

The same thing as `hooks.json`:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "matcher": "startup|resume",
        "hooks": [
          { "type": "command", "command": "$HOME/.local/bin/hookah hook codex", "timeout": 30 }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "^apply_patch$",
        "hooks": [
          { "type": "command", "command": "$HOME/.local/bin/hookah hook codex", "timeout": 30 }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          { "type": "command", "command": "$HOME/.local/bin/hookah hook codex", "timeout": 120 }
        ]
      }
    ]
  }
}
```

Timeouts are in seconds. The project's own toolchain (cargo, node, python, ...) must be on
`PATH` in the hook's environment. A project that pins its tools with mise can use
`$HOME/.local/bin/mise exec -- $HOME/.local/bin/hookah hook codex` as the command.

To check by hand:

```sh
echo '{"cwd":"'"$PWD"'","hook_event_name":"PostToolUse","tool_name":"apply_patch","tool_input":{"command":"*** Begin Patch\n*** Update File: src/main.rs\n*** End Patch"}}' \
  | $HOME/.local/bin/hookah hook codex; echo "exit $?"
```

## Limitations

- Without the `SessionStart` hook there is no up-front baseline: the first edit of a session
  starts it, and that edit is reported with the changed-lines filter only.
- The tool already ran; a block only feeds the report back. It cannot undo the patch.
- Only `apply_patch` is checked. Files changed through shell commands are picked up by the stop
  phase only.
- The docs say `apply_patch` input is in `tool_input.command` but do not show the patch format
  or how paths appear. The header parsing follows the public `apply_patch` grammar; `hookah` also
  accepts `tool_input.patch`. Neither was verified against a live Codex session.
- Community reports say `apply_patch` hook coverage has been intermittent in some versions
  (upstream issue #16732); if edits are not checked, the stop phase still catches them.
- Hosted tools and hooks with `async: true` are not supported here.

## Verified against

<https://developers.openai.com/codex/hooks> (redirects to <https://learn.chatgpt.com/docs/hooks>),
read on 2026-11-17: event names (`SessionStart` with `source` matchers `startup`, `resume`,
`clear`, `compact`, `fork`, and `session_id` in every payload), matcher syntax, stdin fields (`cwd`, `tool_name`,
`tool_input.command`, `stop_hook_active`), `decision`/`reason` JSON, `additionalContext`,
`systemMessage`, exit codes. Not verified: the patch text format and live behavior.
