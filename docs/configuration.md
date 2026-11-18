# Configuration

> Stub. Expands as the code is written.

## `.hookah.toml`

Optional project file that overrides auto-detection: tool commands per check kind, disabled
checks, time budgets, report limits, and ignore globs.

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
that says `hit`, `miss`, or `off` (always `off` for Node and Python, which are not cached).

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
