# Kimi Code

## Mechanism

Command hooks (beta upstream) declared as `[[hooks]]` entries in `~/.kimi-code/config.toml`. Kimi
Code runs `hookah hook kimi` and sends a JSON payload on stdin.

- `PostToolUse` with matcher `Write|Edit` runs the edit phase for `tool_input.path`.
- `Stop` is not supported; see the limitations.

| Situation | Response |
| --- | --- |
| Edit phase finds problems | exit 0, report on stdout; Kimi may append stdout to the model context |
| Formatter rewrote the file | exit 0, a re-read hint on stdout |
| `hookah` cannot finish (timeout, missing tool, internal error, bad payload) | exit 0, warning on stderr only (fail open) |

## Setup

Put `hookah` at `$HOME/.local/bin/hookah`. Add this to `~/.kimi-code/config.toml`:

```toml
[[hooks]]
event = "PostToolUse"
matcher = "Write|Edit"
command = "$HOME/.local/bin/hookah hook kimi"
timeout = 30
```

A rule accepts only `event`, `matcher`, `command`, and `timeout` (1-600 seconds); any other field
makes the config fail to load. The project's own toolchain must be on `PATH` in the hook's
environment. A project that pins its tools with mise can use
`$HOME/.local/bin/mise exec -- $HOME/.local/bin/hookah hook kimi` as the command.

To check by hand:

```sh
echo '{"cwd":"'"$PWD"'","hook_event_name":"PostToolUse","tool_name":"Write","tool_input":{"path":"src/main.rs"}}' \
  | $HOME/.local/bin/hookah hook kimi; echo "exit $?"
```

## Limitations

- No stop phase. Kimi's `Stop` event can block (exit 2), but its payload has no documented
  `stop_hook_active` equivalent, so a failing check could trap the agent in a loop. `hookah` takes
  the safe option and ignores `Stop`. Failures are reported at edit time only, and related tests
  do not run.
- `PostToolUse` is documented as not blockable. Whether stdout of an exit-0 hook reaches the model
  for this event is documented only in general terms ("may be appended to context"), so a failing
  report may be invisible to the model.
- The hooks page does not give the `PostToolUse` payload fields or the tool parameters. The
  mapping (`Write` and `Edit` with `tool_input.path`) comes from the tools reference
  (<https://www.kimi.com/code/docs/en/kimi-code-cli/reference/tools.html>); the older names
  `WriteFile` and `StrReplaceFile` are accepted too. Not verified against a live session.
- Files changed through `Bash` are not checked.

## Verified against

<https://www.kimi.com/code/docs/en/kimi-code-cli/customization/hooks.html>, read on 2026-11-17:
`[[hooks]]` fields, event list, base stdin fields, exit codes, blockable events. Tool parameters
from the tools reference, read on the same date.
