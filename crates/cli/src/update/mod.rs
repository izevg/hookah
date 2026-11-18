//! `hookah update`, and the update side of a session start: wiring only. The decisions live in
//! `hookah-application` and `hookah-domain`.

mod render;

use std::io::Write;

use hookah_application::{RunUpdate, ScheduleUpdate, UpdateMode};
use hookah_domain::verdict::Notice;

use crate::wiring;

/// What a session start does about updates: the notice to show, if any. Starts a background
/// check when one is due. Returns at once.
pub fn session_start_notice() -> Option<Notice> {
    let adapters = wiring::updates();
    ScheduleUpdate::new(adapters.ports(), &adapters.env)
        .run()
        .map(|event| Notice::Update(event.to_string()))
}

/// `hookah update [--check]` and the hidden `--background` run. Returns the exit code.
///
/// The background run is silent and always exits 0: nobody is reading it, and a failure is
/// recorded in the state for the next session start to mention.
pub fn run(check: bool, background: bool) -> i32 {
    let adapters = wiring::updates();
    adapters.installer().clean_up();
    let mode = if background {
        UpdateMode::Background
    } else if check {
        UpdateMode::Check
    } else {
        UpdateMode::Install
    };
    if !background {
        print_out(&render::start(&adapters.env, check));
    }
    let report = RunUpdate::new(adapters.ports(), &adapters.env).run(mode);
    if background {
        return 0;
    }
    let (code, text) = render::finish(&adapters.env.current, &report);
    if code == 0 {
        print_out(&text);
    } else {
        let _ = write!(std::io::stderr(), "{text}");
    }
    code
}

fn print_out(text: &str) {
    let _ = write!(std::io::stdout(), "{text}");
}
