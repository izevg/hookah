# Cursor

Covers the Cursor IDE agent and the `cursor-agent` CLI (`agent`); both read the same
`hooks.json`.

## Mechanism

Command hooks. Cursor runs `hookah hook cursor` and sends one JSON object on stdin. Every payload
carries `hook_event_name` and `workspace_roots`.

- `sessionStart` records the problems the project already has, in the background, so later
  reports show only what the agent introduced (the baseline; see
  [architecture](../../../architecture.md#baseline)). The hook returns at once and prints nothing
  (its `additional_context` output would reach the model). The `session_id` of this event equals
  the `conversation_id` of the later ones, which is how edits and stops find the baseline.
- `afterFileEdit` runs the edit phase for `file_path`.
- `stop` runs the stop phase (checks over the files changed in the session, including related
  tests), rooted at the first of `workspace_roots`. A stop with `status` `aborted` or `error` is
  skipped.

**The edit hook cannot talk to the model.** `afterFileEdit` has no documented output, and exit
code 2 only blocks hooks that run before an action. So the edit phase still formats the file
(Cursor documents this use) and checks it, but what it finds goes to stderr, which Cursor shows
in its Hooks output for the user. **`stop` is the enforcement point:** the stop phase re-runs the
static checks over every changed file and the related tests, and a failure there continues the
agent.

| Situation | Response |
| --- | --- |
| Edit phase finds problems | exit 0, report on stderr (user only; the agent is not told) |
| Formatter rewrote the file | exit 0, re-read hint on stderr (user only) |
| Stop phase finds problems, `loop_count` is `0` | exit 0, stdout `{"followup_message": "<report>"}`; Cursor submits it as the next user message and the agent keeps working |
| Stop phase finds problems, `loop_count` above `0` | exit 0, summary on stderr; never continues twice in a row |
| `hookah` cannot finish (timeout, missing tool, internal error, bad payload) | exit 0, warning on stderr only (fail open) |

`loop_count` is how many automatic follow-ups the conversation already had. `hookah` follows up
only when it is `0`, so the loop ends after one round whatever Cursor's own `loop_limit` (default
5) says.

## Setup

Run this in the project:

```sh
hookah setup cursor                # .cursor/hooks.json in this project
hookah setup cursor --scope user   # ~/.cursor/hooks.json, for every project
```

Both write the same configuration as the manual snippet below. Existing settings and other hooks
are kept, a second run changes nothing, and the original file is backed up once to
`<file>.hookah.bak`. A file that does not parse is refused and left untouched. `--dry-run` shows
the result without writing, `--print` prints only the snippet, and `--command "mise exec --
hookah"` replaces the default command (the absolute path of the running binary).

`hookah setup --detect` lists Cursor as installed when `cursor-agent` or `cursor` is on `PATH` or
`~/.cursor` exists.

Then restart Cursor (or the `cursor-agent` session) and check the Hooks tab in Cursor Settings.
Project hooks run only in trusted workspaces.

### Manual setup

Put `hookah` on `PATH`, then add this to `.cursor/hooks.json` in the project (or
`~/.cursor/hooks.json` for every project):

```json
{
  "version": 1,
  "hooks": {
    "sessionStart": [{ "command": "hookah hook cursor", "timeout": 30 }],
    "afterFileEdit": [{ "command": "hookah hook cursor", "timeout": 30 }],
    "stop": [{ "command": "hookah hook cursor", "timeout": 120 }]
  }
}
```

`version` is required and is `1`. Entries are flat (no matcher groups) and `timeout` is in
seconds. Cursor runs project hooks from the project root and user hooks from `~/.cursor/`, so an
absolute command path is the safe choice for `--scope user`. The project's own toolchain must be on
`PATH` in the hook's environment; a project that pins its tools with mise can use
`mise exec -- hookah hook cursor` as the command.

On Windows the desktop app runs hook commands through PowerShell. A program path with spaces then
needs the call operator: `hookah setup cursor --command "& 'C:\Program Files\hookah\hookah.exe'"`.
This form was not tried on Windows.

To check by hand:

```sh
echo '{"hook_event_name":"afterFileEdit","workspace_roots":["'"$PWD"'"],"file_path":"'"$PWD"'/src/main.rs","edits":[]}' \
  | hookah hook cursor; echo "exit $?"
```

## Limitations

- The edit phase is advisory and user-only, as above. A mistake the agent makes surfaces when it
  stops, not right after the edit. The `postToolUse` hook can inject `additional_context` into the
  conversation, but the docs do not show the `tool_input` of the edit tools, so `hookah` does not
  use it.
- **`cursor-agent` differences.** The docs list `workspaceOpen` as running in the CLI too but do
  not say which other events run there. Forum reports (January to February 2026) say the CLI runs
  `afterFileEdit` and the shell and MCP hooks, did not run `stop` in early builds and later ones
  do, and omits common fields such as `conversation_id`. `hookah` therefore needs only
  `workspace_roots` (plus `file_path` for edits); without a session id there is no baseline and
  edits are reported with the changed-lines filter only. Where `stop` does not fire, nothing
  enforces the checks, because the edit phase cannot reach the model.
- Cloud agents run neither `sessionStart` nor user-level hooks.
- With several workspace roots, the stop phase checks the first one only.
- On Windows, one forum report says a `followup_message` from `stop` is not picked up. Nothing
  `hookah` can do about it.
- Files changed through the shell are picked up by the stop phase only (it asks `git`).

## Verified against

<https://cursor.com/docs/agent/hooks>, read on 2026-11-17: both config locations and `version`,
the event list, the common stdin fields, `afterFileEdit` (`file_path`, `edits`), `stop` (`status`,
`loop_count`, `followup_message`), `sessionStart` (`session_id`), `loop_limit`, exit codes (0
success, 2 block, other fail-open), and PowerShell on Windows. The CLI overview
(<https://cursor.com/docs/cli/overview>) does not mention hooks. The CLI differences above come
from the forum thread <https://forum.cursor.com/t/cursor-cli-hooks/148511> and are not confirmed
by Cursor. Nothing here was run against a live Cursor or `cursor-agent` (both need a login).
