# Claude Code

## Mechanism

Command hooks. Claude Code runs `hookah hook claude` and sends a JSON payload on stdin.

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

Put `hookah` on `PATH`, then add this to `.claude/settings.json` in the project (or
`~/.claude/settings.json` for every project):

```json
{
  "hooks": {
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

## Limitations

- `PostToolUse` cannot undo the edit; the tool already ran. Exit 2 only feeds the report back.
- Claude Code caps hook output (`additionalContext`, `systemMessage`) at 10,000 characters;
  `hookah` reports are already capped by `--report-limit`.
- Only the `Edit`, `Write`, `MultiEdit`, and `NotebookEdit` tools are checked. Files changed
  through `Bash` are picked up by the stop phase only.
- The payload fields used here were checked against the current Claude Code hooks reference,
  except `stop_hook_active` and `notebook_path`, which come from earlier documentation.
