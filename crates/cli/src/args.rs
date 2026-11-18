//! Command-line interface.
//!
//! - `hookah check <path>...`: run the edit phase on files directly (for humans and CI).
//! - `hookah check --changed`: run the stop phase over everything changed in a project.
//! - `hookah status [--json]`: print the latest recorded outcome of each project under a directory.
//! - `hookah hook <harness>`: read a harness hook payload from stdin and answer in that
//!   harness's protocol.
//! - `hookah detect <path>`: print the detected project and toolchain, for debugging.
//! - `hookah plugin <harness>`: print the plugin source of a plugin-based harness, or with
//!   `--out <dir>` write the plugin's files (Orca).
//! - `hookah baseline --session <id> --root <dir>`: the background job that a session-start hook
//!   spawns (hidden). It records the project's existing findings so later reports show only new
//!   ones.
//! - `--report-limit <N>` (global): cap on findings shown per report.
//!
//! - `hookah setup <harness>` / `hookah setup --detect`: write the hook config of a harness, or
//!   survey which harnesses are installed and configured.
//! - `hookah uninstall <harness>|--all`: take out what `setup` wrote, and nothing else.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use hookah_domain::verdict::DEFAULT_LIMIT;

/// Run format, lint, and related tests on files an AI coding agent edits.
#[derive(Debug, Parser)]
#[command(name = "hookah", version, about)]
pub struct Cli {
    /// Maximum number of findings shown per report.
    #[arg(long, global = true, value_name = "N", default_value_t = DEFAULT_LIMIT)]
    pub report_limit: usize,

    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// The subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the edit phase on files, or with `--changed` the stop phase on all changed files;
    /// exits 1 if any check fails.
    Check {
        /// Files to check.
        #[arg(required_unless_present = "changed", conflicts_with = "changed")]
        paths: Vec<PathBuf>,

        /// Check everything changed since the last commit in the project at `--root`, the way the
        /// stop hook does: static checks, then related tests and mutation testing.
        #[arg(long)]
        changed: bool,

        /// The directory to check with `--changed`; ignored without it. Defaults to the current
        /// directory.
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
    },
    /// Print the latest check outcome recorded for the project at a directory and for the projects
    /// nested below it. Reads what the hooks recorded; runs no checks.
    Status {
        /// The directory to look at. Defaults to the current directory.
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,

        /// Print one JSON document instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Answer an agent harness hook: payload on stdin, response on stdout/stderr.
    Hook {
        /// The harness that sent the payload.
        harness: HarnessKind,
    },
    /// Print the plugin source for a harness that loads plugins, to redirect into its plugin
    /// directory (for example `hookah plugin opencode > .opencode/plugins/hookah.ts`), or write
    /// a multi-file plugin with `--out` (`hookah plugin orca --out ./hookah-orca-plugin`).
    Plugin {
        /// The harness the plugin is for.
        harness: PluginKind,

        /// Write the plugin's files into this directory instead of printing. Required for Orca,
        /// whose plugin has several files.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
    /// Write or merge the hook config of an agent harness, or list the installed ones.
    ///
    /// Non-interactive and idempotent: existing settings are kept, hookah's entries are added or
    /// updated in place, and the original file is backed up once to `<file>.hookah.bak`.
    Setup(SetupArgs),
    /// Remove what `hookah setup` wrote: hookah's hook entries or plugin files for one harness,
    /// or for every configured harness with `--all`.
    ///
    /// The exact inverse of setup. Only hookah's own entries leave a config file, a plugin file
    /// leaves only while it still has the content hookah wrote, and the `.hookah.bak` backups are
    /// left alone. Running it again changes nothing.
    Uninstall(UninstallArgs),
    /// Compute the finding baseline of a project for an agent session. Spawned by `hookah hook`
    /// on a session start; not meant to be run by hand.
    #[command(hide = true)]
    Baseline {
        /// The agent session id.
        #[arg(long)]
        session: String,
        /// The project root.
        #[arg(long)]
        root: PathBuf,
    },
    /// Print the config files in effect for a path and the tool environment they set, with the
    /// file each variable comes from. Use it to check config instead of editing a hook command.
    Config {
        /// File or directory to inspect; defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Print the project and toolchain found for a path.
    Detect {
        /// File or directory to inspect.
        path: PathBuf,
    },
}

/// Arguments of `hookah setup`.
#[derive(Debug, Args)]
pub struct SetupArgs {
    /// The harness to configure.
    #[arg(required_unless_present = "detect", conflicts_with = "detect")]
    pub harness: Option<HarnessKind>,

    /// List installed harnesses and which are already configured. Read-only.
    #[arg(long)]
    pub detect: bool,

    /// Which config to edit: the project's or the user's. Defaults to `project`, or `user` for
    /// Kimi Code, which has no project config.
    #[arg(long, value_enum, conflicts_with = "detect")]
    pub scope: Option<ScopeKind>,

    /// Show the file that would be written, and write nothing.
    #[arg(long, conflicts_with_all = ["detect", "print"])]
    pub dry_run: bool,

    /// Print only the config snippet, and write nothing.
    #[arg(long, conflicts_with = "detect")]
    pub print: bool,

    /// The command that starts hookah in each hook, for example `mise exec -- hookah`. Defaults to
    /// the absolute path of this binary.
    #[arg(long, value_name = "CMD", conflicts_with = "detect")]
    pub command: Option<String>,
}

/// Arguments of `hookah uninstall`. Shaped like `hookah setup`: the harness is the positional
/// argument and `--scope` and `--dry-run` mean the same, so the one command undoes the other.
#[derive(Debug, Args)]
pub struct UninstallArgs {
    /// The harness to remove hookah from. `orca` is the Orca plugin folder (see `--dir`).
    #[arg(
        required_unless_present_any = ["all", "purge_cache"],
        conflicts_with = "all"
    )]
    pub harness: Option<UninstallKind>,

    /// Remove hookah from every harness that `hookah setup --detect` reports as configured, at
    /// `--scope` (both scopes when none is given). Does not cover the Orca plugin.
    #[arg(long, conflicts_with = "dir")]
    pub all: bool,

    /// Which config to edit: the project's or the user's. Defaults like setup: `project`, or
    /// `user` for the harnesses that have no project config.
    #[arg(long, value_enum)]
    pub scope: Option<ScopeKind>,

    /// With `orca`: the folder `hookah plugin orca --out` wrote. Defaults to
    /// `~/.hookah-orca-plugin`.
    #[arg(long, value_name = "DIR")]
    pub dir: Option<PathBuf>,

    /// Show what would be removed, and change nothing.
    #[arg(long)]
    pub dry_run: bool,

    /// Also delete hookah's cache directory (baselines, status, tool caches).
    #[arg(long)]
    pub purge_cache: bool,

    /// Delete a plugin file even when its content is not what hookah writes.
    #[arg(long)]
    pub force: bool,
}

/// What `hookah uninstall` takes a harness name for: the setup harnesses, and Orca.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum UninstallKind {
    /// Claude Code.
    Claude,
    /// Codex CLI.
    Codex,
    /// Qoder CLI.
    Qoder,
    /// Kimi Code.
    Kimi,
    /// Cursor.
    Cursor,
    /// opencode.
    Opencode,
    /// Hermes Agent.
    Hermes,
    /// Pi coding agent.
    Pi,
    /// OpenClaw.
    Openclaw,
    /// The Orca plugin folder written by `hookah plugin orca --out`.
    Orca,
}

impl UninstallKind {
    /// The setup harness this names, `None` for Orca.
    pub fn harness(self) -> Option<HarnessKind> {
        Some(match self {
            Self::Claude => HarnessKind::Claude,
            Self::Codex => HarnessKind::Codex,
            Self::Qoder => HarnessKind::Qoder,
            Self::Kimi => HarnessKind::Kimi,
            Self::Cursor => HarnessKind::Cursor,
            Self::Opencode => HarnessKind::Opencode,
            Self::Hermes => HarnessKind::Hermes,
            Self::Pi => HarnessKind::Pi,
            Self::Openclaw => HarnessKind::Openclaw,
            Self::Orca => return None,
        })
    }
}

/// Where a harness config lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ScopeKind {
    /// The current project's config.
    Project,
    /// The user's config, for every project.
    User,
}

/// Supported agent harnesses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum HarnessKind {
    /// Claude Code command hooks.
    Claude,
    /// Codex CLI command hooks.
    Codex,
    /// Qoder CLI command hooks.
    Qoder,
    /// Kimi Code command hooks.
    Kimi,
    /// Cursor (the IDE agent and `cursor-agent`) command hooks.
    Cursor,
    /// opencode, through the bundled plugin (`hookah plugin opencode`).
    Opencode,
    /// Hermes Agent, through the bundled plugin (`hookah setup hermes`).
    Hermes,
    /// Pi coding agent, through the bundled extension (`hookah plugin pi`).
    Pi,
    /// OpenClaw, through the bundled plugin (`hookah plugin openclaw`).
    Openclaw,
}

/// Harnesses that ship a plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PluginKind {
    /// The opencode TypeScript plugin.
    Opencode,
    /// The Orca plugin: a folder with a manifest and TypeScript sources.
    Orca,
    /// The Hermes Agent Python plugin (its `__init__.py`).
    Hermes,
    /// The Pi TypeScript extension.
    Pi,
    /// The OpenClaw JavaScript plugin (its entry point; `hookah setup openclaw` also writes
    /// the manifest).
    Openclaw,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn version_flag_prints_the_cargo_version() {
        let err = Cli::try_parse_from(["hookah", "--version"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
        assert_eq!(
            err.to_string().trim(),
            format!("hookah {}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn parses_check_with_global_limit() {
        let cli = Cli::try_parse_from(["hookah", "check", "a.rs", "b.rs", "--report-limit", "3"])
            .unwrap();
        assert_eq!(cli.report_limit, 3);
        assert!(matches!(cli.command, Command::Check { paths, .. } if paths.len() == 2));
    }

    #[test]
    fn limit_defaults_to_domain_default() {
        let cli = Cli::try_parse_from(["hookah", "hook", "claude"]).unwrap();
        assert_eq!(cli.report_limit, DEFAULT_LIMIT);
        assert!(matches!(
            cli.command,
            Command::Hook {
                harness: HarnessKind::Claude
            }
        ));
    }

    #[test]
    fn parses_plugin_and_hook_for_hermes() {
        let cli = Cli::try_parse_from(["hookah", "plugin", "hermes"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Plugin {
                harness: PluginKind::Hermes,
                out: None
            }
        ));
        let cli = Cli::try_parse_from(["hookah", "hook", "hermes"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Hook {
                harness: HarnessKind::Hermes
            }
        ));
    }

    #[test]
    fn parses_plugin_and_opencode_hook() {
        let cli = Cli::try_parse_from(["hookah", "plugin", "opencode"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Plugin {
                harness: PluginKind::Opencode,
                out: None
            }
        ));
        let cli = Cli::try_parse_from(["hookah", "plugin", "openclaw"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Plugin {
                harness: PluginKind::Openclaw,
                out: None
            }
        ));
        let cli = Cli::try_parse_from(["hookah", "hook", "openclaw"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Hook {
                harness: HarnessKind::Openclaw
            }
        ));
        let cli = Cli::try_parse_from(["hookah", "hook", "opencode"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Hook {
                harness: HarnessKind::Opencode
            }
        ));
    }

    #[test]
    fn parses_plugin_and_hook_for_pi() {
        let cli = Cli::try_parse_from(["hookah", "plugin", "pi"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Plugin {
                harness: PluginKind::Pi,
                out: None
            }
        ));
        let cli = Cli::try_parse_from(["hookah", "hook", "pi"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Hook {
                harness: HarnessKind::Pi
            }
        ));
    }

    #[test]
    fn parses_the_hidden_baseline_job() {
        let cli = Cli::try_parse_from(["hookah", "baseline", "--session=-odd id", "--root", "/p"])
            .unwrap();
        match cli.command {
            Command::Baseline { session, root } => {
                assert_eq!(session, "-odd id");
                assert_eq!(root, PathBuf::from("/p"));
            }
            other => panic!("expected baseline, got {other:?}"),
        }
    }

    #[test]
    fn parses_setup_forms() {
        let cli = Cli::try_parse_from([
            "hookah",
            "setup",
            "kimi",
            "--scope",
            "user",
            "--dry-run",
            "--command",
            "mise exec -- hookah",
        ])
        .unwrap();
        match cli.command {
            Command::Setup(a) => {
                assert_eq!(a.harness, Some(HarnessKind::Kimi));
                assert_eq!(a.scope, Some(ScopeKind::User));
                assert!(a.dry_run && !a.print && !a.detect);
                assert_eq!(a.command.as_deref(), Some("mise exec -- hookah"));
            }
            other => panic!("expected setup, got {other:?}"),
        }
        let cli = Cli::try_parse_from(["hookah", "setup", "--detect"]).unwrap();
        assert!(matches!(cli.command, Command::Setup(a) if a.detect && a.harness.is_none()));
    }

    #[test]
    fn rejects_inconsistent_setup_flags() {
        for bad in [
            &["hookah", "setup"][..],
            &["hookah", "setup", "claude", "--detect"],
            &["hookah", "setup", "--detect", "--dry-run"],
            &["hookah", "setup", "claude", "--dry-run", "--print"],
            &["hookah", "setup", "nope"],
        ] {
            assert!(Cli::try_parse_from(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn check_needs_a_path_or_changed() {
        assert!(Cli::try_parse_from(["hookah", "check"]).is_err());
        assert!(Cli::try_parse_from(["hookah", "check", "a.rs", "--changed"]).is_err());
        let cli = Cli::try_parse_from(["hookah", "check", "--changed", "--root", "d"]).unwrap();
        match cli.command {
            Command::Check {
                paths,
                changed,
                root,
            } => {
                assert!(paths.is_empty() && changed);
                assert_eq!(root, Some(PathBuf::from("d")));
            }
            other => panic!("expected check, got {other:?}"),
        }
    }

    #[test]
    fn parses_status_and_the_orca_plugin() {
        let cli = Cli::try_parse_from(["hookah", "status", "--json", "--root", "w"]).unwrap();
        match cli.command {
            Command::Status { root, json } => {
                assert!(json);
                assert_eq!(root, Some(PathBuf::from("w")));
            }
            other => panic!("expected status, got {other:?}"),
        }
        let cli = Cli::try_parse_from(["hookah", "plugin", "orca", "--out", "o"]).unwrap();
        match cli.command {
            Command::Plugin { harness, out } => {
                assert_eq!(harness, PluginKind::Orca);
                assert_eq!(out, Some(PathBuf::from("o")));
            }
            other => panic!("expected plugin, got {other:?}"),
        }
    }

    #[test]
    fn parses_uninstall_forms() {
        let cli = Cli::try_parse_from([
            "hookah",
            "uninstall",
            "kimi",
            "--scope",
            "user",
            "--dry-run",
            "--purge-cache",
            "--force",
        ])
        .unwrap();
        match cli.command {
            Command::Uninstall(a) => {
                assert_eq!(a.harness, Some(UninstallKind::Kimi));
                assert_eq!(a.scope, Some(ScopeKind::User));
                assert!(a.dry_run && a.purge_cache && a.force && !a.all);
            }
            other => panic!("expected uninstall, got {other:?}"),
        }
        let cli = Cli::try_parse_from(["hookah", "uninstall", "--all"]).unwrap();
        assert!(matches!(cli.command, Command::Uninstall(a) if a.all && a.harness.is_none()));
        let cli = Cli::try_parse_from(["hookah", "uninstall", "--purge-cache"]).unwrap();
        assert!(matches!(cli.command, Command::Uninstall(a) if a.purge_cache));
        let cli = Cli::try_parse_from(["hookah", "uninstall", "orca", "--dir", "d"]).unwrap();
        match cli.command {
            Command::Uninstall(a) => {
                assert_eq!(a.harness, Some(UninstallKind::Orca));
                assert_eq!(a.dir, Some(PathBuf::from("d")));
            }
            other => panic!("expected uninstall, got {other:?}"),
        }
    }

    #[test]
    fn rejects_inconsistent_uninstall_flags() {
        for bad in [
            &["hookah", "uninstall"][..],
            &["hookah", "uninstall", "claude", "--all"],
            &["hookah", "uninstall", "--all", "--dir", "d"],
            &["hookah", "uninstall", "nope"],
        ] {
            assert!(Cli::try_parse_from(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn every_uninstall_kind_but_orca_names_a_setup_harness() {
        for kind in UninstallKind::value_variants() {
            let name = kind.to_possible_value().unwrap().get_name().to_owned();
            match kind.harness() {
                Some(h) => assert_eq!(h.to_possible_value().unwrap().get_name(), name),
                None => assert_eq!(name, "orca"),
            }
        }
    }
}
