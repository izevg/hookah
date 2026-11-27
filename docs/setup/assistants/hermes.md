# Hermes Agent

Hermes Agent is the open-source agent CLI by [Nous Research](https://nousresearch.com)
(<https://github.com/NousResearch/hermes-agent>, MIT; docs at
<https://hermes-agent.nousresearch.com/docs/>). It is a general-purpose assistant that also codes,
so it sits under `assistants/`, apart from the coding-agent harnesses.

## Mechanism

Hermes has three extension points that matter here: **shell hooks** (a `hooks:` block in
`config.yaml`), **Python plugins** (`~/.hermes/plugins/<name>/`), and MCP. hookah uses a Python
plugin, because shell hooks cannot do the one thing hookah's edit phase needs. A shell hook sees
`post_tool_call`, but that event's return value is ignored, so it cannot put text in front of the
model after a file edit; only `transform_tool_result` can, and that event takes a string from a
Python callback. The only shell-hook event that continues the agent is `pre_verify`, so shell hooks
alone would give the stop phase and nothing else.

So `hookah` ships a small, dependency-free Python plugin that runs inside Hermes and calls the
`hookah` binary. The plugin source is embedded in the binary and printed by `hookah plugin
hermes`; `hookah setup hermes` installs it.

| hookah phase | Hermes event | How the text reaches the model |
| --- | --- | --- |
| Session start (baseline) | `on_session_start`, once for a new session | nothing is shown; `hookah` records the problems the project already has in the background (see [architecture](../../architecture.md#baseline)) |
| Edit phase | `transform_tool_result` after `write_file` and `patch` | the report is added to the tool result the model reads next, as a `hookah` field of that JSON result |
| Stop phase | `pre_verify`, when the agent is about to finish a turn in which it edited files | `{"action": "continue", "message": <report>}`: Hermes appends the message as a user turn and the agent keeps working |
| User-only warnings | none | a `WARNING` line in Hermes's own log (`agent.log`, logger `hookah`) |

Details worth knowing:

- **Edit phase.** For `write_file` the plugin takes the files Hermes reports as landed
  (`files_modified`, else `resolved_path`). For `patch` in V4A mode it also reads the `*** Add
  File:`, `*** Update File:`, and `*** Move File:` headers and skips deleted files. A tool result
  with an `error` is ignored. The plugin runs `hookah hook hermes` with a 30 s timeout. The
  report goes in as a JSON field, never as loose text after the JSON: Hermes parses the result of
  a file tool to decide whether the write landed, and a result that stops being JSON would make
  it forget the edit (and `pre_verify` would never fire).
- **Stop phase.** `pre_verify` fires once per attempt when the agent edited files this turn. The
  plugin runs the stop phase (checks over everything changed in the session, including related
  tests; 120 s timeout). When it fails, the plugin returns the report as a `continue` directive.
- **Working directory.** The agent often does not run in the process that hosts the plugin
  (Hermes serves sessions from a gateway process). The plugin asks Hermes for the base directory
  it resolves the session's relative paths against, and falls back to `TERMINAL_CWD`, then the
  process's directory. Reports show paths relative to it.

The plugin and the binary exchange one JSON document each way, versioned with `"v": 1`. It is the
same contract as the [opencode](../plugins/opencode.md#mechanism) plugin:

```json
{"v": 1, "event": "edit", "cwd": "/abs/project", "files": ["src/a.py"],
 "session_id": "...", "model": "anthropic/claude-sonnet-4-6", "already_continuing": false}
```

```json
{"v": 1, "block": true, "model_text": "hookah: 1 problem ...", "user_text": "..."}
```

`event` is `start`, `edit`, or `stop`; `files` is only used by `edit` (relative paths resolve
against `cwd`). In the reply, `block` means the model has to act on `model_text`. `model_text`
without `block` is advice (re-read a file the formatter rewrote). `user_text` is for the human
only. A plugin and a binary with different contract versions refuse to talk to each other, and the
plugin logs a warning to reinstall it.

## Setup

```sh
hookah setup hermes
```

Hermes loads plugins from its home directory only (`~/.hermes`, or `$HERMES_HOME`; on Windows
`%LOCALAPPDATA%\hermes`), so there is no project scope; `--scope project` is refused. Setup does
three things:

1. writes `plugins/hookah/plugin.yaml` and `plugins/hookah/__init__.py` (the text of `hookah
   plugin hermes`) under the Hermes home;
2. adds `hookah` to `plugins.enabled` in `config.yaml`, because Hermes loads a plugin only when it
   is on that allow-list (creating the file, the `plugins:` key, or the `enabled:` list when
   missing);
3. leaves everything else in `config.yaml` alone, byte for byte, comments included.

The `config.yaml` edit is a conservative line editor, not a YAML round trip. It only appends. If
`plugins` or `plugins.enabled` is written in a form it cannot edit safely (`plugins: {}`, or a
flow list such as `enabled: [a, b]`), or if `hookah` is in `plugins.disabled`, setup exits with
code 1, says what to change, and writes nothing: add `hookah` to `plugins.enabled` by hand and run
it again (the second run only installs the plugin files). Changed files are backed up once to
`<file>.hookah.bak`. A second run changes nothing. `--dry-run` lists what would be written, and
`--print` prints the `plugins.enabled` snippet. The plugin embeds no command: it runs `hookah`
from `PATH`, or from `HOOKAH_BIN`, so `--command` has no effect here. Run setup again after
upgrading `hookah` to get the matching plugin. Then restart Hermes.

`hookah setup --detect` lists Hermes as configured when the plugin files exist and `hookah` is on
the allow-list.

### Manual setup

1. Install `hookah` somewhere on `PATH`.
2. Create `~/.hermes/plugins/hookah/` with `hookah plugin hermes` as `__init__.py` and this
   `plugin.yaml`:

   ```yaml
   name: hookah
   version: "1.0"
   description: Runs the project's own format, lint, and test checks on what the agent edits.
   provides_hooks:
     - on_session_start
     - pre_llm_call
     - transform_tool_result
     - pre_verify
   ```

3. Run `hermes plugins enable hookah` (or add `hookah` to `plugins.enabled` in `config.yaml`).
4. Restart Hermes.

### Slow stop checks

Hermes abandons a plugin hook that runs longer than `plugins.hook_callback_timeout` (30 s by
default, at most 600) and carries on as if it had returned nothing. That fits the edit phase, but a
stop phase that runs a test suite can take longer, and then its report is lost. Raise the limit in
`config.yaml` if that happens:

```yaml
plugins:
  hook_callback_timeout: 120
```

Setup does not change it for you.

### Toolchain on `PATH`

`hookah` runs the project's own tools (`cargo`, `npx`, `ruff`, ...), and it inherits the
environment of the Hermes process that hosts the agent. Hermes runs sessions in a gateway process
it starts itself, so the environment of the shell you start `hermes` from usually, but not
always, reaches it. To make the right tools available either way, point `HOOKAH_BIN` at a wrapper
script, for example `$HOME/.local/bin/hookah-mise`:

```sh
#!/bin/sh
exec "$HOME/.local/bin/mise" exec -- "$HOME/.local/bin/hookah" "$@"
```

and set `HOOKAH_BIN` in `~/.hermes/.env` (Hermes reads that file) or in the shell that starts
Hermes.

## Response policy

| Situation | What happens |
| --- | --- |
| Edit phase finds problems | Report added to the tool result the model reads (`block: true`) |
| Formatter rewrote the file | Re-read hint added to the tool result |
| Tool missing for a check | Advice for the model added to the tool result, and a log line for the user |
| Stop phase finds problems, first attempt | `pre_verify` returns `continue` with the report; the agent keeps working |
| Stop phase finds problems, after a continuation | Not continued again; log line for the user |
| Checks degraded (timeout, tool crashed) | Log line only; never blocks |
| `hookah` missing, crashed, timed out, or answered garbage | Log line only; the tool call and the turn are untouched (fail open) |

## Loop guard

Hermes passes `pre_verify` an `attempt` counter: 0 on the first call of a turn, and one higher
after each continuation. The plugin sends `already_continuing: attempt > 0` to `hookah`:

1. First attempt: stop checks fail, `hookah` answers `block: true`, the plugin continues the
   agent with the report.
2. Second attempt, after the agent reacted: checks run again with `already_continuing: true`. If
   they still fail, `hookah` answers `block: false` with a summary for the user; the plugin only
   logs it, and the turn ends.

This mirrors the `stop_hook_active` policy of the Claude Code adapter: block once, then only warn
the user. The decision lives in `hookah` (`crates/harness/src/plugins/hermes/response.rs`).
Hermes bounds continuations on its own too (`agent.max_verify_nudges`, default 3), so even a
broken plugin cannot trap the loop. A turn that did not edit a file through `write_file` or
`patch` never fires `pre_verify`, so plain chat turns cost nothing.

## Limitations

- The tool already ran; a report is advice, not a rollback.
- Only `write_file` and `patch` are checked after the edit. Files changed through `terminal` are
  picked up by the stop phase only when some other edit in the turn made `pre_verify` fire.
- Hermes records a file edit for `pre_verify` from the tool result, so a result that carries
  `error` or no `bytes_written` (`write_file`) or `success: true` (`patch`) counts as no edit.
- Continuation feedback appears in the transcript as a user message, because that is how Hermes
  adds it.
- `on_session_start` fires for a brand-new session only; a resumed session gets its baseline
  lazily, at its first edit.
- Warnings for the user are log lines (`~/.hermes/logs/agent.log`); the plugin has no way to
  show a toast.
- Shell hooks and gateway event hooks are not used. Shell hooks also need a first-use consent per
  command, which the plugin avoids; a plugin needs only its one `plugins.enabled` entry.

## Verified against

Source of the `main` branch of <https://github.com/NousResearch/hermes-agent> (Hermes Agent
2026.9.24) and the docs at <https://hermes-agent.nousresearch.com/docs/user-guide/features/hooks>
and <https://hermes-agent.nousresearch.com/docs/user-guide/features/plugins>, read on
2026-11-17.

Checked live, with that source installed (Python 3.14, `uv sync --frozen`, in a private
`HERMES_HOME`) against a local scripted OpenAI-compatible mock server (no API key). The plugin was
installed by `hookah setup hermes` into a `config.yaml` that already had a `plugins:` block:

- the plugin loads (`plugins.enabled`), and `on_session_start` runs the baseline start;
- after a `write_file` of a Python file with an unused import, the model's next request carries the
  tool result with a `hookah` field: `src/util.py:1:8: ... [F401]`, with a project-relative path;
- when the model then says it is done, `pre_verify` continues the agent once: the next request
  carries the stop report as a user message;
- after that continuation the model finishes again without fixing anything; the agent stops there
  (no third request), and `agent.log` holds `1 problem still failing; not blocking again`;
- with `HOOKAH_BIN` pointing nowhere, every hook logs a warning and returns `None`.

Hermes needs a service manager (launchd, systemd) to start its gateway runtime. In a sandbox with
none, the live run started the same runtime as a plain child process by disabling Hermes's
service discovery in a small launcher; that changes how the runtime starts, not how plugins load or
hooks fire.

Not checked live: `patch` in V4A mode (the header format comes from Hermes's patch parser and
tool schema), Windows, and a real model.
