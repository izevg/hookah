# Updates

hookah keeps itself current. At the start of an agent session it checks, in the background, whether
the official repository has a newer release, and if so installs it. You do not run anything, nothing
slows a session down, and you can switch all of it off.

**For agents.** Do not update hookah by hand-editing files or downloading binaries yourself. To
update now, run `hookah update`; to only look, `hookah update --check`. To stop automatic updates,
see [Opting out](#opting-out). Do not restart the harness to apply an update: the next session uses
the new binary by itself.

## How it works

1. **Trigger.** Every session start (the `SessionStart` event of each harness, or its equivalent)
   asks hookah to check. hookah does not wait for the network: it starts a detached
   `hookah update --background` and returns at once, exactly like the baseline job.
2. **Throttle.** At most one check per 24 hours (one hour after a failed check). The state lives in
   hookah's cache directory, in `update/state.json` (see [Where things live](uninstall.md#where-things-live)).
   A lock in the same directory keeps sessions that start at the same moment from updating twice.
3. **Look.** The background job asks `https://api.github.com/repos/izevg/hookah/releases/latest` for
   the latest stable release. Drafts and prereleases are never candidates.
4. **Decide.** hookah updates only to a strictly greater version. A development build (version
   `0.0.0`: a build from source, or `cargo install`) never updates itself.
5. **Download and verify.** It picks the archive for its own platform (below), downloads
   `SHA256SUMS` and the archive from the same release, and refuses the archive unless its SHA-256
   equals the listed one.
6. **Check the binary.** The executable is taken out of the archive and run with `--version`. It
   must print exactly the expected version, or nothing is installed.
7. **Replace.** The new executable is written next to the running one and renamed over it. On Linux
   and macOS the rename is atomic. A running hookah keeps working; new sessions use the new binary.
8. **Tell you, once.** The background job has no channel to the agent, so the result is left in the
   state file and the *next* session start shows one short line, then forgets it:

   ```text
   hookah: updated hookah from 0.4.0 to 0.5.0; new sessions use it
   ```

   Claude Code shows it as a notice to you (never to the model); other harnesses print it as a
   session-start warning on stderr, and the plugin-based harnesses stay silent. It never blocks anything.

A 0.x minor bump (0.4 to 0.5) installs like any other. Semver treats a minor bump below 1.0 as
breaking, but hookah's hook protocol stays backward compatible: hooks that `hookah setup` wrote
keep working across versions.

### Which archive

hookah picks the asset of the platform it was built for:

| Built for | Archive |
|-----------|---------|
| Linux x86_64 | `hookah-<version>-x86_64-unknown-linux-musl.tar.gz` |
| Linux arm64 | `hookah-<version>-aarch64-unknown-linux-musl.tar.gz` |
| macOS Intel | `hookah-<version>-x86_64-apple-darwin.tar.gz` |
| macOS Apple Silicon | `hookah-<version>-aarch64-apple-darwin.tar.gz` |
| Windows x86_64 | `hookah-<version>-x86_64-pc-windows-msvc.zip` |
| Windows arm64 | `hookah-<version>-aarch64-pc-windows-msvc.zip` |

Any other platform has no official build: hookah does not update and says so once.

### Windows

A running `.exe` cannot be overwritten but can be renamed. hookah renames the current
`hookah.exe` to `hookah.exe.old`, moves the new one in, and puts the old one back if that fails. The
`.old` file is deleted by a later run of hookah.

### When hookah does not update itself

- It is a development build (`0.0.0`).
- The binary lives under a package manager's directory (`~/.cargo/bin`, Homebrew, mise, Nix): update
  it with that tool. hookah tells you once per new version.
- The directory is not writable: hookah says so once. Run the installer again, or fix the permissions.
- Updates are switched off, or automatic installing is.

Nothing that fails here ever blocks the agent: an update problem is a warning at most. A check that
keeps failing (offline, rate limited) is mentioned once, after its third failure in a row.

## Commands

```text
hookah update            # check now and install a newer release, printing what it does
hookah update --check    # only report whether a newer release exists
```

Both ignore the 24-hour throttle and the `auto` setting (you asked), but not `check = false` or
`HOOKAH_NO_UPDATE`. Exit code 0 means up to date, updated, or (with `--check`) update available; 1
means it could not do it, and the message says why.

## Opting out

| How | Effect |
|-----|--------|
| `HOOKAH_NO_UPDATE=1` | no check, no install, no notice; the manual command refuses too. Any value but empty, `0`, `false`, `no`, `off`. |
| `[update] check = false` | the same, from the [user config](../configuration.md#the-user-config) (`~/.config/hookah/config.toml`) |
| `[update] auto = false` | still checks, but only tells you once per new version (`hookah 0.5.0 is available; run hookah update to install it`); installing is up to you |

```toml
# ~/.config/hookah/config.toml
[update]
check = true   # false: switch every update check and install off
auto = true    # false: check and notify only
```

The environment variable wins over the file. Neither can be set per project: the binary is shared by
every project.

## Security model

- **One source.** Releases of `izevg/hookah` on `api.github.com` and `github.com`, both written into
  the binary. No setting, variable, or config file changes the source. The only override exists in
  debug builds of hookah (it is how the tests use a local server) and is compiled out of every
  release binary.
- **HTTPS only**, including every redirect. Download URLs are built from the release tag and a fixed
  asset name, never taken from the API's answer.
- **The system's certificate store** is trusted, so a corporate TLS-inspecting proxy whose root is
  installed on the machine works. `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, and `NO_PROXY` are
  honored. Connecting gives up after 5 seconds, a whole request after 60.
- **Integrity.** The archive's SHA-256 must equal the entry in the release's `SHA256SUMS`, which is
  downloaded from the same release, before a byte is extracted. Only the executable is read from the
  archive, in memory, never by a path from the archive.
- **No half-updates.** Every step leaves the installed binary untouched when it fails, and the new
  binary must run and report the expected version before it replaces anything.
- **What it cannot give you.** `SHA256SUMS` comes from the same place as the archive, so the checksum
  catches a corrupted or truncated download, not a compromised release. Trust rests on GitHub and the
  repository's release process, as it does for the installer script. If that is not enough for you,
  opt out and install from a source you verify.
- **No tokens, no telemetry.** Requests carry no credentials and no machine identifiers, only a
  `hookah/<version>` user agent.

To remove the state file, see [Uninstall](uninstall.md#where-things-live).
