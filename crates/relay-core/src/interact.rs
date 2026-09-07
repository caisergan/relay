//! Prompts: the engine asking a person a question without blocking its runtime.
//!
//! FileZilla's async-request pattern, made await-native. A backend calls
//! [`Interact::ask`] and awaits; the broker records the question, publishes it, and
//! parks a oneshot. The shell only *transports* the question and the answer — the
//! facts and the pending state live here, so a UI remount reconstructs the open
//! prompt instead of creating a second one.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use specta::Type;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::events::EngineEvent;
use crate::model::{Direction, FileFacts, PromptId, SessionId};

/// A prompt nobody answers must not hold a transfer slot forever.
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Prompt {
    #[serde(rename_all = "camelCase")]
    HostKey {
        host: String,
        algo: String,
        sha256: String,
        /// A pinned key for this endpoint exists and does not match. The design draws
        /// this variant in red: it is a different question, not a louder one.
        changed: bool,
    },
    #[serde(rename_all = "camelCase")]
    TlsCert {
        host: String,
        chain_pem: Vec<String>,
        sha256: String,
        changed: bool,
    },
    #[serde(rename_all = "camelCase")]
    Password { hint: String },
    #[serde(rename_all = "camelCase")]
    Conflict {
        local: FileFacts,
        remote: FileFacts,
        direction: Direction,
        /// How many queued jobs "apply to remaining" would cover.
        remaining: u32,
        /// Resume is offered only when the engine has already verified it is safe.
        resume_allowed: bool,
    },
}

impl Prompt {
    /// Prompts are per-session; the UI groups them onto the right tab.
    pub fn is_trust_decision(&self) -> bool {
        matches!(self, Prompt::HostKey { .. } | Prompt::TlsCert { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ConflictAction {
    Overwrite,
    Skip,
    KeepBoth,
    /// Only reachable when the engine offered it: see `Prompt::Conflict::resume_allowed`.
    Resume,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PromptReply {
    #[serde(rename_all = "camelCase")]
    Accept {
        remember: bool,
    },
    Deny,
    #[serde(rename_all = "camelCase")]
    Password {
        value: String,
    },
    #[serde(rename_all = "camelCase")]
    Conflict {
        action: ConflictAction,
        apply_to_remaining: bool,
    },
}

impl PromptReply {
    /// Guard against a reply that answers a different question than the one asked.
    fn answers(&self, prompt: &Prompt) -> bool {
        matches!(
            (prompt, self),
            (
                Prompt::HostKey { .. } | Prompt::TlsCert { .. },
                PromptReply::Accept { .. } | PromptReply::Deny,
            ) | (
                Prompt::Password { .. },
                PromptReply::Password { .. } | PromptReply::Deny,
            ) | (
                Prompt::Conflict { .. },
                PromptReply::Conflict { .. } | PromptReply::Deny,
            )
        )
    }
}

/// A prompt as the UI sees it. Carried in `PromptOpened` and in every snapshot until
/// it is answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PromptRequest {
    pub id: PromptId,
    pub session: SessionId,
    pub prompt: Prompt,
    pub opened_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ResolveError {
    /// Already answered, timed out, or cancelled with its session. Replaying a reply
    /// after a remount lands here, and must not be treated as a failure by the UI.
    Unknown,
    /// The reply does not answer this kind of prompt.
    Mismatched,
}

#[async_trait]
pub trait Interact: Send + Sync {
    async fn ask(&self, session: SessionId, prompt: Prompt) -> PromptReply;

    /// Ask using an id the caller already knows.
    ///
    /// A transfer publishes `JobState::AwaitingPrompt { prompt }` *before* it has an
    /// answer, so the drawer can say what a stalled job is waiting for. That needs the
    /// id up front, which [`Interact::ask`] cannot give. Implementations that do not
    /// track ids may ignore it; the broker does not.
    async fn ask_with_id(&self, _id: PromptId, session: SessionId, prompt: Prompt) -> PromptReply {
        self.ask(session, prompt).await
    }
}

struct Pending {
    request: PromptRequest,
    reply: oneshot::Sender<PromptReply>,
}

/// Owns every open question in the engine.
pub struct PromptBroker {
    pending: Mutex<HashMap<PromptId, Pending>>,
    events: mpsc::Sender<EngineEvent>,
    timeout: Duration,
}

impl PromptBroker {
    pub fn new(events: mpsc::Sender<EngineEvent>) -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            events,
            timeout: PROMPT_TIMEOUT,
        }
    }

    #[doc(hidden)] // tests only: keeps the timeout path exercisable in milliseconds.
    pub fn with_timeout(events: mpsc::Sender<EngineEvent>, timeout: Duration) -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            events,
            timeout,
        }
    }

    /// Every unanswered prompt, for `engine_snapshot`.
    pub fn pending(&self) -> Vec<PromptRequest> {
        let guard = self.pending.lock().expect("prompt broker poisoned");
        let mut out: Vec<_> = guard.values().map(|p| p.request.clone()).collect();
        out.sort_by_key(|r| r.opened_at);
        out
    }

    /// Answer a prompt. Idempotent by construction: the pending entry is removed under
    /// the lock, so a duplicate reply from a remounted UI reports `Unknown` and changes
    /// nothing.
    pub fn resolve(&self, id: PromptId, reply: PromptReply) -> Result<(), ResolveError> {
        let pending = {
            let mut guard = self.pending.lock().expect("prompt broker poisoned");
            match guard.get(&id) {
                None => return Err(ResolveError::Unknown),
                Some(p) if !reply.answers(&p.request.prompt) => {
                    return Err(ResolveError::Mismatched);
                }
                Some(_) => guard.remove(&id).expect("checked above"),
            }
        };
        // A dropped receiver means the asker gave up first; the prompt is still closed.
        let _ = pending.reply.send(reply);
        self.emit_closed(id);
        Ok(())
    }

    /// Deny everything a session still has open. Called by `session_close` and by
    /// shutdown, so no transfer task is left awaiting an answer that cannot come.
    pub fn deny_session(&self, session: SessionId) {
        let denied: Vec<Pending> = {
            let mut guard = self.pending.lock().expect("prompt broker poisoned");
            let ids: Vec<PromptId> = guard
                .values()
                .filter(|p| p.request.session == session)
                .map(|p| p.request.id)
                .collect();
            ids.iter().filter_map(|id| guard.remove(id)).collect()
        };
        for pending in denied {
            let id = pending.request.id;
            let _ = pending.reply.send(PromptReply::Deny);
            self.emit_closed(id);
        }
    }

    fn emit_closed(&self, id: PromptId) {
        // Bounded channel: if the coordinator is gone the engine is shutting down.
        let _ = self.events.try_send(EngineEvent::PromptClosed { id });
    }
}

impl PromptBroker {
    async fn open(&self, id: PromptId, session: SessionId, prompt: Prompt) -> PromptReply {
        let (tx, rx) = oneshot::channel();
        let request = PromptRequest {
            id,
            session,
            prompt,
            opened_at: Utc::now(),
        };
        {
            let mut guard = self.pending.lock().expect("prompt broker poisoned");
            guard.insert(
                id,
                Pending {
                    request: request.clone(),
                    reply: tx,
                },
            );
        }
        if self
            .events
            .send(EngineEvent::PromptOpened { prompt: request })
            .await
            .is_err()
        {
            self.pending
                .lock()
                .expect("prompt broker poisoned")
                .remove(&id);
            return PromptReply::Deny;
        }

        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(reply)) => reply,
            // Sender dropped (session denied it) or the wait expired: both mean no.
            Ok(Err(_)) => PromptReply::Deny,
            Err(_) => {
                self.pending
                    .lock()
                    .expect("prompt broker poisoned")
                    .remove(&id);
                self.emit_closed(id);
                PromptReply::Deny
            }
        }
    }
}

#[async_trait]
impl Interact for PromptBroker {
    async fn ask(&self, session: SessionId, prompt: Prompt) -> PromptReply {
        self.open(Uuid::new_v4(), session, prompt).await
    }

    async fn ask_with_id(&self, id: PromptId, session: SessionId, prompt: Prompt) -> PromptReply {
        self.open(id, session, prompt).await
    }
}
