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
    pending: Vec<Entry>,
    continued_for: Option<u64>,
}

/// A queued edit feedback message with the optional config directory that produced it.
#[derive(PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
enum Entry {
    Scoped { message: String, scope: PathBuf },
    Legacy(String),
}

impl Entry {
    fn message(&self) -> &str {
        match self {
            Self::Scoped { message, .. } | Self::Legacy(message) => message,
        }
    }

    fn scope(&self) -> Option<&Path> {
        match self {
            Self::Scoped { scope, .. } => Some(scope.as_path()),
            Self::Legacy(_) => None,
        }
    }

    fn into_message(self) -> String {
        match self {
            Self::Scoped { message, .. } | Self::Legacy(message) => message,
        }
    }
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
        self.push_entry(Entry::Legacy(message))
    }

    /// Queue actionable feedback tagged with the config directory that admitted the edit.
    pub fn push_scoped(&self, message: String, scope: PathBuf) -> io::Result<()> {
        self.push_entry(Entry::Scoped { message, scope })
    }

    fn push_entry(&self, entry: Entry) -> io::Result<()> {
        self.update(move |state| {
            if !state
                .pending
                .iter()
                .any(|existing| existing.message() == entry.message())
            {
                state.pending.push(entry);
            }
        })
    }

    /// Consume all queued messages that are ready for this model call.
    pub fn take(&self) -> io::Result<Vec<String>> {
        self.take_where(|_| true)
    }

    /// Consume only the queued messages whose originating scope satisfies `allow`, keeping the
    /// remaining entries queued for later.
    pub fn take_where(
        &self,
        mut allow: impl FnMut(Option<&Path>) -> bool,
    ) -> io::Result<Vec<String>> {
        if !self.path.try_exists()? {
            return Ok(Vec::new());
        }
        self.update(|state| {
            let mut delivered = Vec::new();
            let mut remaining = Vec::new();
            for entry in std::mem::take(&mut state.pending) {
                if allow(entry.scope()) {
                    delivered.push(entry.into_message());
                } else {
                    remaining.push(entry);
                }
            }
            state.pending = remaining;
            delivered
        })
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
    fn clean_conversations_leave_no_feedback_files() {
        let dir =
            std::env::temp_dir().join(format!("hookah-no-feedback-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let feedback = HookFeedback {
            path: dir.join("messages.json"),
        };
        assert!(!feedback.claim_continuation("prompt", false).unwrap());
        assert!(feedback.take().unwrap().is_empty());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        assert!(feedback.claim_continuation("prompt", true).unwrap());
        assert!(!feedback.claim_continuation("prompt", true).unwrap());
        assert!(!feedback.claim_continuation("prompt", false).unwrap());
        assert!(feedback.claim_continuation("prompt", true).unwrap());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unreadable_state_is_reported_to_the_caller() {
        let dir = std::env::temp_dir().join(format!("hookah-bad-feedback-{}", std::process::id()));
        let feedback = HookFeedback {
            path: dir.join("messages.json"),
        };
        fs::create_dir_all(&feedback.path).unwrap();
        assert!(feedback.take().is_err());
        assert!(feedback.push("message".into()).is_err());
        assert!(feedback.claim_continuation("prompt", true).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

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

    #[test]
    fn scoped_entries_deliver_only_matching_scopes_and_preserve_others() {
        let dir =
            std::env::temp_dir().join(format!("hookah-scoped-feedback-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let feedback = HookFeedback {
            path: dir.join("messages.json"),
        };
        feedback
            .push_scoped("from-a".into(), PathBuf::from("/root/a"))
            .unwrap();
        feedback
            .push_scoped("from-b".into(), PathBuf::from("/root/b"))
            .unwrap();
        let only_b = feedback
            .take_where(|scope| scope == Some(Path::new("/root/b")))
            .unwrap();
        assert_eq!(only_b, ["from-b"]);
        let remaining = feedback.take().unwrap();
        assert_eq!(remaining, ["from-a"]);
        fs::remove_dir_all(dir).unwrap();
    }
}
