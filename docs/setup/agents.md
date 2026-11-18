# Let your agent install it

This page is the procedure an AI coding agent follows to install and configure hookah. A user
only has to paste `https://github.com/izevg/hookah` into their agent and ask: "install and
configure hookah for me." This page is also served as plain markdown at
<https://izevg.github.io/hookah/setup/agents.md>, and the whole documentation as one text file at
<https://izevg.github.io/hookah/llms-full.txt>.

**For agents.** Follow these steps literally, in order. Do not hand-edit settings files: `hookah setup` writes
them. Do not ask the user questions unless a step says so. Stop and report if a step fails.

## 1. Detect the operating system and the harness you run in

- Operating system: `uname -s` prints `Linux` or `Darwin`. On Windows (PowerShell) use
  `$env:OS`, which is `Windows_NT`.
- Harness: you are one of `claude` (Claude Code), `codex` (Codex CLI), `qoder` (Qoder CLI), `cursor`
  (Cursor IDE or `cursor-agent`), `kimi` (Kimi Code), `opencode`. If you are not sure, skip this: step 3 lists what is installed.

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

where `<harness>` is `claude`, `codex`, `qoder`, `cursor`, `kimi`, or `opencode`. This edits the project's
config (for `kimi`, the user's, because Kimi Code has no project config). Use `--scope user` to
configure every project of this user instead. Add `--dry-run` first if the user wants to see the
change. Rules you can rely on:

- Running it again is safe: it changes nothing when the config is up to date.
- Existing settings are kept. The original file is copied once to `<file>.hookah.bak`.
- If the existing config file does not parse, setup exits with code 1, prints the reason, and
  leaves the file untouched. Tell the user, and do not try to fix the file yourself.
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

Do not restart the harness yourself: it would end your own session.
