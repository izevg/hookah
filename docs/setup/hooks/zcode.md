# ZCode

ZCode starts `hookah hook zcode` as a native command hook and passes one JSON object on stdin.

| Event | What Hookah does | Feedback |
|-------|------------------|----------|
| `SessionStart` (`startup` or `resume`) | Starts the session baseline | Warnings on stderr |
| `PostToolUse` (`Write`, `Edit`, or `ApplyPatch`) | Formats and lints the files the tool changed | Findings and formatter notices in `hookSpecificOutput.additionalContext` |
| `Stop` | Checks the files changed during the session | `decision: "block"` and a reason ask ZCode to continue once. If the next attempt still fails, Hookah reports findings on stderr |

## Set up

Run from the project root:

```sh
hookah setup zcode
hookah setup --detect
```

Project setup normally writes `.zcode/config.json`. If the project uses `zcode.json` and has no
`.zcode/config.json`, or if Hookah is already set up in `zcode.json`, setup uses that file.
For all of your projects, use
`hookah setup zcode --scope user` to edit `~/.zcode/cli/config.json`.

In ZCode, open **Settings → Hooks**. Select the workspace and click **Trust** for the new
entries when prompted. User-level hooks require no separate trust review. If the config already
has `hooks.enabled: false`, Hookah preserves it. Enable hooks in ZCode when you are ready to
use them. Start a new session or run `/reload` to load the changes.

From the ZCode terminal CLI, use `zcode hooks trust review --workspace "$PWD" --json` to inspect
the project hooks. The ZCode CLI hooks guide explains how to grant trust to the reviewed
entries by digest. The CLI bundled with ZCode 4.0.0 skips workspace hooks in `--prompt` mode
even after granting trust. Use `hookah setup zcode --scope user` for headless runs with that build.

Use `hookah setup zcode --dry-run` to preview a merge or `hookah setup zcode --print` to print
the complete snippet. By default, setup uses a `process` runner with the absolute executable
path and separate arguments, so paths can contain spaces. To run Hookah through a shell command
such as a mise wrapper, use `hookah setup zcode --command "mise exec -- hookah"`.

## Manual configuration

Merge the groups printed by `hookah setup zcode --print` into `hooks.events` in the selected
file. The following is a minimal example for the edit phase. Replace the path with the absolute
path to your binary. On Windows, use forward slashes or escaped backslashes in the JSON path
and include the `.exe` suffix.

```json
{
  "hooks": {
    "enabled": true,
    "events": {
      "PostToolUse": [
        {
          "matcher": "Write|Edit|ApplyPatch",
          "hooks": [
            {
              "type": "process",
              "command": "/absolute/path/to/hookah",
              "args": ["hook", "zcode"],
              "timeoutMs": 30000
            }
          ]
        }
      ]
    }
  }
}
```

To remove the project or user hooks, run `hookah uninstall zcode` with the same scope you used
for setup, then start a new session or run `/reload`.

See the [ZCode hooks reference](https://zcode.z.ai/en/docs/hooks) for the protocol and the
[ZCode CLI hooks guide](https://github.com/zai-org/ZCode/blob/main/apps/zcode-cli/docs/usage/09-hooks.md)
for the terminal trust flow.
