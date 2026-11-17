# Qoder

## Mechanism

Command hooks. Qoder CLI runs `hookah hook qoder` and sends a JSON payload on stdin.

- `PostToolUse` with matcher `Write|Edit` runs the edit phase for `tool_input.file_path`.
- `Stop` runs the stop phase (checks over the files changed in the session, including related
  tests).

| Situation | Response |
| --- | --- |
| Edit phase finds problems | exit 0, JSON `hookSpecificOutput.additionalContext` with the report |
| Formatter rewrote the file | exit 0, `additionalContext` asking the model to re-read the file |
| Stop phase finds problems | exit 2, report on stderr; Qoder injects it into the conversation and the agent keeps working |
| Stop phase finds problems and `stop_hook_active` is `true` | exit 0, summary on stderr; never blocks twice in a row |
| `hookah` cannot finish (timeout, missing tool, internal error, bad payload) | exit 0, warning on stderr only (fail open) |

The Qoder docs do not say that `PostToolUse` can block, so the edit phase uses the documented
`additionalContext` field instead of exit code 2.

## Setup

Put `hookah` at `$HOME/.local/bin/hookah`. Add this to `.qoder/settings.json` in the project (or
`~/.qoder/settings.json`; `.qoder/settings.local.json` also works):

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Write|Edit",
        "hooks": [
          { "type": "command", "command": "$HOME/.local/bin/hookah hook qoder", "timeout": 30 }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          { "type": "command", "command": "$HOME/.local/bin/hookah hook qoder", "timeout": 120 }
        ]
      }
    ]
  }
}
```

The project's own toolchain must be on `PATH` in the hook's environment. A project that pins its
tools with mise can use `$HOME/.local/bin/mise exec -- $HOME/.local/bin/hookah hook qoder` as the
command.

To check by hand:

```sh
echo '{"cwd":"'"$PWD"'","hook_event_name":"PostToolUse","tool_name":"Write","tool_input":{"file_path":"src/main.rs"}}' \
  | $HOME/.local/bin/hookah hook qoder; echo "exit $?"
```

## Limitations

- The tool already ran; the report is advice, not a rollback.
- Only `Write` and `Edit` are checked. Files changed through `Bash` are picked up by the stop
  phase only.
- The docs show `tool_input.file_path` for `Write` only; `Edit` is assumed to use the same field.
  `SubagentStop` is not handled.
- Whether `PostToolUse` honors exit code 2, and whether `systemMessage` is understood, is not
  documented; `hookah` relies on neither.
- Shell expansion of `$HOME` in hook commands was not verified; use an absolute path if it does
  not expand.

## Verified against

<https://docs.qoder.com/cli/hooks> and <https://docs.qoder.com/cli/hooks-reference>, read on
2026-11-17: config locations, common stdin fields, `Write` input, `Stop` fields including
`stop_hook_active`, exit codes, `hookSpecificOutput` with `hookEventName`, `additionalContext`.
The tools page (<https://docs.qoder.com/cli/tools>) names `Write` and `Edit` but not their
parameters.
