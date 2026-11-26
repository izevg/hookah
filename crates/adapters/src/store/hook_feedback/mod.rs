//! Per-conversation feedback passed between Antigravity's independent hook processes.

use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::cache_dir::{fnv1a, project_cache_dir};

/// Pending edit feedback and the last prompt for which the stop hook requested continuation.
pub struct HookFeedback {
    path: PathBuf,
}

#[derive(Default, PartialEq, Eq, Deserialize, Serialize)]
struct State {
    pending: Vec<String>,
    continued_for: Option<u64>,
}

impl HookFeedback {
    /// Locate the private feedback file for a workspace and conversation.
    pub fn antigravity(root: &Path, conversation: &str) -> io::Result<Self> {
        let directory = project_cache_dir(root).map_err(|e| io::Error::other(e.to_string()))?;
        let key = fnv1a(conversation.as_bytes());
        Ok(Self {
            path: directory.join(format!("antigravity-{key:016x}.json")),
        })
    }

    /// Queue actionable feedback once per batch, even when several edits happen concurrently.
    pub fn push(&self, message: String) -> io::Result<()> {
        self.update(move |state| {
            if !state.pending.contains(&message) {
                state.pending.push(message);
            }
        })
    }

    /// Consume the messages that are ready for this model call.
    pub fn take(&self) -> io::Result<Vec<String>> {
        if !self.path.try_exists()? {
            return Ok(Vec::new());
        }
        self.update(|state| std::mem::take(&mut state.pending))
    }

    /// Return true once per prompt with unresolved findings. A clean stop resets the guard.
    pub fn claim_continuation(&self, prompt: &str, has_findings: bool) -> io::Result<bool> {
        if !has_findings && !self.path.try_exists()? {
            return Ok(false);
        }
        let prompt_key = fnv1a(prompt.as_bytes());
        self.update(|state| {
            if !has_findings {
                state.continued_for = None;
                false
            } else if state.continued_for == Some(prompt_key) {
                false
            } else {
                state.continued_for = Some(prompt_key);
                true
            }
        })
    }

    fn update<T>(&self, change: impl FnOnce(&mut State) -> T) -> io::Result<T> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.path.with_extension("lock"))?;
        lock.lock()?;
        let mut state: State = match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == ErrorKind::NotFound => State::default(),
            Err(error) => return Err(error),
        };
        let before = serde_json::to_vec(&state)?;
        let result = change(&mut state);
        let after = serde_json::to_vec(&state)?;
        if after != before {
            let tmp = self.path.with_extension("tmp");
            let saved = fs::write(&tmp, after).and_then(|()| fs::rename(&tmp, &self.path));
            if saved.is_err() {
                let _ = fs::remove_file(tmp);
            }
            saved?;
        }
        Ok(result)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_writers_deliver_one_copy_of_each_message() {
        let dir = std::env::temp_dir().join(format!("hookah-feedback-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let feedback = HookFeedback {
            path: dir.join("messages.json"),
        };
        std::thread::scope(|scope| {
            let feedback = &feedback;
            for message in ["a", "b", "a"] {
                scope.spawn(move || feedback.push(message.into()).unwrap());
            }
        });
        let mut messages = feedback.take().unwrap();
        messages.sort_unstable();
        assert_eq!(messages, ["a", "b"]);
        assert!(feedback.take().unwrap().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
}
