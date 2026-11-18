//! The stored form of a [`Toolchain`].
//!
//! The domain model carries no serialization. [`ToolchainDto`] mirrors it for the cache file; the
//! conversions are exhaustive, so a new variant fails to compile here instead of being lost.

use std::path::PathBuf;
use std::time::Duration;

use hookah_domain::toolchain::{Arg, CheckKind, CommandSpec, MissingTool, Tool, Toolchain};
use serde::{Deserialize, Serialize};

/// Serializable [`Toolchain`].
#[derive(Debug, Serialize, Deserialize)]
pub struct ToolchainDto {
    tools: Vec<ToolDto>,
    missing: Vec<MissingDto>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ToolDto {
    name: String,
    kind: KindDto,
    program: String,
    args: Vec<ArgDto>,
    cwd: PathBuf,
    timeout: Duration,
}

#[derive(Debug, Serialize, Deserialize)]
struct MissingDto {
    name: String,
    kind: KindDto,
    hint: String,
}

#[derive(Debug, Serialize, Deserialize)]
enum ArgDto {
    Literal(String),
    Targets,
    TargetsOutside(Vec<String>),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
enum KindDto {
    Format,
    Lint,
    Typecheck,
    Architecture,
    Test,
    Mutation,
}

impl From<CheckKind> for KindDto {
    fn from(kind: CheckKind) -> Self {
        match kind {
            CheckKind::Format => Self::Format,
            CheckKind::Lint => Self::Lint,
            CheckKind::Typecheck => Self::Typecheck,
            CheckKind::Architecture => Self::Architecture,
            CheckKind::Test => Self::Test,
            CheckKind::Mutation => Self::Mutation,
        }
    }
}

impl From<KindDto> for CheckKind {
    fn from(kind: KindDto) -> Self {
        match kind {
            KindDto::Format => Self::Format,
            KindDto::Lint => Self::Lint,
            KindDto::Typecheck => Self::Typecheck,
            KindDto::Architecture => Self::Architecture,
            KindDto::Test => Self::Test,
            KindDto::Mutation => Self::Mutation,
        }
    }
}

impl From<&Arg> for ArgDto {
    fn from(arg: &Arg) -> Self {
        match arg {
            Arg::Literal(s) => Self::Literal(s.clone()),
            Arg::Targets => Self::Targets,
            Arg::TargetsOutside(paths) => Self::TargetsOutside(paths.clone()),
        }
    }
}

impl From<ArgDto> for Arg {
    fn from(arg: ArgDto) -> Self {
        match arg {
            ArgDto::Literal(s) => Self::Literal(s),
            ArgDto::Targets => Self::Targets,
            ArgDto::TargetsOutside(paths) => Self::TargetsOutside(paths),
        }
    }
}

impl From<&Toolchain> for ToolchainDto {
    fn from(toolchain: &Toolchain) -> Self {
        Self {
            tools: toolchain
                .tools()
                .iter()
                .map(|t| ToolDto {
                    name: t.name.clone(),
                    kind: t.kind.into(),
                    program: t.command.program.clone(),
                    args: t.command.args.iter().map(ArgDto::from).collect(),
                    cwd: t.command.cwd.clone(),
                    timeout: t.command.timeout,
                })
                .collect(),
            missing: toolchain
                .missing()
                .iter()
                .map(|m| MissingDto {
                    name: m.name.clone(),
                    kind: m.kind.into(),
                    hint: m.hint.clone(),
                })
                .collect(),
        }
    }
}

impl From<ToolchainDto> for Toolchain {
    fn from(dto: ToolchainDto) -> Self {
        let tools = dto
            .tools
            .into_iter()
            .map(|t| Tool {
                name: t.name,
                kind: t.kind.into(),
                command: CommandSpec::new(
                    t.program,
                    t.args.into_iter().map(Arg::from).collect(),
                    t.cwd,
                    t.timeout,
                ),
            })
            .collect();
        let missing = dto
            .missing
            .into_iter()
            .map(|m| MissingTool::new(m.name, m.kind.into(), m.hint))
            .collect();
        Toolchain::new(tools).with_missing(missing)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn tool(name: &str, kind: CheckKind, args: Vec<Arg>) -> Tool {
        Tool {
            name: name.into(),
            kind,
            command: CommandSpec::new("prog", args, "/root", Duration::new(7, 500)),
        }
    }

    #[test]
    fn a_toolchain_survives_a_json_round_trip() {
        let kinds = [
            CheckKind::Format,
            CheckKind::Lint,
            CheckKind::Typecheck,
            CheckKind::Architecture,
            CheckKind::Test,
            CheckKind::Mutation,
        ];
        let tools = kinds
            .iter()
            .map(|k| {
                tool(
                    k.name(),
                    *k,
                    vec![
                        Arg::Literal("--flag".into()),
                        Arg::Targets,
                        Arg::TargetsOutside(vec!["src".into(), "tests".into()]),
                    ],
                )
            })
            .collect();
        let missing = kinds
            .iter()
            .map(|k| MissingTool::new(format!("m-{}", k.name()), *k, "install it"))
            .collect();
        let original = Toolchain::new(tools).with_missing(missing);
        let json = serde_json::to_string(&ToolchainDto::from(&original)).unwrap();
        let back: Toolchain = serde_json::from_str::<ToolchainDto>(&json).unwrap().into();
        assert_eq!(back, original);
    }

    #[test]
    fn an_empty_toolchain_round_trips() {
        let json = serde_json::to_string(&ToolchainDto::from(&Toolchain::default())).unwrap();
        let back: Toolchain = serde_json::from_str::<ToolchainDto>(&json).unwrap().into();
        assert_eq!(back, Toolchain::default());
    }
}
