//! Common harness protocol.
//!
//! - [`Harness`]: the trait every adapter implements: `parse(stdin) -> Invocation` and
//!   `render(invocation, outcome) -> Response`.
//! - [`Invocation`]: which use case a payload asks for.
//! - [`Response`]: what the CLI writes and returns (exit code, stdout, stderr).
//!
//! Also holds the shared fail-open rule: only `Verdict::Fail` may block the agent;
//! `Verdict::Degraded` never does. See [`blocking_report`].

use std::fmt;

use hookah_application::{EditRequest, Outcome, StopRequest};
use hookah_domain::verdict::{Report, Verdict};

/// What a harness payload asks `hookah` to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    /// A file was just edited: run the edit phase.
    Edit(EditRequest),
    /// The agent wants to stop: run the stop phase.
    Stop {
        /// The use case input.
        request: StopRequest,
        /// The harness says this stop attempt already follows a blocked stop. Blocking again
        /// could loop forever, so adapters must not block in this state.
        already_continuing: bool,
    },
    /// An event `hookah` does not care about.
    Ignore,
}

/// What the CLI writes and returns to the harness.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Response {
    /// Process exit code.
    pub exit_code: i32,
    /// Text for standard output.
    pub stdout: String,
    /// Text for standard error.
    pub stderr: String,
}

impl Response {
    /// Exit 0 with nothing to say.
    pub fn silent() -> Self {
        Self::default()
    }
}

/// A payload that could not be understood. The CLI turns it into a warning and exit code 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadError(pub String);

impl fmt::Display for PayloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "malformed hook payload: {}", self.0)
    }
}

impl std::error::Error for PayloadError {}

/// A driving adapter for one agent harness.
pub trait Harness {
    /// Parses the raw payload read from standard input.
    fn parse(&self, stdin: &str) -> Result<Invocation, PayloadError>;

    /// Renders the use case outcome in this harness's protocol.
    fn render(&self, invocation: &Invocation, outcome: &Outcome) -> Response;
}

/// The report that may block the agent, if any. This is the shared fail-open rule: `Pass` and
/// `Degraded` verdicts never block.
pub fn blocking_report(outcome: &Outcome) -> Option<&Report> {
    match &outcome.verdict {
        Verdict::Fail(report) => Some(report),
        Verdict::Pass | Verdict::Degraded(_) => None,
    }
}
