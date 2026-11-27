//! What the user said about updates: `HOOKAH_NO_UPDATE` and the `[update]` table of the user
//! config (`~/.config/hookah/config.toml`; see [`Host::user_config`]).
//!
//! `HOOKAH_NO_UPDATE` set to anything but empty, `0`, `false`, `no`, or `off` switches updates
//! off, and beats the file. In the file, `check = false` switches off every check and install and
//! `auto = false` keeps the check and drops the automatic install. A missing, unreadable, or
//! broken file means the defaults: both on.

use hookah_domain::update::Policy;
use serde::Deserialize;

use crate::config::env::Host;
use crate::config::env::file::UpdateSection;

/// Environment variable that switches updates off.
pub const NO_UPDATE_VAR: &str = "HOOKAH_NO_UPDATE";

/// Reads only `[update]` from the user config so a mistyped sibling key (such as `models`)
/// never disables a user's update opt-out.
#[derive(Default, Deserialize)]
struct UpdateFile {
    #[serde(default)]
    update: UpdateSection,
}

/// The policy in effect for `host`.
pub fn policy(host: &Host) -> Policy {
    let mut policy = Policy::default();
    if let Some(path) = host.user_config()
        && let Ok(text) = std::fs::read_to_string(path)
        && let Ok(parsed) = toml_edit::de::from_str::<UpdateFile>(&text)
    {
        policy.check = parsed.update.check.unwrap_or(true);
        policy.auto = parsed.update.auto.unwrap_or(true);
    }
    if (host.var)(NO_UPDATE_VAR).is_some_and(|v| switches_off(&v)) {
        policy.check = false;
    }
    policy
}

fn switches_off(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    !matches!(value.as_str(), "" | "0" | "false" | "no" | "off")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hookah-upd-set-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A host whose user config is `text` (when given) and whose environment is `vars`.
    fn host(name: &str, text: Option<&str>, vars: &[(&str, &str)]) -> Host {
        let dir = scratch(name);
        let config = dir.join("config.toml");
        if let Some(text) = text {
            fs::write(&config, text).unwrap();
        }
        let mut vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        vars.push(("HOOKAH_CONFIG".into(), config.display().to_string()));
        Host {
            var: Box::new(move |n| vars.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone())),
            windows: false,
        }
    }

    fn pol(check: bool, auto: bool) -> Policy {
        Policy { check, auto }
    }

    #[test]
    fn defaults_are_on() {
        assert_eq!(policy(&host("none", None, &[])), pol(true, true));
        assert_eq!(policy(&host("empty", Some(""), &[])), pol(true, true));
    }

    #[test]
    fn the_config_file_turns_each_switch_off() {
        assert_eq!(
            policy(&host("check", Some("[update]\ncheck = false\n"), &[])),
            pol(false, true)
        );
        assert_eq!(
            policy(&host("auto", Some("[update]\nauto = false\n"), &[])),
            pol(true, false)
        );
        assert_eq!(
            policy(&host(
                "on",
                Some("[update]\ncheck = true\nauto = true\n"),
                &[]
            )),
            pol(true, true)
        );
    }

    #[test]
    fn a_broken_config_means_the_defaults() {
        assert_eq!(
            policy(&host("broken", Some("[update\ncheck = false"), &[])),
            pol(true, true)
        );
    }

    #[test]
    fn a_mistyped_models_key_still_honors_the_update_opt_out() {
        let text = "models = \"gpt-5\"\n[update]\ncheck = false\nauto = false\n";
        assert_eq!(
            policy(&host("mistyped-models", Some(text), &[])),
            pol(false, false)
        );
        // The full config parser still reports the type error.
        assert!(crate::config::env::file::parse(text).is_err());
    }

    #[test]
    fn the_environment_switches_everything_off() {
        for on in ["1", "true", "yes", "anything", " 1 "] {
            assert_eq!(
                policy(&host("env", None, &[(NO_UPDATE_VAR, on)])),
                pol(false, true),
                "{on:?}"
            );
        }
        for off in ["", "0", "false", "No", "OFF"] {
            assert_eq!(
                policy(&host("envoff", None, &[(NO_UPDATE_VAR, off)])),
                pol(true, true),
                "{off:?}"
            );
        }
    }

    #[test]
    fn the_environment_beats_the_file() {
        let h = host(
            "both",
            Some("[update]\ncheck = true\n"),
            &[(NO_UPDATE_VAR, "1")],
        );
        assert_eq!(policy(&h), pol(false, true));
    }
}
