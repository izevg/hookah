//! Node toolchain detection.
//!
//! The project is the directory of the nearest `package.json`; [`package::PackageInfo`] reads it
//! and its workspace root. A tool is *available* only when both hold: its npm package is declared
//! in `dependencies`/`devDependencies` of the package or the workspace root, and its binary
//! resolves in `node_modules/.bin`, walking up to the workspace root. Binaries run directly from
//! `.bin` (never `npx`, which could download), with the project root as the working directory, so
//! each tool finds the project's own configuration.
//!
//! | Check        | Tool       | Preference and command                                                  |
//! |--------------|------------|-------------------------------------------------------------------------|
//! | format       | biome      | `biome format --write --reporter=json --no-errors-on-unmatched {targets}` when `biome.json(c)` is configured |
//! | format       | prettier   | `prettier --write --ignore-unknown {targets}` when biome does not format |
//! | lint         | biome      | `biome lint --reporter=json --no-errors-on-unmatched --max-diagnostics=200 {targets}` when configured, Biome 1.9 or newer |
//! | lint         | eslint     | `eslint --format json {targets}` when a flat or legacy config exists    |
//! | typecheck    | tsc        | `tsc --noEmit --pretty false -p <tsconfig> --incremental --tsBuildInfoFile <cache>/tsc.tsbuildinfo`, or `tsc -b --pretty false <tsconfig>` with project references |
//! | architecture | depcruise  | `depcruise --config <cfg> --output-type json {targets}` when a `.dependency-cruiser.*` config exists |
//! | test         | vitest     | `vitest related --run --reporter=json --outputFile=<cache>/vitest-report.json --passWithNoTests {targets}` |
//! | test         | jest       | `jest --ci --json --no-watchman --passWithNoTests --findRelatedTests {targets}` |
//! | mutation     | stryker    | `stryker run --incremental --reporters json --mutate {targets}` (one comma-joined target) |
//!
//! Both biome and eslint lint when both are configured. Of vitest and jest, vitest runs unless
//! `scripts.test` mentions jest and not vitest. Only mutation testing reports a missing tool, and
//! only when a test runner exists. Nothing is ever installed.

pub mod package;
pub mod tsconfig;

use std::path::{Path, PathBuf};
use std::time::Duration;

use hookah_domain::toolchain::{Arg, CheckKind, CommandSpec, MissingTool, Tool, Toolchain};

use package::PackageInfo;

/// Name of the Biome tool.
pub const BIOME: &str = "biome";
/// Name of the Prettier tool.
pub const PRETTIER: &str = "prettier";
/// Name of the ESLint tool.
pub const ESLINT: &str = "eslint";
/// Name of the TypeScript compiler tool.
pub const TSC: &str = "tsc";
/// Name of the dependency-cruiser tool.
pub const DEPCRUISE: &str = "depcruise";
/// Name of the Vitest tool.
pub const VITEST: &str = "vitest";
/// Name of the Jest tool.
pub const JEST: &str = "jest";
/// Name of the Stryker tool.
pub const STRYKER: &str = "stryker";

/// File name of the TypeScript configuration, in the project root.
pub const TSCONFIG: &str = "tsconfig.json";
/// File name of the incremental compiler state inside the project cache directory.
pub const TSBUILDINFO: &str = "tsc.tsbuildinfo";
/// File name of the Vitest report inside the project cache directory.
pub const VITEST_REPORT: &str = "vitest-report.json";

const BIOME_CONFIGS: &[&str] = &["biome.json", "biome.jsonc"];
const ESLINT_CONFIGS: &[&str] = &[
    "eslint.config.js",
    "eslint.config.mjs",
    "eslint.config.cjs",
    "eslint.config.ts",
    "eslint.config.mts",
    "eslint.config.cts",
    ".eslintrc",
    ".eslintrc.js",
    ".eslintrc.cjs",
    ".eslintrc.json",
    ".eslintrc.yaml",
    ".eslintrc.yml",
];
const DEPCRUISE_CONFIGS: &[&str] = &[
    ".dependency-cruiser.js",
    ".dependency-cruiser.cjs",
    ".dependency-cruiser.mjs",
    ".dependency-cruiser.json",
];

const FORMAT_TIMEOUT: Duration = Duration::from_secs(30);
const LINT_TIMEOUT: Duration = Duration::from_secs(60);
const TSC_TIMEOUT: Duration = Duration::from_secs(120);
const DEPCRUISE_TIMEOUT: Duration = Duration::from_secs(60);
const TEST_TIMEOUT: Duration = Duration::from_secs(300);
const STRYKER_TIMEOUT: Duration = Duration::from_secs(480);

/// A declared tool whose binary resolves.
struct Installed {
    bin: PathBuf,
    version: Option<String>,
}

fn installed(info: &PackageInfo, package: &str, bin: &str) -> Option<Installed> {
    if !info.declares(package) {
        return None;
    }
    Some(Installed {
        bin: info.bin(bin)?,
        version: info.installed_version(package),
    })
}

/// `(major, minor)` of a version string such as `2.6.8` or `1.9.0-beta.1`.
fn major_minor(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.split(|c: char| !c.is_ascii_digit()).next()?;
    Some((major, minor.parse().ok()?))
}

/// Whether this Biome has the `json` reporter (1.9 and newer). An unknown version is assumed to.
fn biome_has_json_reporter(version: Option<&str>) -> bool {
    version
        .and_then(major_minor)
        .is_none_or(|(major, minor)| major >= 2 || (major == 1 && minor >= 9))
}

fn lit(s: &str) -> Arg {
    Arg::Literal(s.into())
}

fn tool(
    name: &str,
    kind: CheckKind,
    bin: &Installed,
    args: Vec<Arg>,
    cwd: &Path,
    secs: Duration,
) -> Tool {
    Tool {
        name: name.into(),
        kind,
        command: CommandSpec::new(bin.bin.to_string_lossy(), args, cwd, secs),
    }
}

/// Which test runner a project uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runner {
    /// Vitest.
    Vitest,
    /// Jest.
    Jest,
}

impl Runner {
    fn tool_name(self) -> &'static str {
        match self {
            Self::Vitest => VITEST,
            Self::Jest => JEST,
        }
    }

    fn stryker_plugin(self) -> &'static str {
        match self {
            Self::Vitest => "@stryker-mutator/vitest-runner",
            Self::Jest => "@stryker-mutator/jest-runner",
        }
    }
}

fn runner_candidates(info: &PackageInfo) -> [Runner; 2] {
    let script = info.test_script.as_deref().unwrap_or("");
    if script.contains("jest") && !script.contains("vitest") {
        [Runner::Jest, Runner::Vitest]
    } else {
        [Runner::Vitest, Runner::Jest]
    }
}

fn find_runner(info: &PackageInfo) -> Option<(Runner, Installed)> {
    runner_candidates(info)
        .into_iter()
        .find_map(|r| Some((r, installed(info, r.tool_name(), r.tool_name())?)))
}

/// Whether the project has a test runner, that is, whether tests can be selected for it.
pub fn has_test_runner(info: &PackageInfo) -> bool {
    find_runner(info).is_some()
}

/// Builds the toolchain of the package. `cache` yields the project cache directory when a tool
/// wants to keep state there; without it the tools run without incremental state.
pub fn toolchain(info: &PackageInfo, cache: impl Fn() -> Option<PathBuf>) -> Toolchain {
    let root = info.root.as_path();
    let mut tools = Vec::new();

    let biome = installed(info, "@biomejs/biome", BIOME)
        .filter(|_| info.config_file(BIOME_CONFIGS).is_some());
    if let Some(b) = &biome {
        tools.push(tool(
            BIOME,
            CheckKind::Format,
            b,
            [
                "format",
                "--write",
                "--reporter=json",
                "--no-errors-on-unmatched",
            ]
            .iter()
            .map(|a| lit(a))
            .chain([Arg::Targets])
            .collect(),
            root,
            FORMAT_TIMEOUT,
        ));
    } else if let Some(p) = installed(info, "prettier", PRETTIER) {
        tools.push(tool(
            PRETTIER,
            CheckKind::Format,
            &p,
            vec![lit("--write"), lit("--ignore-unknown"), Arg::Targets],
            root,
            FORMAT_TIMEOUT,
        ));
    }

    if let Some(b) = biome
        .as_ref()
        .filter(|b| biome_has_json_reporter(b.version.as_deref()))
    {
        tools.push(tool(
            BIOME,
            CheckKind::Lint,
            b,
            [
                "lint",
                "--reporter=json",
                "--no-errors-on-unmatched",
                "--max-diagnostics=200",
            ]
            .iter()
            .map(|a| lit(a))
            .chain([Arg::Targets])
            .collect(),
            root,
            LINT_TIMEOUT,
        ));
    }
    if let Some(e) = installed(info, "eslint", ESLINT).filter(|_| eslint_configured(info)) {
        tools.push(tool(
            ESLINT,
            CheckKind::Lint,
            &e,
            vec![lit("--format"), lit("json"), Arg::Targets],
            root,
            LINT_TIMEOUT,
        ));
    }

    if let Some(t) = installed(info, "typescript", TSC)
        && let Some(args) = tsc_args(info, &cache)
    {
        tools.push(tool(TSC, CheckKind::Typecheck, &t, args, root, TSC_TIMEOUT));
    }

    if let Some(d) = installed(info, "dependency-cruiser", DEPCRUISE)
        && let Some(config) = info.config_file(DEPCRUISE_CONFIGS)
    {
        tools.push(tool(
            DEPCRUISE,
            CheckKind::Architecture,
            &d,
            vec![
                lit("--config"),
                lit(&config.to_string_lossy()),
                lit("--output-type"),
                lit("json"),
                Arg::Targets,
            ],
            root,
            DEPCRUISE_TIMEOUT,
        ));
    }

    let runner = find_runner(info);
    if let Some((runner, bin)) = &runner {
        tools.push(match runner {
            Runner::Vitest => {
                let mut args = vec![lit("related"), lit("--run"), lit("--reporter=json")];
                if let Some(dir) = cache() {
                    args.push(lit(&format!(
                        "--outputFile={}",
                        dir.join(VITEST_REPORT).to_string_lossy()
                    )));
                }
                args.extend([lit("--passWithNoTests"), Arg::Targets]);
                tool(VITEST, CheckKind::Test, bin, args, root, TEST_TIMEOUT)
            }
            Runner::Jest => tool(
                JEST,
                CheckKind::Test,
                bin,
                [
                    "--ci",
                    "--json",
                    "--no-watchman",
                    "--passWithNoTests",
                    "--findRelatedTests",
                ]
                .iter()
                .map(|a| lit(a))
                .chain([Arg::Targets])
                .collect(),
                root,
                TEST_TIMEOUT,
            ),
        });
    }

    let mut missing = Vec::new();
    if let Some((runner, _)) = runner {
        match installed(info, "@stryker-mutator/core", STRYKER) {
            Some(s) => tools.push(tool(
                STRYKER,
                CheckKind::Mutation,
                &s,
                ["run", "--incremental", "--reporters", "json", "--mutate"]
                    .iter()
                    .map(|a| lit(a))
                    .chain([Arg::Targets])
                    .collect(),
                root,
                STRYKER_TIMEOUT,
            )),
            None => missing.push(MissingTool::new(
                STRYKER,
                CheckKind::Mutation,
                stryker_hint(info, runner),
            )),
        }
    }
    Toolchain::new(tools).with_missing(missing)
}

fn eslint_configured(info: &PackageInfo) -> bool {
    info.config_file(ESLINT_CONFIGS).is_some()
        || std::fs::read_to_string(info.root.join("package.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .is_some_and(|m| m.get("eslintConfig").is_some())
}

/// The `tsc` arguments for the project, or `None` without a `tsconfig.json`.
///
/// Project references need `tsc -b`: a plain `-p` run fails with TS6305 for every import of a
/// referenced project until that project has been built, and checks nothing at all for a
/// solution-style `files: []` root. Build mode is incremental by itself and takes no
/// `--noEmit` (TypeScript rejects it for referenced projects, TS6310), so it writes the
/// declared build outputs of the projects, as the project's own `tsc -b` would. Everything else
/// runs `--noEmit` with incremental state kept in the cache directory.
fn tsc_args(info: &PackageInfo, cache: &impl Fn() -> Option<PathBuf>) -> Option<Vec<Arg>> {
    let text = std::fs::read_to_string(info.root.join(TSCONFIG)).ok()?;
    if tsconfig::has_references(&text) {
        return Some(vec![
            lit("-b"),
            lit("--pretty"),
            lit("false"),
            lit(TSCONFIG),
        ]);
    }
    let mut args: Vec<Arg> = ["--noEmit", "--pretty", "false", "-p", TSCONFIG]
        .iter()
        .map(|a| lit(a))
        .collect();
    if let Some(dir) = cache() {
        args.extend([
            lit("--incremental"),
            lit("--tsBuildInfoFile"),
            lit(&dir.join(TSBUILDINFO).to_string_lossy()),
        ]);
    }
    Some(args)
}

fn stryker_hint(info: &PackageInfo, runner: Runner) -> String {
    if info.declares("@stryker-mutator/core") {
        format!(
            "@stryker-mutator/core is declared but not installed; run `{}`",
            info.manager.install()
        )
    } else {
        format!(
            "{} @stryker-mutator/core {}",
            info.manager.add_dev(),
            runner.stryker_plugin()
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::package::scratch::Scratch;
    use super::*;

    fn cache() -> Option<PathBuf> {
        Some("/cache".into())
    }

    fn detect(s: &Scratch) -> Toolchain {
        toolchain(&PackageInfo::read(&s.0).unwrap(), cache)
    }

    fn argv(tc: &Toolchain, kind: CheckKind, name: &str) -> Vec<String> {
        tc.of_kind(kind)
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("no {name} tool"))
            .command
            .expand_args(&["T".into()])
    }

    /// A package with `deps` declared and installed with their `.bin` entries.
    fn project(deps: &[(&str, &str, &str)]) -> Scratch {
        let s = Scratch::new();
        let list: Vec<String> = deps
            .iter()
            .map(|(n, _, _)| format!(r#""{n}":"*""#))
            .collect();
        s.write(
            "package.json",
            &format!(r#"{{"devDependencies":{{{}}}}}"#, list.join(",")),
        );
        for (name, version, bin) in deps {
            s.install("", name, version, Some(bin));
        }
        s
    }

    #[test]
    fn declared_but_not_installed_and_installed_but_not_declared_are_unavailable() {
        let s = Scratch::new();
        s.write("package.json", r#"{"devDependencies":{"prettier":"3"}}"#);
        assert!(detect(&s).tools().is_empty());
        let s = Scratch::new();
        s.write("package.json", "{}");
        s.install("", "prettier", "3.0.0", Some("prettier"));
        assert!(detect(&s).tools().is_empty());
    }

    #[test]
    fn prettier_formats_without_a_config() {
        let s = project(&[("prettier", "3.3.0", "prettier")]);
        let tc = detect(&s);
        assert_eq!(
            argv(&tc, CheckKind::Format, PRETTIER),
            ["--write", "--ignore-unknown", "T"]
        );
        let spec = &tc.tools()[0].command;
        assert_eq!(
            spec.program,
            s.path("node_modules/.bin/prettier").to_string_lossy()
        );
        assert_eq!(spec.cwd, s.0);
        assert_eq!(spec.timeout, Duration::from_secs(30));
    }

    #[test]
    fn biome_needs_its_config_and_replaces_prettier_for_format() {
        let s = project(&[
            ("@biomejs/biome", "2.6.8", "biome"),
            ("prettier", "3.0.0", "prettier"),
        ]);
        let names = |tc: &Toolchain| {
            tc.tools()
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&detect(&s)), ["prettier"]);
        s.write("biome.jsonc", "{}");
        let tc = detect(&s);
        assert_eq!(names(&tc), ["biome", "biome"]);
        assert_eq!(
            argv(&tc, CheckKind::Format, BIOME),
            [
                "format",
                "--write",
                "--reporter=json",
                "--no-errors-on-unmatched",
                "T"
            ]
        );
        assert_eq!(
            argv(&tc, CheckKind::Lint, BIOME),
            [
                "lint",
                "--reporter=json",
                "--no-errors-on-unmatched",
                "--max-diagnostics=200",
                "T"
            ]
        );
    }

    #[test]
    fn old_biome_formats_but_does_not_lint() {
        let s = project(&[("@biomejs/biome", "1.8.3", "biome")]);
        s.write("biome.json", "{}");
        let tc = detect(&s);
        assert_eq!(tc.of_kind(CheckKind::Format).count(), 1);
        assert_eq!(tc.of_kind(CheckKind::Lint).count(), 0);
        assert!(biome_has_json_reporter(Some("1.9.0")));
        assert!(biome_has_json_reporter(Some("2.0.0-beta.1")));
        assert!(biome_has_json_reporter(None));
        assert!(!biome_has_json_reporter(Some("1.8.3")));
    }

    #[test]
    fn eslint_needs_a_config_and_runs_alongside_biome() {
        let s = project(&[
            ("eslint", "9.0.0", "eslint"),
            ("@biomejs/biome", "2.0.0", "biome"),
        ]);
        s.write("biome.json", "{}");
        assert_eq!(detect(&s).of_kind(CheckKind::Lint).count(), 1);
        s.write("eslint.config.mjs", "export default [];");
        let tc = detect(&s);
        let lint: Vec<_> = tc
            .of_kind(CheckKind::Lint)
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(lint, ["biome", "eslint"]);
        assert_eq!(
            argv(&tc, CheckKind::Lint, ESLINT),
            ["--format", "json", "T"]
        );
    }

    #[test]
    fn eslint_config_in_package_json_or_workspace_root_counts() {
        let s = Scratch::new();
        s.write(
            "package.json",
            r#"{"devDependencies":{"eslint":"8"},"eslintConfig":{"root":true}}"#,
        );
        s.install("", "eslint", "8.57.0", Some("eslint"));
        assert_eq!(detect(&s).of_kind(CheckKind::Lint).count(), 1);

        let s = Scratch::new();
        s.write("pnpm-workspace.yaml", "");
        s.write("package.json", r#"{"devDependencies":{"eslint":"9"}}"#);
        s.write("eslint.config.js", "");
        s.install("", "eslint", "9.0.0", Some("eslint"));
        s.write("packages/a/package.json", "{}");
        let info = PackageInfo::read(&s.path("packages/a")).unwrap();
        assert_eq!(toolchain(&info, cache).of_kind(CheckKind::Lint).count(), 1);
    }

    #[test]
    fn tsc_is_incremental_in_the_cache_dir() {
        let s = project(&[("typescript", "5.6.2", "tsc")]);
        assert!(detect(&s).tools().is_empty(), "no tsconfig, no tsc");
        s.write(
            "tsconfig.json",
            r#"{ "compilerOptions": { "strict": true, }, }"#,
        );
        let tc = detect(&s);
        assert_eq!(
            argv(&tc, CheckKind::Typecheck, TSC),
            [
                "--noEmit",
                "--pretty",
                "false",
                "-p",
                "tsconfig.json",
                "--incremental",
                "--tsBuildInfoFile",
                "/cache/tsc.tsbuildinfo"
            ]
        );
        assert_eq!(tc.tools()[0].command.timeout, Duration::from_secs(120));
        // Without a cache directory the incremental flags are dropped, not the check.
        let info = PackageInfo::read(&s.0).unwrap();
        let tc = toolchain(&info, || None);
        assert_eq!(
            argv(&tc, CheckKind::Typecheck, TSC),
            ["--noEmit", "--pretty", "false", "-p", "tsconfig.json"]
        );
    }

    #[test]
    fn tsc_uses_build_mode_with_project_references() {
        let s = project(&[("typescript", "5.6.2", "tsc")]);
        s.write(
            "tsconfig.json",
            r#"{ // solution style
              "files": [], "references": [{ "path": "./packages/lib" }] }"#,
        );
        assert_eq!(
            argv(&detect(&s), CheckKind::Typecheck, TSC),
            ["-b", "--pretty", "false", "tsconfig.json"]
        );
    }

    #[test]
    fn depcruise_needs_a_config_and_gets_it_by_absolute_path() {
        let s = project(&[("dependency-cruiser", "16.0.0", "depcruise")]);
        assert!(detect(&s).tools().is_empty());
        s.write(".dependency-cruiser.cjs", "module.exports = {};");
        let tc = detect(&s);
        assert_eq!(
            argv(&tc, CheckKind::Architecture, DEPCRUISE),
            [
                "--config",
                &s.path(".dependency-cruiser.cjs").to_string_lossy(),
                "--output-type",
                "json",
                "T"
            ]
        );
        assert_eq!(tc.tools()[0].command.timeout, Duration::from_secs(60));
    }

    #[test]
    fn vitest_writes_its_report_into_the_cache_dir() {
        let s = project(&[("vitest", "2.1.0", "vitest")]);
        let tc = detect(&s);
        assert_eq!(
            argv(&tc, CheckKind::Test, VITEST),
            [
                "related",
                "--run",
                "--reporter=json",
                "--outputFile=/cache/vitest-report.json",
                "--passWithNoTests",
                "T"
            ]
        );
        assert_eq!(tc.tools()[0].command.timeout, Duration::from_secs(300));
    }

    #[test]
    fn jest_runs_in_ci_mode_without_watchman() {
        let s = project(&[("jest", "29.7.0", "jest")]);
        assert_eq!(
            argv(&detect(&s), CheckKind::Test, JEST),
            [
                "--ci",
                "--json",
                "--no-watchman",
                "--passWithNoTests",
                "--findRelatedTests",
                "T"
            ]
        );
    }

    #[test]
    fn the_test_script_breaks_the_tie_between_runners() {
        let s = project(&[("vitest", "2.0.0", "vitest"), ("jest", "29.0.0", "jest")]);
        let name = |s: &Scratch| {
            detect(s)
                .of_kind(CheckKind::Test)
                .next()
                .unwrap()
                .name
                .clone()
        };
        assert_eq!(name(&s), "vitest");
        s.write(
            "package.json",
            r#"{"scripts":{"test":"jest --ci"},"devDependencies":{"vitest":"2","jest":"29"}}"#,
        );
        assert_eq!(name(&s), "jest");
    }

    #[test]
    fn stryker_runs_incrementally_with_a_json_report() {
        let s = project(&[
            ("vitest", "2.0.0", "vitest"),
            ("@stryker-mutator/core", "9.0.0", "stryker"),
        ]);
        let tc = detect(&s);
        assert!(tc.missing().is_empty());
        assert_eq!(
            argv(&tc, CheckKind::Mutation, STRYKER),
            [
                "run",
                "--incremental",
                "--reporters",
                "json",
                "--mutate",
                "T"
            ]
        );
        let spec = &tc.of_kind(CheckKind::Mutation).next().unwrap().command;
        assert_eq!(spec.timeout, Duration::from_secs(480));
    }

    #[test]
    fn missing_stryker_is_reported_only_with_a_test_runner_and_matches_the_manager() {
        let s = project(&[("typescript", "5.0.0", "tsc")]);
        assert!(detect(&s).missing().is_empty());

        let s = project(&[("jest", "29.0.0", "jest")]);
        s.write("pnpm-lock.yaml", "");
        let tc = detect(&s);
        assert_eq!(
            tc.missing(),
            [MissingTool::new(
                "stryker",
                CheckKind::Mutation,
                "pnpm add -D @stryker-mutator/core @stryker-mutator/jest-runner"
            )]
        );

        let s = project(&[("vitest", "2.0.0", "vitest")]);
        s.write("yarn.lock", "");
        assert_eq!(
            detect(&s).missing()[0].hint,
            "yarn add -D @stryker-mutator/core @stryker-mutator/vitest-runner"
        );
    }

    #[test]
    fn declared_but_uninstalled_stryker_hints_at_install() {
        let s = project(&[("vitest", "2.0.0", "vitest")]);
        s.write(
            "package.json",
            r#"{"devDependencies":{"vitest":"2","@stryker-mutator/core":"9"}}"#,
        );
        let tc = detect(&s);
        assert!(tc.missing()[0].hint.contains("run `npm install`"));
        assert!(has_test_runner(&PackageInfo::read(&s.0).unwrap()));
    }

    #[test]
    fn no_installed_runner_means_no_test_runner() {
        let s = project(&[("eslint", "9.0.0", "eslint")]);
        s.write("package.json", r#"{"devDependencies":{"eslint":"9"}}"#);
        assert!(!has_test_runner(&PackageInfo::read(&s.0).unwrap()));
    }

    #[test]
    fn parses_major_and_minor() {
        assert_eq!(major_minor("2.6.8"), Some((2, 6)));
        assert_eq!(major_minor("1.9.0-beta.1"), Some((1, 9)));
        assert_eq!(major_minor("x"), None);
    }
}
