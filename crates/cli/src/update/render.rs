//! The words of `hookah update`: what it says before it works, and how each result reads.

use hookah_application::{UpdateEnv, UpdateReport};
use hookah_domain::update::{REPOSITORY, Version};

const INSTALLER: &str = "https://izevg.github.io/hookah/install.sh";

/// The line printed before anything is checked.
pub fn start(env: &UpdateEnv, check_only: bool) -> String {
    let what = if check_only {
        "checking"
    } else {
        "checking and installing"
    };
    format!(
        "hookah {}: {what} the latest release of github.com/{REPOSITORY}\n",
        env.current
    )
}

/// The exit code and the text of a finished run. Success goes to stdout, a failure to stderr:
/// the caller decides by the code.
pub fn finish(current: &Version, report: &UpdateReport) -> (i32, String) {
    match report {
        UpdateReport::UpToDate { latest } => (
            0,
            format!("hookah {current} is up to date (latest release: {latest})\n"),
        ),
        UpdateReport::Available { latest } => (
            0,
            format!("hookah {latest} is available (running {current}); run `hookah update` to install it\n"),
        ),
        UpdateReport::Updated { from, to } => (
            0,
            format!("updated hookah from {from} to {to}; new sessions use it\n"),
        ),
        UpdateReport::NotDue => (0, "the last check was recent; nothing to do\n".into()),
        UpdateReport::Disabled => (
            1,
            "hookah: updates are switched off (HOOKAH_NO_UPDATE is set, or `[update] check = false` in the user config)\n".into(),
        ),
        UpdateReport::DevBuild => (
            1,
            format!(
                "hookah: this is a development build ({current}) and does not update itself; install a release: curl -fsSL {INSTALLER} | sh\n"
            ),
        ),
        UpdateReport::Busy => (1, "hookah: another update is running; try again in a minute\n".into()),
        UpdateReport::Failed(error) => (1, format!("hookah: update failed: {error}\n")),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use hookah_application::ports::UpdateError;
    use hookah_domain::update::Policy;

    use super::*;

    fn v(t: &str) -> Version {
        Version::parse(t).unwrap()
    }

    #[test]
    fn the_first_line_says_what_is_about_to_happen() {
        let env = UpdateEnv {
            current: v("1.0.0"),
            target: None,
            policy: Policy::default(),
        };
        assert_eq!(
            start(&env, false),
            "hookah 1.0.0: checking and installing the latest release of github.com/izevg/hookah\n"
        );
        assert!(start(&env, true).contains(": checking the latest"));
    }

    #[test]
    fn good_outcomes_exit_zero() {
        let cur = v("1.0.0");
        let cases = [
            (UpdateReport::UpToDate { latest: v("1.0.0") }, "up to date"),
            (
                UpdateReport::Available { latest: v("1.1.0") },
                "hookah update",
            ),
            (
                UpdateReport::Updated {
                    from: v("1.0.0"),
                    to: v("1.1.0"),
                },
                "from 1.0.0 to 1.1.0",
            ),
            (UpdateReport::NotDue, "recent"),
        ];
        for (report, text) in cases {
            let (code, out) = finish(&cur, &report);
            assert_eq!(code, 0, "{report:?}");
            assert!(out.contains(text), "{out}");
            assert!(out.ends_with('\n'));
        }
    }

    #[test]
    fn problems_exit_one_with_a_reason() {
        let cur = v("1.0.0");
        let cases = [
            (UpdateReport::Disabled, "HOOKAH_NO_UPDATE"),
            (UpdateReport::DevBuild, "development build (1.0.0)"),
            (UpdateReport::Busy, "another update"),
            (
                UpdateReport::Failed(UpdateError::failed("offline")),
                "update failed: offline",
            ),
        ];
        for (report, text) in cases {
            let (code, out) = finish(&cur, &report);
            assert_eq!(code, 1, "{report:?}");
            assert!(out.contains(text), "{out}");
        }
    }
}
