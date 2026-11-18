//! The `CheckRunner` decorator that applies project config to every tool process.

use std::collections::HashSet;
use std::sync::Mutex;

use hookah_application::error::PortError;
use hookah_application::ports::{CheckRunner, ProcessOutput};
use hookah_domain::toolchain::CommandSpec;

use super::{Host, resolve};

/// Runs commands through `inner` with the variables [`resolve`] finds for the command's working
/// directory. Config wins over the variables the detector put on the command, which win over the
/// inherited environment. Each warning is printed to stderr once per process.
pub struct ProjectEnvRunner<R> {
    inner: R,
    host: Host,
    warned: Mutex<HashSet<String>>,
}

impl<R> ProjectEnvRunner<R> {
    /// Wraps `inner`.
    pub fn new(inner: R, host: Host) -> Self {
        Self {
            inner,
            host,
            warned: Mutex::new(HashSet::new()),
        }
    }

    /// The warnings not shown before; remembers them.
    fn unseen(&self, warnings: &[String]) -> Vec<String> {
        let Ok(mut seen) = self.warned.lock() else {
            return Vec::new();
        };
        warnings
            .iter()
            .filter(|w| seen.insert((*w).clone()))
            .cloned()
            .collect()
    }
}

impl<R: CheckRunner> CheckRunner for ProjectEnvRunner<R> {
    fn run(&self, spec: &CommandSpec, targets: &[String]) -> Result<ProcessOutput, PortError> {
        let found = resolve(&spec.cwd, &self.host);
        for w in self.unseen(&found.warnings) {
            eprintln!("hookah: warning: config: {w}");
        }
        if found.entries.is_empty() {
            return self.inner.run(spec, targets);
        }
        let mut env = spec.env.clone();
        env.extend(found.pairs());
        self.inner.run(&spec.clone().with_env(env), targets)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    struct Echo;

    impl CheckRunner for Echo {
        fn run(&self, spec: &CommandSpec, _: &[String]) -> Result<ProcessOutput, PortError> {
            Ok(ProcessOutput {
                stdout: format!("{:?}", spec.env),
                ..ProcessOutput::default()
            })
        }
    }

    fn quiet() -> Host {
        Host {
            var: Box::new(|_| None),
            windows: false,
        }
    }

    #[test]
    fn each_warning_is_shown_once() {
        let runner = ProjectEnvRunner::new(Echo, quiet());
        let w = |s: &str| s.to_string();
        assert_eq!(runner.unseen(&[w("a"), w("b")]), [w("a"), w("b")]);
        assert_eq!(runner.unseen(&[w("b"), w("c")]), [w("c")]);
        assert!(runner.unseen(&[w("a")]).is_empty());
    }

    #[test]
    fn config_variables_follow_the_detectors_and_win() {
        let dir = std::env::temp_dir().join(format!("hookah-runner-unit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".hookah.local.toml"), "[env]\nK = \"cfg\"\n").unwrap();
        let spec = CommandSpec::new("x", vec![], &dir, std::time::Duration::from_secs(1))
            .with_env(vec![("K".into(), "det".into()), ("D".into(), "d".into())]);
        let out = ProjectEnvRunner::new(Echo, quiet())
            .run(&spec, &[])
            .unwrap();
        assert_eq!(out.stdout, r#"[("K", "det"), ("D", "d"), ("K", "cfg")]"#);
        let other = std::env::temp_dir().join(format!("hookah-runner-none-{}", std::process::id()));
        std::fs::create_dir_all(&other).unwrap();
        let plain = CommandSpec::new("x", vec![], &other, std::time::Duration::from_secs(1));
        let out = ProjectEnvRunner::new(Echo, quiet())
            .run(&plain, &[])
            .unwrap();
        let _ = std::fs::remove_dir_all(&other);
        assert_eq!(out.stdout, "[]", "no config below: nothing added");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
