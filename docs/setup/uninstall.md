# Uninstall

`hookah uninstall` is the exact inverse of `hookah setup`: it takes out what setup wrote, and
nothing else. This page has the one-command path, the manual path for every harness (for when the
binary is gone or you prefer to edit by hand), where everything lives per OS, and how to verify that
nothing is left.

**For agents.** If the user says "put out my hookah", or asks to remove or uninstall hookah, follow
this page. Do not hand-edit the settings files while the `hookah` binary still works: run the
commands in [the one-command path](#the-one-command-path). Do not restart the harness yourself.

## The one-command path

Run these from the project root, in this order (the binary must still exist while you run the first):

```sh
hookah uninstall --all              # every harness `hookah setup --detect` reports as configured
hookah uninstall orca               # only if you installed the Orca plugin (see below)
hookah uninstall --all --purge-cache   # optional: also delete hookah's cache (can be combined with the first line)
```

then remove the binary:

```sh
curl -fsSL https://izevg.github.io/hookah/install.sh | sh -s -- --uninstall      # Linux and macOS
```

```powershell
$env:HOOKAH_UNINSTALL = '1'; irm https://izevg.github.io/hookah/install.ps1 | iex   # Windows
```

Both installers run `hookah uninstall --all` themselves before they delete the binary, because hooks
left pointing at a deleted binary fail on every agent edit, and the command that removes them would
be gone. Pass `--keep-config` (`$env:HOOKAH_KEEP_CONFIG = '1'` on Windows) to remove only the binary.
Installed with `cargo install`? Run `cargo uninstall hookah` instead.

Then restart each harness: it reads its hooks at startup.

### What `hookah uninstall` does

```text
hookah uninstall <harness>|--all [--scope project|user] [--dry-run] [--purge-cache] [--force]
hookah uninstall orca [--dir DIR]
hookah uninstall --purge-cache
```

It is shaped like `hookah setup`: the harness is the positional argument, `--scope` and `--dry-run`
mean the same, and `<harness>` without `--scope` picks the scope setup would have picked (project;
user for `kimi`, `hermes`, and `openclaw`). With `--all` and no `--scope`, both scopes are walked.

- `--all` removes hookah from every harness that `hookah setup --detect` reports as configured, at
  the given scope.
- `--dry-run` prints what would be removed and writes nothing.
- `--purge-cache` also deletes hookah's cache directory (see [Where things live](#where-things-live)).
  With `HOOKAH_CACHE_DIR` set, only the per-project directories inside it (16 hexadecimal digits) are
  deleted, because that directory may hold anything.
- `--force` deletes a plugin file even when its content is not what hookah writes.
- `orca` is the folder `hookah plugin orca --out` wrote (default `~/.hookah-orca-plugin`); `--dir`
  names another. `--all` does not cover it, because setup does not manage it.

Rules you can rely on:

- Only hookah's own entries leave a config file: a hook whose command runs `hookah hook <harness>`,
  whatever the path, the quoting, or a `mise exec --` prefix. A hook group, an event, or a table is
  removed only when this removal emptied it. Every other key, hook, and comment stays, byte for byte
  where the file was written the way setup writes it (JSON files are written with two-space
  indentation, as setup does).
- A file that setup created from nothing and that now holds nothing is deleted, and so are the empty
  directories above it up to the harness's own directory.
- A plugin file is deleted only while its content is what hookah writes. If you edited it (or another
  hookah version wrote it), uninstall keeps it, says why, and exits with code 1. Delete it by hand,
  or run again with `--force`.
- A config file that does not parse is kept, with the reason, and exit code 1.
- The `<file>.hookah.bak` backup that setup made is never restored, because you may have changed
  the file since. It is left in place and named in the output. Delete it when you no longer need it.
- Running it twice is safe: the second run finds nothing and exits with code 0, as does a run where
  hookah was never installed.
- It never touches your project's `.hookah.toml`, and it never installs anything or uses the network.
  (hookah's self-update is the one thing that does; see [Updates](update.md).)

The output is plain text, one fact per line: `removed hookah's entries from <file>`, `deleted <file>`,
`kept <file>: <reason>`, then a `next:` line (restart the harness).

## The manual path

Use this when the binary is already gone, or when you want to see every edit. Paths are for the
project scope (relative to the project root) and the user scope.

### Claude Code and Qoder

Files: `.claude/settings.json` or `~/.claude/settings.json`; `.qoder/settings.json` or
`~/.qoder/settings.json`.

Under `hooks`, delete every hook object whose `command` contains `hookah hook claude` (or
`hookah hook qoder`) in the events `SessionStart`, `PostToolUse`, `Stop`, and `SubagentStop` (Claude
Code) or `SessionStart`, `PostToolUse`, and `Stop` (Qoder). If that leaves a group
(`{"matcher": ..., "hooks": []}`) or an event empty, delete it too. If `hooks` is empty, delete it.

### ZCode

Files: `.zcode/config.json` or `zcode.json` in the project, and `~/.zcode/cli/config.json` for
the user scope.

Under `hooks.events`, remove each `SessionStart`, `PostToolUse`, and `Stop` hook whose
process runner points to `hookah` (or `hookah.exe`) with `["hook", "zcode"]` as its
arguments. For a shell runner, remove the entry that runs `hookah hook zcode`. Remove groups
and event lists that this leaves empty. Start a new ZCode session or run `/reload`.

### Codex CLI

Files: `.codex/config.toml` or `~/.codex/config.toml`.

Delete each `[[hooks.<Event>.hooks]]` table whose `command` contains `hookah hook codex`, in the
events `SessionStart`, `PostToolUse`, and `Stop`, together with the `[[hooks.<Event>]]` table above
it when it holds nothing else. Codex asks you to trust hooks, so also check `/hooks` after a restart.

### Kimi Code

File: `~/.kimi-code/config.toml` (user scope only).

Delete each `[[hooks]]` table whose `command` contains `hookah hook kimi` (two of them: events
`SessionStart` and `PostToolUse`).

### Antigravity CLI

Files: `.agents/hooks.json` or `~/.gemini/config/hooks.json`.

In the `hookah` group, delete the commands containing `hookah hook antigravity` and any
matcher groups or events this empties. Remove the `hookah` group if it is empty.

### Cursor

Files: `.cursor/hooks.json` or `~/.cursor/hooks.json`.

Under `hooks`, delete each entry whose `command` contains `hookah hook cursor` in the events
`sessionStart`, `afterFileEdit`, and `stop`. Delete events and `hooks` when they end up empty. Leave
`"version": 1`.

### opencode

Delete the plugin file: `.opencode/plugins/hookah.ts`, or `~/.config/opencode/plugins/hookah.ts`
(`$XDG_CONFIG_HOME/opencode/plugins/hookah.ts` when that variable is set).

### Pi

Delete the extension file: `.pi/extensions/hookah.ts`, or `~/.pi/agent/extensions/hookah.ts`
(`$PI_CODING_AGENT_DIR/extensions/hookah.ts` when that variable is set). In a running Pi session,
run `/reload`.

### Hermes Agent

Home: `~/.hermes`, or `$HERMES_HOME` (on Windows `%LOCALAPPDATA%\hermes` when that is how it is set).

1. In `config.yaml`, remove the line `- hookah` from `plugins.enabled` (and the `enabled:` and
   `plugins:` keys above it if nothing else is under them). Or run `hermes plugins disable hookah`.
2. Delete the folder `plugins/hookah/` (it holds `plugin.yaml` and `__init__.py`).

### OpenClaw

State directory: `~/.openclaw`, or `$OPENCLAW_STATE_DIR`.

1. Run `openclaw plugins disable hookah`. (`hookah uninstall openclaw` prints this command; it does
   not edit OpenClaw's own config.) An entry `plugins.entries.hookah` may remain in OpenClaw's
   config; delete it there if you want it gone.
2. Delete the folder `extensions/hookah/` (it holds `index.js` and `openclaw.plugin.json`).
3. Restart the OpenClaw Gateway.

### Orca

Delete the folder you passed to `hookah plugin orca --out` (default `~/.hookah-orca-plugin`). Then
remove the plugin from Orca: `orca plugin uninstall hookah` if you installed it, or
`orca plugin dev unload <folder>` if you loaded it for development. The hooks of the agents Orca
hosts are the harnesses above; remove them the same way.

### Backups

Setup copied each config file it changed to `<file>.hookah.bak` (for example
`.claude/settings.json.hookah.bak`) once, before its first change. They are left in place.
If you have not touched the file since setup, you may copy the backup over it; otherwise merge by
hand. Delete the backups when you are done.

## Where things live

| What | Linux | macOS | Windows |
|------|-------|-------|---------|
| Binary (install script) | `~/.local/bin/hookah` | `~/.local/bin/hookah` | `%LOCALAPPDATA%\hookah\bin\hookah.exe` |
| Binary (`cargo install`) | `~/.cargo/bin/hookah` | `~/.cargo/bin/hookah` | `%USERPROFILE%\.cargo\bin\hookah.exe` |
| `PATH` entry | none: `install.sh` prints a line to add yourself, in a shell profile you edited | same | the install directory, in the **user** `PATH`, added by `install.ps1` |
| Cache | `$XDG_CACHE_HOME/hookah`, else `~/.cache/hookah` | `~/Library/Caches/hookah` | `%LOCALAPPDATA%\hookah\cache` |
| Update state | `<cache>/update/` (`state.json` and a lock file) | same | same |

Notes:

- `HOOKAH_CACHE_DIR` (an absolute path) overrides the cache location on every OS. `--dir` or
  `HOOKAH_INSTALL_DIR` moves the binary.
- The self-update ([Updates](update.md)) keeps its state and lock in `update/` inside the cache. On
  Windows it may also leave `hookah.exe.old` next to the binary; the installer's uninstall does not
  mind it, delete it if it stays. `--purge-cache` removes the `update` directory too, in a
  `HOOKAH_CACHE_DIR` as well.
- The cache holds one directory per project (named with 16 hexadecimal digits): finding baselines, the
  latest check status, and tool caches. It is safe to delete at any time; hookah rebuilds it.
- `install.sh` never edits a shell profile, so on Linux and macOS there is no `PATH` entry of
  hookah's to remove. If you added `export PATH=...` to a profile yourself, remove that line.
- `install.ps1 -Uninstall` removes the install directory from your user `PATH` when the directory is
  gone (it removes the directory when it ends up empty). A directory that still holds other programs
  keeps its entry. With `HOOKAH_NO_MODIFY_PATH=1` the `PATH` is left alone. To do it by hand:
  Settings, "Edit environment variables for your account", and remove the entry from `Path`; or in
  PowerShell:

  ```powershell
  $dir = "$env:LOCALAPPDATA\hookah\bin"
  $path = [Environment]::GetEnvironmentVariable('Path', 'User') -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -ine $dir }
  [Environment]::SetEnvironmentVariable('Path', ($path -join ';'), 'User')
  ```

## Verify that nothing is left

1. Ask hookah (while the binary exists):

   ```sh
   hookah setup --detect
   ```

   Every harness must say `project: not configured` or `no project config`, and `user: not
   configured`. Run it in each project you set up with `--scope project`.

2. Look for leftovers in the files. On Linux and macOS, from the project root:

   ```sh
   grep -n "hookah" .claude/settings.json .codex/config.toml .qoder/settings.json .cursor/hooks.json \
     ~/.claude/settings.json ~/.codex/config.toml ~/.qoder/settings.json ~/.kimi-code/config.toml \
     .zcode/config.json zcode.json ~/.zcode/cli/config.json ~/.cursor/hooks.json ~/.hermes/config.yaml 2>/dev/null
   ls .opencode/plugins/hookah.ts .pi/extensions/hookah.ts ~/.config/opencode/plugins/hookah.ts \
     ~/.pi/agent/extensions/hookah.ts ~/.hermes/plugins/hookah ~/.openclaw/extensions/hookah \
     ~/.hookah-orca-plugin 2>/dev/null
   command -v hookah
   ls ~/.cache/hookah ~/Library/Caches/hookah 2>/dev/null
   ```

   On Windows (PowerShell):

   ```powershell
   Select-String -Pattern 'hookah' -Path .claude\settings.json, .codex\config.toml, .qoder\settings.json, .cursor\hooks.json, `
     "$HOME\.claude\settings.json", "$HOME\.codex\config.toml", "$HOME\.qoder\settings.json", "$HOME\.kimi-code\config.toml", `
     .zcode\config.json, zcode.json, "$HOME\.zcode\cli\config.json", "$HOME\.cursor\hooks.json", "$HOME\.hermes\config.yaml" -ErrorAction SilentlyContinue
   Get-Item .opencode\plugins\hookah.ts, .pi\extensions\hookah.ts, "$HOME\.config\opencode\plugins\hookah.ts", `
     "$HOME\.pi\agent\extensions\hookah.ts", "$HOME\.hermes\plugins\hookah", "$HOME\.openclaw\extensions\hookah", `
     "$HOME\.hookah-orca-plugin" -ErrorAction SilentlyContinue
   Get-Command hookah -ErrorAction SilentlyContinue
   Get-Item "$env:LOCALAPPDATA\hookah" -ErrorAction SilentlyContinue
   ```

   Nothing should print. What you may still see: `*.hookah.bak` backup files (they are not
   matched above), and a `hookah` mentioned in your own hook scripts. The project's `.hookah.toml`
   is your configuration and stays. If you moved a config with `XDG_CONFIG_HOME`, `HERMES_HOME`,
   `OPENCLAW_STATE_DIR`, or `PI_CODING_AGENT_DIR`, look there instead.

3. Restart the harness, and edit a file: no hookah report should appear.
