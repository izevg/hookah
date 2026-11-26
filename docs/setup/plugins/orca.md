# Orca

## Mechanism

[Orca](https://github.com/stablyai/orca) is a desktop orchestrator: it runs coding agents (Claude
Code, Codex, Qoder, Kimi Code, opencode, and others) in managed git worktrees. It has no hook
protocol of its own for `hookah` to speak, so there are two layers, and they do different jobs.

1. **Per-edit feedback comes from the agent Orca hosts.** An agent in an Orca worktree runs its own
   hooks, so `hookah setup <harness>` is all it takes. See the guide of that harness:
   [Antigravity](../hooks/cli/antigravity/index.md), [Claude Code](../hooks/cli/claude-code.md),
   [Codex](../hooks/cli/codex.md), [Qoder](../hooks/cli/qoder.md), [Kimi Code](../hooks/cli/kimi.md),
   [opencode](opencode.md), and [ZCode](../hooks/zcode.md).
2. **The Orca plugin adds what only Orca knows.** It is a small Orca plugin that ships inside the
   `hookah` binary. It never touches your code. It does three things:
   - shows the latest `hookah` verdict of every worktree, in a sidebar panel and in the status bar;
   - adds palette commands to check a worktree and to set up hooks for it;
   - suggests hook setup by default and can set up `hookah` right before Orca starts an agent.

The plugin only runs the `hookah` binary: `hookah status --json` to read, `hookah check --changed`
to check, `hookah setup` to configure. Every rule stays in `hookah` itself.

### What it shows

`hookah` records the outcome of each run (an edit hook or a stop hook) per project, in its cache
directory, and `hookah status` reads it back. The plugin asks for it again whenever an agent
finishes a turn, a workspace is created or removed, or the focus moves.

- **Status bar item.** `hookah: 3 problems` (red), `hookah: ok` (green), `hookah: skipped` (a run
  was degraded, which never blocks the agent), or `hookah` (nothing recorded yet) for the focused
  worktree. Hover for the first report lines; press it for the same text as a message.
- **Sidebar panel "hookah".** One row per local worktree: result, problem count, and how long ago
  it was checked. Select a row to see the failing lines, and press **Check now** to run a check.
  A monorepo worktree lists each of its projects.

The status is the latest run only. An edit-hook run says something about the edited file, a stop
run (or **Check now**) about everything changed. A stop run on a clean tree records a pass, so a
fixed problem disappears once the worktree is checked again.

### Palette commands

| Command | What it does |
| --- | --- |
| `hookah: check this worktree` | Runs `hookah check --changed` in the focused worktree in the background, then shows the result as a notification. This is the stop phase: static checks over every changed file, then related tests and mutation testing. |
| `hookah: set up hooks for this worktree` | For each installed agent that has no `hookah` hooks yet, writes its **project** config into the worktree (`hookah setup <harness> --scope project`). Agents without a project config (Kimi Code) are reported, never touched. |
| `hookah: show status of this worktree` | Shows the latest status as a message. The status bar item runs it. |

### Setting hookah up for hosted agents

Setup writes into the repository (`.claude/settings.json`, `.codex/config.toml`, and so on), and
those are usually tracked files, so the plugin never does it unasked. The setting **Hooks for
hosted agents** decides:

| Value | What happens when Orca is about to start an agent in a worktree where `hookah` is not set up for it |
| --- | --- |
| `suggest` (default) | A notification, once per worktree and agent: "hookah is not set up for claude". Nothing is written. |
| `auto` | `hookah setup <agent> --scope project` runs *before* the agent starts, so the agent reads the hooks at startup, and a notification says what was written. Agents without a project config (Kimi Code) only get the suggestion. |
| `off` | Nothing. |

"Not set up" means the agent is installed and has no `hookah` hooks in its project config or in
the user's config. If you ran `hookah setup <harness> --scope user` once, the plugin sees it and
stays silent. That is the quieter route: it needs no file in any repository. Review and commit (or
git-ignore) the project config when you use `auto`; some agents ask you to approve new hooks the
first time (Claude Code: `/hooks`).

The launch hook only prepares. It never blocks or changes the launch, it passes on any error
(Orca also passes after 2 s), and it never installs anything.

## Setup

Install `hookah` first (see [the install guide](../agents.md)) so that the `hookah` command works
for the account that runs Orca. A desktop app often has a shorter `PATH` than your shell: the plugin
also tries `~/.local/bin/hookah` (and `%LOCALAPPDATA%\hookah\bin\hookah.exe` on Windows), and the
setting **hookah command** takes an absolute path.

Write the plugin's files to a folder, then install it:

```sh
hookah plugin orca --out ~/.hookah-orca-plugin
orca plugin install ~/.hookah-orca-plugin
```

`orca plugin install` lists exactly what the plugin asks for and prints a command with an
`--approve` fingerprint. Read the list, and run that command only if you agree. The plugin asks
for:

| Access | Why |
| --- | --- |
| Run its own code (`trusted-worker`) | It starts the `hookah` binary with your account's access. |
| `workspace:read` | List worktrees and their paths, and know which one is focused. |
| `agent:read` | Learn when an agent finishes a turn, to refresh the status. |
| `notifications:show` | Show the result of a check and the setup notices. |
| `settings:own` | Read its three settings. |

It does **not** ask to type into terminals, launch agents, create or change workspaces, or read
terminal output. It needs Orca `>= 1.4.248`.

To develop or try it without installing, use `orca plugin dev load ~/.hookah-orca-plugin`. After
upgrading `hookah`, write the folder again (`hookah plugin orca --out` replaces only its own files)
and install it again with `orca plugin install`, so plugin and binary match. Files you add to the
folder stay.

### Settings

Under the plugin in Orca's Settings:

| Setting | Default | Meaning |
| --- | --- | --- |
| hookah command | `hookah` | The executable. Use an absolute path if Orca cannot find it. |
| Hooks for hosted agents | `suggest` | `suggest`, `auto`, or `off`; see above. |
| Check timeout (seconds) | `300` | How long **hookah: check this worktree** may run, tests included. |

### Verify

The plugin was verified on Orca 1.4.277 for macOS. The live checks covered installation and
upgrade, worktree verdicts in the sidebar and status bar, and project hook setup before the
Antigravity launch command.

1. In a worktree, ask an agent to edit a file with a problem. The status bar item turns red.
2. Open the **hookah** panel from the right sidebar: the worktree row says `failing`.
3. Run **hookah: check this worktree** from the palette. A notification shows the report.

From a terminal, the same facts are one command away:

```sh
hookah status --root <worktree>          # text
hookah status --root <worktree> --json   # what the plugin reads
hookah check --changed --root <worktree> # what the palette command runs
```

`hookah status` prints one JSON document: `{"v":1,"root":...,"overall":"fail"|"degraded"|"pass"|null,
"projects":[{"root":...,"status":null|{"phase":"edit"|"stop","standing":...,"problems":3,
"detail":null,"at_ms":...,"lines":[...]}}]}`. `overall` is the worst standing of the projects that
have a record, and `null` when none has. It runs no checks and always exits 0.

## Limitations

- **Local worktrees only.** Worktrees on an SSH host or a paired Orca server are skipped: the plugin
  runs the `hookah` on this machine.
- **The status is the latest run.** It is not a history, and two agents in one project overwrite
  each other's record.
- **No inline results.** The plugin reports; the hooks of the hosted agent still feed findings to
  the agent. A verdict never travels through Orca.
- **Agents supported at launch.** Setup on launch covers `antigravity`, `claude`, `codex`, `qoder`,
  `kimi`, `opencode`, and `zcode`. Launching ZCode through Orca requires a working `zcode` terminal
  launcher on `PATH`.

## Development

The sources are in `crates/harness/src/plugins/orca/plugin/` (`orca-plugin.json`, `src/`, and tests in
`test/`). `hookah plugin orca` embeds the manifest and `src/` with `include_str!`.

```sh
cd crates/harness/src/plugins/orca/plugin
orca plugin types .   # writes the typings into .orca/ (git-ignored)
npx tsc -p .          # type-check
orca plugin test .    # worker, panel, and client tests
orca plugin check .   # renders the panel once
```
