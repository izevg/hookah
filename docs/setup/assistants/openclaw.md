# OpenClaw

[OpenClaw](https://github.com/openclaw/openclaw) (docs: <https://docs.openclaw.ai>) is the
open-source personal AI assistant formerly known as Clawdbot and Moltbot. It is a general
assistant, not a coding agent, but its own file tools (`write`, `edit`, `apply_patch`) edit files
in its agent workspace, and it can also run coding agents (Claude Code, Codex, and others) for
you. `hookah` covers both paths, see "Direct and delegated editing".

## Mechanism

OpenClaw has two in-process extension systems. Internal hooks (`HOOK.md` scripts for command and
lifecycle events such as `command:new`) have no event for tool calls or file edits. Plugins do:
typed `api.on(...)` handlers inside the agent and tool lifecycle. So `hookah` ships a plugin: a
small, dependency-free JavaScript plugin that runs inside OpenClaw and calls the `hookah` binary.
Its source is embedded in the binary and printed by `hookah plugin openclaw`; `hookah setup
openclaw` also writes the manifest OpenClaw requires (`openclaw.plugin.json`).

- **Session start.** The `session_start` hook runs `hookah hook openclaw` with `event: "start"`
  and the session key. `hookah` records the problems the project already has in the background,
  so later reports show only what the agent introduced (the baseline; see
  [architecture](../../architecture.md#baseline)). OpenClaw's session start does not carry a
  working directory, so the plugin takes the agent workspace it learned from the
  `before_model_resolve` hook of the same session. Where no session start fires (for example
  `openclaw agent --local`), the first edit claims the baseline lazily.
- **Edit phase.** The plugin registers tool-result middleware
  (`api.registerAgentToolResultMiddleware`) for `write`, `edit`, and `apply_patch`, on the
  `openclaw` runtime. It runs before the runtime feeds the tool result back to the model, and it
  receives the tool arguments and the working directory. For each call, the plugin runs `hookah
  hook openclaw` (30 s timeout) and appends hookah's report to the tool result as one more text
  block. That is what the model reads next. `write` and `edit` name their file in `path` (also
  accepted: `file_path`, `filePath`); for `apply_patch` the plugin reads the `*** Add File:`,
  `*** Update File:`, and `*** Move to:` headers of the `input` text and skips deleted files.
  Failed tool calls are not checked.
- **Stop phase.** The `before_agent_finalize` hook runs when the agent is about to give its final
  answer. If the session edited files since the last check, the plugin runs the stop phase
  (checks over everything changed in the session, including related tests; 120 s timeout). When
  it fails, the plugin hands the model the report once, and the loop guard below stops a second
  round. How the report reaches the model depends on what OpenClaw allows, see "The stop phase
  and OpenClaw".
- **User-only warnings.** Missing tools, degraded checks, and "still failing" summaries never
  reach the model. The plugin writes them to the OpenClaw log as `hookah: ...` warnings
  (`api.logger.warn`), which also show on the console of `openclaw gateway run` and
  `openclaw agent --local`.

The plugin and the binary exchange one JSON document each way, the same contract as the
[opencode plugin](../plugins/opencode.md#mechanism) (`"v": 1`, events `start`, `edit`, `stop`),
so `hookah hook openclaw` shares its parser and response policy.

### The stop phase and OpenClaw

OpenClaw's `before_agent_finalize` hook can answer `{ action: "revise", reason }` to ask for one
more model pass. The plugin does so, and the Codex native hook relay honors it. The embedded
runner, which runs OpenClaw's own agent, does not: once a turn has had side effects (a file edit
is one), it logs `before_agent_finalize requested revision after potential side effects; not
rewinding` and finalizes anyway. Steering the active run from inside the hook is refused too (the
run no longer accepts input at that point). So, for the stop phase, the plugin queues the report
as a next-turn injection (`api.session.workflow.enqueueNextTurnInjection`, a 24 hour lifetime):
the model gets it as context with the next message in that session, and the user gets a log
warning that the checks are failing. That is the strongest available path, and it is weaker than
the other harnesses: **the stop phase never keeps the agent working in the same turn.** The edit
phase is unaffected and gives the full per-edit loop.

## Setup

```sh
hookah setup openclaw     # ~/.openclaw/extensions/hookah/
```

It writes `index.js` (exactly the text of `hookah plugin openclaw`) and `openclaw.plugin.json`
into the extensions directory. The state directory follows `OPENCLAW_STATE_DIR`, else
`$OPENCLAW_HOME/.openclaw`, else `~/.openclaw`. Setup is user-level only (`--scope project` is
refused): OpenClaw loads project extensions only from its own agent workspace
(`<workspace>/.openclaw/extensions`), which is usually not the project you edit. Existing files are
backed up once to `<file>.hookah.bak`; run the command again after upgrading `hookah` to get the
matching plugin.
The plugin embeds no command: it runs `hookah` from `PATH`, or from `HOOKAH_BIN`, so `--command`
has no effect. `--dry-run` and `--print` work as for the other harnesses (`--print` shows the
entry point only).

Then do the two steps that setup does not do for you, because they change OpenClaw's own config
(a JSON5 file hookah does not edit):

```sh
openclaw plugins enable hookah
openclaw config set plugins.entries.hookah.hooks.allowConversationAccess true
```

and restart the Gateway (or run `openclaw plugins reload hookah`). Both are needed:

- OpenClaw treats an installed plugin's tool-result middleware as trusted, so it runs it only for
  a plugin that is explicitly enabled and that declares the middleware in its manifest. Without
  `enable`, the plugin loads but the edit phase stays silent.
- `before_agent_finalize` reads the conversation, which OpenClaw gates behind
  `allowConversationAccess`. Without it, the stop phase never runs. The edit phase does not need
  it.

`openclaw plugins list` should show `hookah` as loaded and enabled. OpenClaw may log "can't verify
where this plugin came from"; that is expected for a local plugin and does not stop it from
loading.

### Manual setup

```sh
mkdir -p ~/.openclaw/extensions/hookah
hookah plugin openclaw > ~/.openclaw/extensions/hookah/index.js
cat > ~/.openclaw/extensions/hookah/openclaw.plugin.json <<'EOF'
{
  "id": "hookah",
  "name": "hookah",
  "description": "Runs the project's own checks after file edits and feeds failures back.",
  "activation": { "onStartup": true },
  "contracts": { "agentToolResultMiddleware": ["openclaw"] },
  "configSchema": { "type": "object", "additionalProperties": false }
}
EOF
```

then the two `openclaw` commands above. The plugin reads one environment variable: `HOOKAH_BIN`,
the binary to run (default: `hookah` from `PATH`).

### Toolchain on `PATH`

`hookah` runs the project's own tools (`cargo`, `npx`, `ruff`, ...), and it inherits the
environment of the OpenClaw Gateway process. Start the Gateway through mise (`mise exec --
openclaw gateway run`), or point `HOOKAH_BIN` at a wrapper such as `$HOME/.local/bin/hookah-mise`:

```sh
#!/bin/sh
exec "$HOME/.local/bin/mise" exec -- "$HOME/.local/bin/hookah" "$@"
```

## Direct and delegated editing

- **Direct.** The OpenClaw agent edits files itself with `write`, `edit`, and `apply_patch`. This
  is what the plugin covers. The path of a relative file is resolved against the working
  directory OpenClaw reports for the tool call (the agent workspace). `hookah` finds the project
  from the file, so the workspace needs to be, or be inside, a project that has a manifest
  (`Cargo.toml`, `package.json`, `pyproject.toml`).
- **Delegated.** OpenClaw can run Claude Code, Codex, Gemini CLI, and other coding agents, over
  ACP (`/acp spawn`, `sessions_spawn({ runtime: "acp" })`) or through the native Codex app-server
  harness. Those agents edit files in their own tools, which this plugin does not see: OpenClaw
  relays a native Codex tool result only for observation, and the transformed result never
  reaches the model. For them, configure the agent itself: `hookah setup claude`, `hookah setup
  codex`, and so on, in the project they work in. Their hooks then fire inside their own
  processes, whoever started them. Whether an ACP adapter loads the agent's hook configuration was
  not verified, so check it on your setup.

## Response policy

| Situation | What happens |
| --- | --- |
| Edit phase finds problems | Report appended to the tool result the model reads (`block: true`) |
| Formatter rewrote the file | Re-read hint appended to the tool result |
| Tool missing for a check | Advice for the model appended to the tool result, and a log warning for the user |
| Stop phase finds problems, first time | Report queued for the next turn, plus a `revise` request for runtimes that honor one; log warning |
| Stop phase finds problems, right after that | Not sent again; log warning for the user (the loop guard) |
| Checks degraded (timeout, tool crashed) | Log warning only; never blocks |
| `hookah` missing, crashed, timed out, or answered garbage | Log warning only; the tool call is untouched (fail open) |

## Loop guard

The plugin keeps a per-session flag, "the last stop check handed the model a report", and sends
it to `hookah` as `already_continuing`:

1. Stop check after edits fails, no flag: hand over the report, set the flag.
2. Stop check at the next finalization (the next turn): runs again with `already_continuing:
   true`. If it still fails, `hookah` answers `block: false` with a summary for the user; the
   plugin only warns, and clears the flag. If it passes, the flag is cleared silently.
3. The turn after that starts with a clean flag.

The decision lives in `hookah` (`crates/harness/src/opencode/response.rs`, shared with opencode);
the plugin only keeps the flag. A session with no file edit since its last check is not checked,
so plain chat turns cost nothing. OpenClaw also bounds revisions itself (one extra pass for the
plugin's idempotency key).

## Limitations

- The tool already ran; a report is advice, not a rollback.
- Only `write`, `edit`, and `apply_patch` of OpenClaw's own runtime are checked after the edit.
  Files changed through `exec` (a shell command) are picked up by the stop phase only.
- The stop phase reaches the model on the next turn, not the same one (see above).
- The middleware runs inside the tool call, so a slow check delays the model's next step by up to
  the 30 s edit timeout.
- Changes to the plugin file need `openclaw plugins reload hookah` or a Gateway restart.
- The plugin is written against OpenClaw 2026.11.5. Its hook and middleware APIs are young; re-run
  `hookah setup openclaw` after upgrading `hookah`, and check `openclaw plugins list` after
  upgrading OpenClaw.

## Verified against

<https://docs.openclaw.ai/plugins/hooks/reference>, the plugin hook pages
(`docs/plugins/hooks/`), and the source of <https://github.com/openclaw/openclaw>, plus the
`openclaw` npm package, version 2026.11.5, read and run on 2026-11-17.

Checked live, with that OpenClaw version (installed with npm into a scratch directory) against a
local mock OpenAI-compatible model server (no API key, no messaging channels), with `openclaw agent
--local` and with a loopback Gateway (`openclaw gateway run`) plus `openclaw agent`:

- the plugin loads from `~/.openclaw/extensions/hookah/` after `hookah setup openclaw`, and runs
  once `openclaw plugins enable hookah` has been run (a global-origin plugin that is not enabled
  loads, but its middleware does not run);
- after a `write` of a file with an unused variable, the tool result the model receives in its next
  request carries `hookah: 1 problem after editing src/main.rs ...` (a Rust project, checked with
  Clippy);
- `tool_result_persist` is not enough for the edit phase: it only changes the stored transcript,
  while the model reads the in-memory result, which is why the plugin uses the middleware;
- `before_agent_finalize` runs only with `allowConversationAccess`, and its `revise` answer is
  ignored by the embedded runner after file edits (the log line quoted above);
- through the Gateway, `session_start` fires with the session key, and the stop report enqueued
  by the plugin appears in the model's request of the next turn in that session, where the second
  stop check only warns (the loop guard); with `--local` the next-turn queue refuses the entry
  (there is no stored session), so there the stop phase only warns;
- a missing `hookah` binary logs `` `/nonexistent/hookah` not found; install hookah or set
  HOOKAH_BIN; skipping checks `` and leaves the tool call untouched.

Not checked live: `apply_patch` (the mock model called `write` only; the argument name `input` and
the header format come from the OpenClaw source), the Codex native harness and ACP runtimes, and
channel-backed sessions.
