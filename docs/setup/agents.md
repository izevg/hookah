# Let your agent install it

This page is the procedure an AI coding agent follows to install and configure hookah. A user
only has to paste `https://github.com/izevg/hookah` into their agent and say: **"Pack me a
hookah"** (install and configure hookah for me). This page is also served as plain markdown at
<https://izevg.github.io/hookah/setup/agents.md>, and the whole documentation as one text file at
<https://izevg.github.io/hookah/llms-full.txt>.

**For agents.** If the user says "pack me a hookah" (or something similar), it means: install hookah
and configure it for the harness you are running in; follow the procedure below. "Put out my hookah"
means: uninstall it, see [Uninstall](uninstall.md).

Follow these steps literally, in order. Do not hand-edit settings files: `hookah setup` writes
them. Do not ask the user questions unless a step says so. Stop and report if a step fails.

## 1. Detect the operating system and the harness you run in

- Operating system: `uname -s` prints `Linux` or `Darwin`. On Windows (PowerShell) use
  `$env:OS`, which is `Windows_NT`.
- Harness: you are one of `claude` (Claude Code), `codex` (Codex CLI), `qoder` (Qoder CLI), `cursor`
  (Cursor IDE or `cursor-agent`), `kimi` (Kimi Code), `opencode`, `hermes` (Hermes Agent), `pi` (Pi coding agent),
  `openclaw` (OpenClaw). If you are not sure, skip this: step 3 lists what is installed.

## 2. Install the binary

Skip this step if `hookah --version` already prints a version.

- Linux or macOS: `curl -fsSL https://izevg.github.io/hookah/install.sh | sh`
- Windows (PowerShell): `irm https://izevg.github.io/hookah/install.ps1 | iex`
- From source, if the scripts are unavailable and Rust is installed:
  `cargo install --git https://github.com/izevg/hookah --locked hookah`

Then run `hookah --version` again. If the command is not found, the install directory is not on
`PATH`: use the absolute path the install script printed for every `hookah` command below. The hooks
that setup writes always use the absolute path of the binary, so they do not depend on `PATH`.

## 3. List the harnesses

Run this from the project root:

```sh
hookah setup --detect
```

It prints one line per harness: whether it is installed, and whether hookah is already configured
for this project and for this user. Pick the harness you run in. If the user named several, repeat
steps 4 to 6 for each. Do nothing for a harness that is not installed.

## 4. Configure the harness

From the project root:

```sh
hookah setup <harness>
```

where `<harness>` is `claude`, `codex`, `qoder`, `cursor`, `kimi`, `opencode`, `hermes`, `pi`, or `openclaw`. This edits the
project's config (for `kimi`, `hermes`, and `openclaw`, the user's, because they have no project config; for
`hermes` it installs a plugin under `~/.hermes` and enables it in `config.yaml`; for `openclaw` it installs a
plugin under `~/.openclaw/extensions`). Use `--scope user` to
configure every project of this user instead. Add `--dry-run` first if the user wants to see the
change. Rules you can rely on:

- Running it again is safe: it changes nothing when the config is up to date.
- Existing settings are kept. The original file is copied once to `<file>.hookah.bak`.
- If the existing config file does not parse, setup exits with code 1, prints the reason, and
  leaves the file untouched. Tell the user, and do not try to fix the file yourself. (For `hermes`, setup also refuses a
  `plugins` or `plugins.enabled` written as an inline list or map; tell the user to add `hookah`
  to `plugins.enabled` and run setup again.)
- If the project pins its tools with mise, add `--command "mise exec -- hookah"`.

## 5. Verify

```sh
hookah setup --detect
hookah detect <any source file in the project>
```

The first must now list the harness as `configured` for the scope you used. The second prints the
format, lint, and test tools hookah found for that file. If it finds none, hookah stays silent for
that project, which is correct: it only runs tools the project already configures.

## 6. Tell the user what to do next

The harness reads its hooks at startup, and some ask for approval. Tell the user, in these words or
close to them: "hookah is configured in `<file written>`. Restart `<harness>`, and approve the new
hooks if it asks (Claude Code: run `/hooks`; Codex: run `/hooks` and trust them). After that,
hookah checks every file I edit." Print the `next:` line of the `hookah setup` output.

OpenClaw is the exception to "restart": its `next:` line names two `openclaw` commands to run
(`openclaw plugins enable hookah` and one `openclaw config set ...`), which you may run when the
user agrees, because setup does not edit OpenClaw's own config. See
[OpenClaw](assistants/openclaw.md).

Do not restart the harness yourself: it would end your own session.

## 7. Project-specific environment: never edit the hook command

The hook that setup wrote may be global (`--scope user`, `~/.codex/config.toml`,
`~/.claude/settings.json`): one command for every project. **Never add project-specific
environment variables, wrappers (`env JAVA_HOME=... hookah ...`, `sh -c`, scripts), or paths to the
hook command.** They would apply to every project of the user and break the others. This holds
when a tool fails for a missing JDK, a Maven `settings.xml`, a registry, or a `PATH` entry.

Put the variables where `hookah` reads them per project instead:

- `<project root>/.hookah.local.toml`, for this clone only (keep it out of git:
  `echo .hookah.local.toml >> .git/info/exclude`), or `.hookah.toml` when the team shares it;
- the user config, `~/.config/hookah/config.toml` (`%APPDATA%\hookah\config.toml` on Windows),
  with a `[[project]]` section whose `path` matches the repository, when the repository must not
  be touched.

```toml
[env]
JAVA_HOME = "/Library/Java/JavaVirtualMachines/temurin-17.jdk/Contents/Home"
MAVEN_ARGS = "-s {root}/ci/settings.xml"
```

Then verify with `hookah config <project directory>`: it lists the files read and every variable
with the file it came from. Details: [Configuration](../configuration.md).

## Uninstalling

To remove hookah, follow [Uninstall](uninstall.md). In short, from the project root and before the
binary is deleted: `hookah uninstall --all` (add `--dry-run` first if the user wants to see the
change), then `curl -fsSL https://izevg.github.io/hookah/install.sh | sh -s -- --uninstall` on Linux and
macOS, or `$env:HOOKAH_UNINSTALL = '1'; irm https://izevg.github.io/hookah/install.ps1 | iex` in
PowerShell. Both remove only what hookah wrote, and running them again is safe. Verify with
`hookah setup --detect` before the binary goes, and tell the user to restart the harness.
