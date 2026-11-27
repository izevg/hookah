//! Wire Antigravity's hook events and its per-conversation feedback to the use cases.

use hookah_adapters::store::hook_feedback::HookFeedback;
use hookah_application::{StartRequest, StopRequest};
use hookah_harness::antigravity::{self, Event};
use hookah_harness::protocol::{Invocation, Response, blocking_report, merge_outcomes};

use crate::hooks::model::{self, ModelGate};
use crate::{run_invocation, warning};

pub(crate) fn run(stdin: &str, limit: usize) -> Response {
    let payload = match antigravity::parse(stdin) {
        Ok(payload) => payload,
        Err(error) => return warning(error.to_string()),
    };
    let Some(root) = payload.roots.first() else {
        return if matches!(payload.event, Event::Stop { .. }) {
            antigravity::stop(None)
        } else {
            antigravity::acknowledge()
        };
    };
    let gate = ModelGate::new(&payload.model);
    let feedback = || HookFeedback::antigravity(root, &payload.session);
    match payload.event {
        Event::Start => {
            let mut response = antigravity::acknowledge();
            for root in payload.roots {
                let invocation = Invocation::Start(StartRequest::new(root, &payload.session));
                antigravity::warn(&mut response, &run_invocation(&invocation, limit, &gate));
            }
            response
        }
        Event::Edit(request) => {
            let scope = model::edit_dir(&request).to_path_buf();
            if !gate.allows(&scope) {
                return antigravity::acknowledge();
            }
            let outcome = run_invocation(&Invocation::Edit(request), limit, &gate);
            let mut response = antigravity::acknowledge();
            antigravity::warn(&mut response, &outcome);
            if let Some(message) = antigravity::edit_feedback(&outcome)
                && let Err(error) = feedback().and_then(|store| store.push_scoped(message, scope))
            {
                response.stderr.push_str(&feedback_warning(error));
            }
            response
        }
        Event::BeforeModel => match feedback().and_then(|store| {
            store.take_where(|scope| gate.allows(scope.unwrap_or(root.as_path())))
        }) {
            Ok(messages) if !messages.is_empty() => antigravity::inject(messages.join("\n")),
            Ok(_) => antigravity::acknowledge(),
            Err(error) => Response {
                stderr: feedback_warning(error),
                ..antigravity::acknowledge()
            },
        },
        Event::Stop { eligible: true } => {
            let outcomes = payload
                .roots
                .iter()
                .map(|root| {
                    let request =
                        StopRequest::new(root).with_session(Some(payload.session.clone()));
                    let invocation = Invocation::Stop {
                        request,
                        already_continuing: false,
                    };
                    run_invocation(&invocation, limit, &gate)
                })
                .collect();
            let outcome = merge_outcomes(outcomes, limit);
            let report = blocking_report(&outcome);
            if report.is_none() && !gate.any_allowed() {
                return antigravity::stop(None);
            }
            let claim = feedback()
                .and_then(|store| store.claim_continuation(&payload.prompt, report.is_some()));
            let mut response = match (&claim, report) {
                (Ok(true), Some(report)) => antigravity::stop(Some(format!(
                    "Fix these findings before finishing:\n{report}"
                ))),
                _ => antigravity::stop(None),
            };
            antigravity::warn(&mut response, &outcome);
            match (claim, report) {
                (Err(error), _) => response.stderr.push_str(&feedback_warning(error)),
                (Ok(false), Some(report)) => response.stderr.push_str(&format!("{report}\n")),
                _ => {}
            }
            response
        }
        Event::Stop { eligible: false } => antigravity::stop(None),
        Event::Ignore => antigravity::acknowledge(),
    }
}

fn feedback_warning(error: std::io::Error) -> String {
    format!("hookah: antigravity could not relay feedback: {error}\n")
}
