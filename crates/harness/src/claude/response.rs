//! Claude Code hook response.
//!
//! A failing verdict becomes exit code 2 with the report on stderr, which Claude Code feeds back
//! to the model. A passing or degraded verdict becomes exit code 0; notices go out as
//! non-blocking context.
