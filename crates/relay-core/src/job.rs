//! The transfer job wire contract.
//!
//! Phase 0 fixed the *shape* the shell renders. Phase 2 supplies what was missing
//! behind it: [`crate::queue`] holds the engine's own job record and the single
//! transition function that owns these states, and [`crate::scheduler`] decides which
//! of them runs. Rust stays authoritative — the frontend projects [`JobSnapshot`] and
//! never invents a transition of its own.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::EngineError;
use crate::interact::ConflictAction;
use crate::model::{Direction, JobId, ServerId, SessionId};
use crate::wire::{Bytes, Order};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum JobState {
    /// Folder jobs only: enumerating children before they can be queued.
    Scanning,
    Queued,
    /// Destination stat, conflict detection, resume verification.
    Preparing,
    #[serde(rename_all = "camelCase")]
    AwaitingPrompt {
        prompt: crate::model::PromptId,
    },
    Transferring,
    /// Content is on the destination but has not been proven correct yet.
    Verifying,
    #[serde(rename_all = "camelCase")]
    Paused {
        reason: PauseReason,
    },
    #[serde(rename_all = "camelCase")]
    Failed {
        error: EngineError,
        attempts: u32,
    },
    #[serde(rename_all = "camelCase")]
    Done {
        at: DateTime<Utc>,
        /// The destination was deliberately left alone — a skip, by answer or policy.
        /// It lives inside `Done` rather than beside it because a skipped job *is*
        /// finished, and two fields that can disagree about that would eventually.
        skipped: bool,
    },
    /// Carries a time for the same reason `Done` does: the Completed tab sorts by it,
    /// and the seven-day prune has nothing to compare a bare unit variant against —
    /// which would leave cancelled jobs in the database for ever.
    #[serde(rename_all = "camelCase")]
    Cancelled {
        at: DateTime<Utc>,
    },
}

impl JobState {
    /// Terminal states survive in the snapshot so a UI remount cannot lose an outcome.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            JobState::Done { .. } | JobState::Cancelled { .. } | JobState::Failed { .. }
        )
    }

    pub fn occupies_transfer_slot(&self) -> bool {
        matches!(self, JobState::Transferring | JobState::Verifying)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum PauseReason {
    /// A person pressed pause.
    User,
    /// The session went away; the scheduler resumes these on reconnect.
    SessionDown,
    /// The global concurrency limit dropped below the number of running jobs.
    Throttled,
    /// The process stopped while this was running. Recovery cannot know whether the
    /// bytes in flight reached the disk, so the job waits to be verified rather than
    /// being restarted or trusted.
    Restarted,
}

/// Whether a job moves bytes itself or owns others that do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum JobKind {
    File,
    /// A recursive transfer. Its progress is the sum of its children's and it holds no
    /// connection of its own, so it never spends one of the concurrency slots.
    Folder,
}

/// What the queue drawer and the flow gutter render. Progress is coalesced before it
/// reaches this type: phase 5 budgets ten updates per second per job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshot {
    pub id: JobId,
    pub session: SessionId,
    pub server_id: ServerId,
    pub kind: JobKind,
    pub direction: Direction,
    pub remote_path: String,
    pub local_path: PathBuf,
    /// `None` while a folder job is still scanning, or when the server withholds size.
    pub size: Option<Bytes>,
    pub transferred: Bytes,
    pub state: JobState,
    pub order: Order,
    /// Bytes per second over the recent window; `None` until there is a window.
    pub speed_bps: Option<Bytes>,
    pub eta_secs: Option<u32>,
    /// Attempts spent, including the running one. The drawer reads this with
    /// `retry_at` to say "retrying" instead of an unexplained pause.
    pub attempts: u32,
    /// When a job requeued by the scheduler's backoff becomes eligible again.
    pub retry_at: Option<DateTime<Utc>>,
    /// Set once "apply to remaining" propagates a decision onto later jobs.
    pub conflict_policy: Option<ConflictAction>,
    /// Folder jobs own their children; the drawer nests them under the parent.
    pub parent: Option<JobId>,
    pub started_at: Option<DateTime<Utc>>,
}

impl JobSnapshot {
    pub fn percent(&self) -> Option<f32> {
        match self.size {
            Some(Bytes::ZERO) => Some(100.0),
            Some(size) => Some((self.transferred.get() as f64 / size.get() as f64 * 100.0) as f32),
            None => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct QueueStats {
    pub active: u32,
    pub queued: u32,
    pub failed: u32,
    pub done: u32,
    pub speed_bps: Bytes,
}

/// Queue commands the interface can issue. Phase 2 implements the scheduler behind
/// them; phase 0 answers the ones the demo engine can honestly perform and returns
/// `Unsupported` for the rest rather than pretending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum QueueOp {
    #[serde(rename_all = "camelCase")]
    Cancel {
        job: JobId,
    },
    #[serde(rename_all = "camelCase")]
    Retry {
        job: JobId,
    },
    #[serde(rename_all = "camelCase")]
    Pause {
        job: JobId,
    },
    #[serde(rename_all = "camelCase")]
    Resume {
        job: JobId,
    },
    PauseAll,
    ResumeAll,
    /// Move `job` immediately after `after`, or to the front when `after` is `None`.
    #[serde(rename_all = "camelCase")]
    Reorder {
        job: JobId,
        after: Option<JobId>,
    },
    ClearCompleted,
}
