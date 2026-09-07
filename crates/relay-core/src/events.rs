//! Engine → app updates. One enum, one ordered stream.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use specta::Type;
use uuid::Uuid;

use crate::interact::PromptRequest;
use crate::job::{JobSnapshot, QueueStats};
use crate::model::{PromptId, RemoteEntry, Session, SessionId, SessionState};
use crate::wire::Seq;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum LogKind {
    Status,
    Command,
    Response,
    Error,
}

/// One line of the per-session protocol log. Secrets are redacted before a line is
/// constructed, not on the way out: the phase 5 audit greps these buffers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub at: DateTime<Utc>,
    pub kind: LogKind,
    pub line: String,
}

/// The human-readable feed behind the activity slide-over (full UI is phase 4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    pub id: Uuid,
    pub at: DateTime<Utc>,
    pub text: String,
    pub detail: Option<String>,
}

/// A directory listing as currently known for a session's remote pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ListingSnapshot {
    pub session: SessionId,
    pub path: String,
    /// Monotonic per session. The pane ignores a listing whose request is older than
    /// the one it is waiting for, so a slow response cannot replace a newer directory.
    pub request: Seq,
    pub entries: Vec<RemoteEntry>,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EngineEvent {
    /// A tab exists. Sent before any state for that session.
    #[serde(rename_all = "camelCase")]
    SessionOpened { session: Box<Session> },
    #[serde(rename_all = "camelCase")]
    SessionState { id: SessionId, state: SessionState },
    #[serde(rename_all = "camelCase")]
    SessionClosed { id: SessionId },
    #[serde(rename_all = "camelCase")]
    Latency { id: SessionId, ms: u32 },
    #[serde(rename_all = "camelCase")]
    Listing { listing: Box<ListingSnapshot> },
    /// Covers progress and every state change; the coordinator coalesces the
    /// progress-only ones before they are sequenced.
    #[serde(rename_all = "camelCase")]
    JobUpdate { job: Box<JobSnapshot> },
    #[serde(rename_all = "camelCase")]
    QueueStats { stats: QueueStats },
    #[serde(rename_all = "camelCase")]
    Log { id: SessionId, line: LogLine },
    #[serde(rename_all = "camelCase")]
    Activity { id: SessionId, entry: ActivityEntry },
    #[serde(rename_all = "camelCase")]
    PromptOpened { prompt: PromptRequest },
    #[serde(rename_all = "camelCase")]
    PromptClosed { id: PromptId },
}

impl EngineEvent {
    /// True for updates that only move a progress bar. These may be dropped in favour
    /// of a newer one for the same job; nothing else may.
    pub fn is_coalescable_progress(&self) -> bool {
        match self {
            EngineEvent::JobUpdate { job } => {
                matches!(job.state, crate::job::JobState::Transferring)
            }
            _ => false,
        }
    }

    pub fn session(&self) -> Option<SessionId> {
        match self {
            EngineEvent::SessionOpened { session } => Some(session.id),
            EngineEvent::SessionState { id, .. }
            | EngineEvent::SessionClosed { id }
            | EngineEvent::Latency { id, .. }
            | EngineEvent::Log { id, .. }
            | EngineEvent::Activity { id, .. } => Some(*id),
            EngineEvent::Listing { listing } => Some(listing.session),
            EngineEvent::JobUpdate { job } => Some(job.session),
            EngineEvent::PromptOpened { prompt } => Some(prompt.session),
            EngineEvent::QueueStats { .. } | EngineEvent::PromptClosed { .. } => None,
        }
    }
}
