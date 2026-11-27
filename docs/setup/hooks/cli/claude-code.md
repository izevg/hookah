# Claude Code

## Mechanism

Command hooks. Claude Code runs `hookah hook claude` and sends a JSON payload on stdin.

- `SessionStart` with matcher `startup|resume` records the problems the project already has, in
  the background, so later reports show only what the agent introduced (the baseline; see
  [architecture](../../../architecture.md#baseline)). The hook returns at once and prints nothing.
- `PostModelSwitch` updates the session's active model when `models` filtering is configured so
  mid-session switches take effect before the next assistant turn is written to the transcript.
- `PostToolUse` with matcher `Edit|Write|MultiEdit|NotebookEdit` runs the edit phase (format and
  lint of the edited file).
- `Stop` and `SubagentStop` run the stop phase (checks over the files changed in the session,
  including related tests).

Feedback goes back through the hook protocol:

| Situation | Response |
| --- | --- |
| Edit phase finds problems | exit 2, report on stderr; Claude Code shows it to the model |
| Formatter rewrote the file | exit 0, JSON `hookSpecificOutput.additionalContext` asking the model to re-read the file |
| Stop phase finds problems | exit 2, report on stderr; the agent keeps working |
| Stop phase finds problems and `stop_hook_active` is `true` | exit 0, `systemMessage` for the user; never blocks twice in a row, so the agent cannot loop forever |
| `hookah` cannot finish (timeout, missing tool, internal error, bad payload) | exit 0, warning only (fail open) |

## Setup

Run this in the project:

```sh
hookah setup claude                # .claude/settings.json in this project
hookah setup claude --scope user   # ~/.claude/settings.json, for every project
```

Both write the same configuration as the manual snippet below. Existing settings are kept, a
second run changes nothing, and the original file is backed up once to `<file>.hookah.bak`. A file
that does not parse is refused and left untouched. `--dry-run` shows the result without writing,
`--print` prints only the snippet, and `--command "mise exec -- hookah"` replaces the default command
(the absolute path of the running binary).

Then restart Claude Code, or run `/hooks` in it to review and approve the new hooks.

### Windows

`hookah setup claude` writes the path of `hookah.exe` with forward slashes and quotes it when it
contains a space, for example `C:/Users/me/AppData/Local/hookah/bin/hookah.exe hook claude`. Claude
Code runs the `command` string through Git Bash on Windows (PowerShell only when Git Bash is not
installed), and Git Bash treats a backslash as an escape character, so a native `C:\...` path would
lose its separators. Windows accepts the forward slash form everywhere. Without Git Bash, a command
that starts with a quoted path is printed by PowerShell instead of run: install Git for Windows, or
write the command by hand with a leading `&` and `"shell": "powershell"`.
(Source: the "Command hook fields" section of <https://code.claude.com/docs/en/hooks>.)

### Manual setup

Put `hookah` on `PATH`, then add this to `.claude/settings.json` in the project (or
`~/.claude/settings.json` for every project):

```json
{
  "hooks": {
    "SessionStart": [
      {
        "matcher": "startup|resume",
        "hooks": [
          { "type": "command", "command": "hookah hook claude", "timeout": 30 }
        ]
      }
    ],
    "PostModelSwitch": [
      {
        "hooks": [
          { "type": "command", "command": "hookah hook claude", "timeout": 30 }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "Edit|Write|MultiEdit|NotebookEdit",
        "hooks": [
          { "type": "command", "command": "hookah hook claude", "timeout": 30 }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          { "type": "command", "command": "hookah hook claude", "timeout": 120 }
        ]
      }
    ],
    "SubagentStop": [
      {
        "hooks": [
          { "type": "command", "command": "hookah hook claude", "timeout": 120 }
        ]
      }
    ]
  }
}
```

Timeouts are in seconds. To confirm the hook fires, run `/hooks` in Claude Code, or pipe a payload
by hand:

```sh
echo '{"cwd":"'"$PWD"'","hook_event_name":"PostToolUse","tool_name":"Edit","tool_input":{"file_path":"src/main.rs"}}' \
  | hookah hook claude; echo "exit $?"
```

Claude Code runs the `command` string through the user's shell, so `$HOME` and a `PATH` lookup
expand as in a terminal. Prefer `$HOME` over `~` in a JSON command: tilde expansion depends on the
shell and on its position in the word, while `$HOME` always expands. A project that pins its
toolchain with mise can prefix the command with `mise exec --` so the pinned tools are on `PATH`.

## Dogfooding

This repository runs `hookah` on itself. Install the binary into `~/.local`:

```sh
cargo install --path crates/cli --root ~/.local --locked
```

The committed [`.claude/settings.json`](../../../../.claude/settings.json) calls
`$HOME/.local/bin/mise exec -- $HOME/.local/bin/hookah hook claude` on `SessionStart`
(timeout 30), `PostToolUse` (timeout 180), and `Stop` (timeout 900). Re-run the install after changing `hookah` to pick up
the new build.

## Limitations

- Without the `SessionStart` hook there is no up-front baseline: the first edit of a session
  starts it, and that edit is reported with the changed-lines filter only.
- `PostToolUse` cannot undo the edit; the tool already ran. Exit 2 only feeds the report back.
- Claude Code caps hook output (`additionalContext`, `systemMessage`) at 10,000 characters;
  `hookah` reports are already capped by `--report-limit`.
- Only the `Edit`, `Write`, `MultiEdit`, and `NotebookEdit` tools are checked. Files changed
  through `Bash` are picked up by the stop phase only.
- The payload fields used here were checked against the current Claude Code hooks reference,
  except `stop_hook_active` and `notebook_path`, which come from earlier documentation.
