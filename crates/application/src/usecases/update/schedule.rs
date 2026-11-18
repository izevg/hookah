//! `ScheduleUpdate`: what a session start does about updates.

use hookah_domain::update::Event;

use super::UpdateEnv;
use crate::ports::UpdatePorts;

/// Reports what the updater left for the user and starts a check when one is due.
pub struct ScheduleUpdate<'a> {
    ports: UpdatePorts<'a>,
    env: &'a UpdateEnv,
}

impl<'a> ScheduleUpdate<'a> {
    /// Creates the use case.
    pub fn new(ports: UpdatePorts<'a>, env: &'a UpdateEnv) -> Self {
        Self { ports, env }
    }

    /// The pending event, shown once, if there is one. Starts a background check when the last
    /// one is a day old (an hour after a failed one). Returns at once and never fails: a store or
    /// spawn problem only means no notice and no check this time.
    pub fn run(&self) -> Option<Event> {
        if !self.env.policy.check || self.env.current.is_dev() {
            return None;
        }
        let mut state = self.ports.store.load();
        let event = state.take_pending();
        if event.is_some() {
            let _ = self.ports.store.save(&state);
        }
        if state.due(self.ports.clock.now_ms()) {
            let _ = self.ports.spawner.spawn_check();
        }
        event
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use hookah_domain::update::state::CHECK_INTERVAL_MS;
    use hookah_domain::update::{Policy, Target};

    use super::super::fakes::{World, v};
    use super::*;

    fn env(current: &str, policy: Policy) -> UpdateEnv {
        UpdateEnv {
            current: v(current),
            target: Some(Target::MacArm64),
            policy,
        }
    }

    fn world() -> World {
        World::serving("2.0.0", Target::MacArm64)
    }

    fn run(w: &World, e: &UpdateEnv) -> Option<Event> {
        ScheduleUpdate::new(w.ports(), e).run()
    }

    #[test]
    fn a_first_session_starts_a_check_and_says_nothing() {
        let w = world();
        assert_eq!(run(&w, &env("1.0.0", Policy::default())), None);
        assert_eq!(w.spawns.get(), 1);
    }

    #[test]
    fn a_recent_check_is_not_repeated() {
        let w = world();
        w.state
            .borrow_mut()
            .record_checked(w.now.get() - 1000, &v("1.0.0"));
        assert_eq!(run(&w, &env("1.0.0", Policy::default())), None);
        assert_eq!(w.spawns.get(), 0);
        w.now.set(w.now.get() + CHECK_INTERVAL_MS);
        run(&w, &env("1.0.0", Policy::default()));
        assert_eq!(w.spawns.get(), 1);
    }

    #[test]
    fn a_dev_build_and_an_opt_out_do_nothing_at_all() {
        let w = world();
        w.state
            .borrow_mut()
            .record_updated(&v("1.0.0"), &v("1.1.0"));
        assert_eq!(run(&w, &env("0.0.0", Policy::default())), None);
        let off = Policy {
            check: false,
            auto: true,
        };
        assert_eq!(run(&w, &env("1.0.0", off)), None);
        assert_eq!(w.spawns.get(), 0);
        assert_eq!(w.saves.get(), 0);
        assert!(
            w.state.borrow().pending.is_some(),
            "left for a later session"
        );
    }

    #[test]
    fn a_pending_event_is_handed_out_once() {
        let w = world();
        w.state
            .borrow_mut()
            .record_checked(w.now.get(), &v("1.1.0"));
        w.state
            .borrow_mut()
            .record_updated(&v("1.0.0"), &v("1.1.0"));
        let e = env("1.1.0", Policy::default());
        assert_eq!(
            run(&w, &e),
            Some(Event::Updated {
                from: "1.0.0".into(),
                to: "1.1.0".into()
            })
        );
        assert_eq!(w.saves.get(), 1);
        assert_eq!(run(&w, &e), None);
        assert_eq!(w.saves.get(), 1, "nothing left to store");
    }

    #[test]
    fn store_and_spawn_failures_are_swallowed() {
        let mut w = world();
        w.save_fails = true;
        w.spawn_fails = true;
        w.state
            .borrow_mut()
            .record_updated(&v("1.0.0"), &v("1.1.0"));
        assert!(run(&w, &env("1.1.0", Policy::default())).is_some());
        assert!(
            w.state.borrow().pending.is_some(),
            "not stored, so shown again"
        );
    }
}
