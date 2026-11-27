# Configuration

> Partly implemented. The `[env]` table, `models` filtering, and `[update]` work today; tool
> commands, disabled checks, time budgets, report limits, and ignore globs are planned for the
> same files.

## Where configuration lives

| File | Scope | Committed? |
|------|-------|------------|
| `<project root>/.hookah.toml` | the project, shared with the team | yes |
| `<project root>/.hookah.local.toml` | the project, only for you | no |
| the user config (see below) | every project, selected by path | no, lives outside any repository |

All three are TOML. Unknown keys are ignored, so a file written for a newer `hookah` still gives an
older one the parts it understands. A file that does not parse is skipped with a warning on
stderr (`hookah: warning: config: <file>: <reason>; ignoring it`); `hookah` never blocks the agent
over configuration.

**The project root for configuration** is the nearest directory, starting at the directory a tool
runs in and walking up to the git checkout root, that holds a `.hookah.toml` or a
`.hookah.local.toml`. With no such file it is the git root, and with no git checkout the directory
itself. The walk never goes above the git root, so a stray file in a parent directory of the
checkout is not picked up.

Keep `.hookah.local.toml` out of version control without touching the repository's tracked files:

```sh
echo .hookah.local.toml >> .git/info/exclude     # only this clone
echo .hookah.local.toml >> ~/.config/git/ignore  # every repository of this user
```

### The user config

| OS | User config |
|----|-------------|
| Linux, macOS, other Unix | `$XDG_CONFIG_HOME/hookah/config.toml` when that is an absolute path, else `~/.config/hookah/config.toml` |
| Windows | `%APPDATA%\hookah\config.toml`, falling back to `%USERPROFILE%\AppData\Roaming\hookah\config.toml` |

`HOOKAH_CONFIG` overrides it on every OS: an absolute path to the file (an empty or relative value
is ignored). The file is optional. It holds one `[[project]]` section per group of projects:

```toml
[[project]]
path = "~/Work/Code/corp/**"        # a prefix or a glob, matched over the project root
[project.env]
JAVA_HOME = "/Library/Java/JavaVirtualMachines/temurin-17.jdk/Contents/Home"
MAVEN_ARGS = "-s {git_root}/keycloak/ci/settings.xml"
```

`path` is expanded like a value (see below). Without `*` or `?` it is a prefix: it selects that
directory and everything below it. With them it is a glob: `*` and `?` stay inside one path
segment, `**` spans any number of segments, and the glob selects any directory it matches and
everything below. `/` and `\` are the same, and on Windows case is ignored.

## The `[env]` table: the environment of the tools

`hookah` runs the tools a project already uses: `mvn`, `gradle`, `cargo`, `eslint`, `pytest`, and
so on. Some projects only work with an environment of their own: a JDK, a Maven `settings.xml`
for a corporate registry, a proxy, a `PATH` entry. The `[env]` table (a name to string map) sets
those variables for every tool process `hookah` starts for that project, in every phase (edit,
stop, and the baseline job), on top of the inherited environment.

```toml
# .hookah.toml or .hookah.local.toml
[env]
JAVA_HOME = "/Library/Java/JavaVirtualMachines/temurin-17.jdk/Contents/Home"
MAVEN_ARGS = "-s {root}/ci/settings.xml"
PATH = "{root}/bin:${PATH}"
```

In the user config the same table goes in a `[project.env]` of a `[[project]]` section.

**Precedence**, lowest to highest: `.hookah.toml`, then `.hookah.local.toml`, then the matching
`[[project]]` sections of the user config (in file order, later sections win). A variable
set by config also wins over one that `hookah` itself sets for a tool, and that wins over the
inherited environment. Variables merge per name, not per file.

**Values** are strings, expanded in one pass:

| Written | Becomes |
|---------|---------|
| leading `~` (alone or before `/` or `\`) | the home directory (`$HOME`, `%USERPROFILE%` on Windows) |
| `{root}` | the project root for configuration (see above) |
| `{git_root}` | the git checkout root, or the project root outside git |
| `${NAME}` | the inherited variable `NAME`, empty when it is not set |

Anything else stays as written, including `~` in the middle of a value: `MAVEN_ARGS = "-s
~/s.xml"` is not expanded, use `"-s ${HOME}/s.xml"`. An empty value sets the variable to the empty
string; there is no way to unset a variable.

The detection cache (see `architecture.md`) is not keyed by this environment: it holds Rust's
`cargo metadata`, which these variables almost never change. After changing a variable that
should affect it (a different `RUSTUP_TOOLCHAIN`, say), clear the cache or run once with
`HOOKAH_NO_CACHE=1`.

### Never put project settings in the hook command

Hooks are installed globally, one command for every project (`~/.claude/settings.json`,
`~/.codex/config.toml`, ...). Prefixing that command with `env JAVA_HOME=... hookah hook codex`
or wrapping it in a script gives every project the settings of one, and breaks the others. Put the
variables in `.hookah.local.toml` or the user config instead. This is a rule for agents too, see
[Let your agent install it](setup/agents.md).

Check what is in effect for a file or directory:

```console
$ hookah config path/to/project
root: /Users/x/Work/Code/corp/monorepo
git root: /Users/x/Work/Code/corp/monorepo
user config: /Users/x/.config/hookah/config.toml (read)
file: /Users/x/.config/hookah/config.toml
env:
  JAVA_HOME=/Library/Java/JavaVirtualMachines/temurin-17.jdk/Contents/Home  (/Users/x/.config/hookah/config.toml [[project]] path = "~/Work/Code/corp/**")
  MAVEN_ARGS=-s /Users/x/Work/Code/corp/monorepo/keycloak/ci/settings.xml  (...)
models: all
```

It also prints the warning for every file that did not parse.

### Example: a corporate Maven repository, without touching the repository

The repository cannot take a `.hookah.toml`, and its Maven needs Java 17 and the company's
`settings.xml`. Add this once to the user config; it covers every checkout under `corp`:

```toml
# ~/.config/hookah/config.toml
[[project]]
path = "~/Work/Code/corp/**"
[project.env]
JAVA_HOME = "/Library/Java/JavaVirtualMachines/temurin-17.jdk/Contents/Home"
MAVEN_ARGS = "-s {git_root}/ci/settings.xml"
```

For one repository, a `.hookah.local.toml` in its root with the same `[env]` does it too.

### What `hookah` does for Java by itself

`hookah` runs `mvnw`/`gradlew` (or `mvn`/`gradle`) from the project directory, so the settings
those builds read natively keep working without any configuration: `.mvn/maven.config` and
`.mvn/jvm.config` (Maven), `gradle.properties` including `org.gradle.java.home`, and the
toolchains of both. `hookah` does not guess a `JAVA_HOME` from `.sdkmanrc`, `.java-version`, or
`.tool-versions`: they name a version, not a path, and the install location depends on the
machine, so set `JAVA_HOME` in `[env]`.

## Filtering by model: `models`

By default, once a harness is configured, `hookah` hooks run for every model that harness uses.
Set `models` to restrict hooks to specific models while keeping the same harness setup:

```toml
# ~/.config/hookah/config.toml (applies to every project),
# or .hookah.toml / .hookah.local.toml (applies to one project).
# Place top-level keys before [env], [update], or [[project]].
models = ["claude-sonnet-*", "gpt-5*"]
```

In the user config, a `[[project]]` section can also set its own `models` list (before
`[project.env]`):

```toml
# ~/.config/hookah/config.toml
models = ["claude-sonnet-*", "gpt-5*"]

[[project]]
path = "~/Work/Code/corp/**"
models = ["*"]   # run on every model in corp repositories
```

**Precedence**, lowest to highest (each layer that specifies `models` replaces the lower one;
omitting `models` inherits it):

1. Top-level `models` in the user config (`~/.config/hookah/config.toml`): global default for
   every project.
2. `<project root>/.hookah.toml`: shared project setting.
3. `<project root>/.hookah.local.toml`: personal override for one project.
4. Matching `[[project]]` sections of the user config, in file order (later sections win).
5. `HOOKAH_MODELS` in the environment: comma-separated list of patterns (for example
   `HOOKAH_MODELS="claude-sonnet-*,gpt-5*"`), winning over every file. A blank value is ignored.

**Matching rules:**

- Matching is ASCII case-insensitive. `*` matches any sequence of characters and `?` matches any
  single character.
- A pattern without `/` matches either the full model identifier or the bare model name after the
  last `/`: `claude-sonnet-*` matches both `claude-sonnet-4-6` and
  `anthropic/claude-sonnet-4-6`.
- A pattern containing `/` matches the full identifier only: `anthropic/claude-sonnet-*` matches
  `anthropic/claude-sonnet-4-6` and not `openai/claude-sonnet-4-6` or `claude-sonnet-4-6`.
- `models = ["*"]` (or `HOOKAH_MODELS="*"`) removes a narrower lower-precedence filter and runs on
  every model, including harnesses that do not report one.
- `models = []` matches no models and skips hooks for that scope.
- When `models` is restricted and a harness event carries no model identifier, the event is
  skipped silently. Manual `hookah check` calls are never filtered by `models`.

**How each harness reports its model:**

| Harness | Source |
|---------|--------|
| Codex CLI | `model` field in `SessionStart`, `PostToolUse`, and `Stop` payloads |
| Cursor | `model` field in common hook payload fields (may be omitted by `cursor-agent`) |
| Claude Code | `model` on `SessionStart` and `to_model` on `PostModelSwitch` (registered by `hookah setup claude` and cached per session when `models` is restricted); latest assistant `message.model` from `transcript_path` (or `agent_transcript_path` on `SubagentStop`) on `PostToolUse` and `Stop` |
| Antigravity CLI | `modelName` field in `SessionStart`, `PreInvocation`, and `Stop` payloads (cached per conversation when `models` is restricted so `PostToolUse` resolves it too) |
| Pi | active `ctx.model` (`provider/id`) sent by the bundled extension (`hookah setup pi`) |
| opencode | active session model (`provider/id`) tracked by the bundled plugin (`hookah setup opencode`); `session.created` starts as before and a non-blocking `start` runs once the first model-bearing event arrives so filtered sessions still get a baseline |
| OpenClaw | active session model (`provider/id`) tracked per session by the bundled plugin (`hookah setup openclaw`) |
| Hermes Agent | `model` tracked per session from `on_session_start`, `pre_llm_call`, tool results, and `pre_verify` (`hookah setup hermes`) |
| Kimi Code, Qoder, ZCode | read if `model` (or a JSONL transcript) is present; currently not sent by those harnesses, so hooks are skipped when `models` is restricted |

In a monorepo with nested projects, `Start` and `Stop` evaluate each discovered project against
its own configuration, so a child `.hookah.toml` can enable or disable hooks independently of the
repository root. The model filter gates hook checks only: self-update notices on `SessionStart`
remain governed by `[update]` / `HOOKAH_NO_UPDATE`, and no per-session model cache file is written
when `models` is unset.

After upgrading `hookah`, re-run `hookah setup <harness>` if you use Claude Code (to add
`PostModelSwitch`) or a bundled plugin (to update the installed plugin file; for manual single-file
installs, redirect `hookah plugin opencode > .opencode/plugins/hookah.ts` or
`hookah plugin pi > .pi/extensions/hookah.ts`).

**Verifying the filter:**

Check the effective `models` patterns and test specific model identifiers without running any checks:

```console
$ hookah config --model claude-sonnet-4-6
...
models: claude-sonnet-*, gpt-5*  (/path/to/.hookah.toml)
model check: claude-sonnet-4-6 -> allow (matches pattern "claude-sonnet-*")

$ hookah config --model claude-opus-4-6
...
models: claude-sonnet-*, gpt-5*  (/path/to/.hookah.toml)
model check: claude-opus-4-6 -> skip (does not match configured models: claude-sonnet-*, gpt-5*)
```

To verify what model identifier `hookah` extracts from a real harness hook payload (including
session-cache and transcript fallback) and why it would allow or skip the hook, pipe a sample
payload into `hookah config --hook <harness>`:

```console
$ printf '%s\n' '{"conversationId":"s","workspacePaths":["'"$PWD"'"],"modelName":"gemini-4-neon-low"}' \
    | hookah config --hook antigravity
harness: antigravity
event: start
model: gemini-4-neon-low (payload)
scope: /path/to/project
models: claude-sonnet-*, gpt-5*  (/path/to/.hookah.toml)
decision: skip (does not match configured models: claude-sonnet-*, gpt-5*)
```

## The `[update]` table: self-update

hookah checks for a newer official release at a session start and installs it by itself (see
[Updates](setup/update.md)). The user config can change that; `.hookah.toml` cannot, because the
binary is shared by every project.

```toml
# ~/.config/hookah/config.toml
[update]
check = true   # false: no check, no install, no notice
auto = true    # false: check and notify only; `hookah update` installs
```

`HOOKAH_NO_UPDATE=1` in the environment does what `check = false` does and wins over the file.

## `.hookah.toml` (planned)

Optional project file that overrides auto-detection: tool commands per check kind, disabled
checks, time budgets, report limits, and ignore globs.

## Budgets

`hookah` is light by default (see "Light by default" in `architecture.md`): it runs light tests
only, and skips tests and mutation testing when the change is too big. Until `.hookah.toml` can
set them, these environment variables in the environment of the hook change the budgets. Each
takes a whole number; anything else is ignored and the default stays.

| Variable | Default | Meaning |
|----------|---------|---------|
| `HOOKAH_TEST_BUDGET` | `25` | The most related test targets that run at stop. More are not run, and the agent gets the notice `N related tests selected; above the light budget of M, run them yourself`. `0` runs no tests at all. |
| `HOOKAH_MUTATION_MAX_FILES` | `3` | The most changed code files (tests and sources) of a project mutation testing accepts. `0` switches mutation testing off. |
| `HOOKAH_DOWNSTREAM_BUDGET` | `10` | Maven reactors only: the most modules that depend on the edited module (the dependents) that the compile check and the lint runs build along with it. With more, only the module and its upstream compile, and the agent gets the notice `javac: the N modules that depend on this one were not compiled ...`. `0` compiles no dependents, silently. |
| `HOOKAH_MUTATION_MAX_LINES` | `150` | The most changed lines in them (versus `HEAD`) mutation testing accepts; a new file counts as long as the file. `0` switches mutation testing off. |

Over a mutation budget the run is skipped with `mutation testing skipped: change too large (X
files / Y lines, budget F / L)`. A budget never fails a check: the notice is non-blocking, and it
shows for the user as a warning. Only the stop phase has budgets; the edit phase runs no tests and
no mutation testing. Example, for a fast CI machine:

```sh
HOOKAH_TEST_BUDGET=100 HOOKAH_MUTATION_MAX_LINES=400 hookah check --changed
```

## Cache

Where `hookah` keeps baselines and session state, and how to clear it.

`hookah` writes only into its own cache directory and the project. Per project it keeps one
directory, `<cache>/<16 hex digits>`, named by a hash of the canonical project root. It holds the
Rust detection cache (`detect-rust.json` and `metadata.json`), see "Detection cache"
in `architecture.md`, and the incremental state of some tools.

| OS | `<cache>` |
|----|-----------|
| Linux and other Unix | `$XDG_CACHE_HOME/hookah` when that is an absolute path, else `~/.cache/hookah` |
| macOS | `~/Library/Caches/hookah` (an absolute `$XDG_CACHE_HOME` still wins) |
| Windows | `%LOCALAPPDATA%\hookah\cache`, falling back to `%USERPROFILE%\AppData\Local\hookah\cache` |

`HOOKAH_CACHE_DIR` overrides `<cache>` on every OS. Set it to an absolute path and `hookah` uses
that directory as `<cache>` itself (no `hookah` suffix added). An empty or relative value is
ignored. Tests and CI use it so the layout is identical everywhere.

On Unix the directories are created with mode `0700`. When no location can be resolved (no home
directory), `hookah` works without a cache.

### Switching the detection cache off

Set `HOOKAH_NO_CACHE=1` (any value other than empty or `0`) in the environment of the hook. Every
call then detects from scratch and writes nothing. `hookah detect <path>` prints a `cache:` line
that says `hit`, `miss`, or `off` (always `off` for Node, Python, and Java, which are not cached).

### Clearing the cache

The cache is safe to delete at any time; the next call rebuilds it.

```sh
rm -rf ~/.cache/hookah                 # Linux (or $XDG_CACHE_HOME/hookah)
rm -rf ~/Library/Caches/hookah         # macOS
rmdir /s /q "%LOCALAPPDATA%\hookah\cache"   # Windows
```

To clear one project only, delete its `<16 hex digits>` directory.

## Windows

Reports show paths with `/` separators on every platform (`src/lib.rs:3: message`), and an
absolute path outside the project as `C:/other/a.rs`. Tools are the ones the project provides:
`node_modules\.bin\<tool>.cmd` for Node and `<venv>\Scripts\<tool>.exe` for Python. A tool that
runs past its time limit is killed together with its child processes. See `architecture.md`
("Windows") for the mechanism.
