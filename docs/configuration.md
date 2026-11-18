# Configuration

> Partly implemented. The `[env]` table below works today; tool commands, disabled checks,
> time budgets, report limits, and ignore globs are planned for the same files.

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

## `.hookah.toml` (planned)

Optional project file that overrides auto-detection: tool commands per check kind, disabled
checks, time budgets, report limits, and ignore globs.

## Budgets

`hookah` is light by default (see "Light by default" in `architecture.md`): it runs light tests
only, and skips tests and mutation testing when the change is too big. Until `.hookah.toml` can
set them, three environment variables in the environment of the hook change the budgets. Each
takes a whole number; anything else is ignored and the default stays.

| Variable | Default | Meaning |
|----------|---------|---------|
| `HOOKAH_TEST_BUDGET` | `25` | The most related test targets that run at stop. More are not run, and the agent gets the notice `N related tests selected; above the light budget of M, run them yourself`. `0` runs no tests at all. |
| `HOOKAH_MUTATION_MAX_FILES` | `3` | The most changed code files (tests and sources) of a project mutation testing accepts. `0` switches mutation testing off. |
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
