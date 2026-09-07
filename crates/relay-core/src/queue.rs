//! What the queue knows about a job, and the only function allowed to change it.
//!
//! Phase 1 had no queue: a transfer was a task, its state lived in the task's local
//! variables, and the shell learned about it from whatever `JobUpdate` the task
//! happened to send. That works for one file and stops working the moment a job can
//! be paused, retried, reordered, resumed after a restart, or owned by a parent
//! folder — all of which need the job to exist somewhere that outlives its task.
//!
//! [`Job`] is that record and [`advance`] is the only thing permitted to move it
//! between states. Everything else — the scheduler, the session actors, the SQLite
//! worker, the shell — proposes a [`JobEvent`] and reads the result. Concentrating
//! the rules in one function is what makes them testable: the table at the bottom of
//! this file walks every (state, event) pair, so a transition nobody thought about is
//! a failing test rather than a job wedged in a state with no way out.
//!
//! **A rejected event is not an error.** Two things racing is normal — a cancellation
//! arriving while a transfer is finishing, a pause landing just after a failure — and
//! exactly one of them should win. [`advance`] returns `false` for the loser and
//! leaves the job alone. Nothing panics, and nothing is logged, because nothing went
//! wrong.
//!
//! The distinction this module exists to protect is **displayed progress versus
//! durable progress**. `transferred` is what the progress bar reads and is allowed to
//! be optimistic; [`ResumeRecord::checkpoint`] is what a resume is allowed to trust,
//! and it only ever moves after bytes are proven durable. They are separate fields
//! because they answer different questions, and conflating them is how a resume
//! splices two files together.

use std::path::PathBuf;

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::EngineError;
use crate::interact::ConflictAction;
use crate::job::{JobKind, JobSnapshot, JobState, PauseReason};
use crate::model::{Direction, FileFacts, JobId, PromptId, ServerId, SessionId};
use crate::wire::{Bytes, Order};

/// How many times the scheduler will run a job before handing it to the user.
///
/// Counts attempts, not retries: a job that fails three times with a network error
/// has been tried three times and stops. Only [`EngineError::is_retryable`] failures
/// consume an attempt this way; anything touching trust, identity or integrity fails
/// on the spot, because retrying it either cannot succeed or would paper over a real
/// mismatch.
pub const MAX_ATTEMPTS: u32 = 3;

/// Space left between adjacent queue positions.
///
/// Reordering rewrites one job's `order` to a value between its new neighbours, so a
/// drag costs one row instead of renumbering the queue. The gap is exhausted only
/// after ten insertions between the same pair, and [`renumber`] handles that case.
pub const ORDER_STRIDE: i64 = 1024;

/// Groups the jobs one user gesture created, so "apply to remaining" has a scope and
/// a re-sent enqueue can be recognised as the same request.
pub type BatchId = Uuid;

/// How far finalisation got, so a crash in the middle of it can be reconciled instead
/// of guessed at.
///
/// The dangerous window is between renaming the temporary file onto the destination
/// and recording that the job is done. A process that dies there leaves a complete,
/// correct destination and a job that still says `Transferring`. Recovery must verify
/// the destination rather than assume either outcome — re-running the transfer would
/// overwrite good content, and marking it done without looking would claim a success
/// that may not have happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Finalization {
    /// Bytes are still going into the temporary file.
    Pending,
    /// The rename is about to be attempted. Written before it, never after.
    Intended,
    /// The destination holds the finished content.
    Renamed,
}

/// Everything that must line up before a partial file may be continued rather than
/// started again.
///
/// The single rule this type exists to enforce: **a suffix is not ownership and a size
/// is not identity.** A `.relaypart` file next to the destination proves nothing about
/// who wrote it or what is in it. This record — created before the first byte is
/// written — is what proves both, and resume is offered only when every field in it
/// still checks out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeRecord {
    /// The source as it was when the partial was created. Re-read before resuming: if
    /// these facts moved, the remaining bytes belong to a different file than the ones
    /// already written, and splicing them produces a file that never existed.
    pub source: FileFacts,
    /// The Relay-owned partial. Ownership is established by this record existing, not
    /// by the path's shape.
    pub temporary_path: String,
    /// Bytes proven durable — flushed and fsynced locally, or read back remotely.
    /// Never advanced from a write acknowledgement alone.
    pub checkpoint: Bytes,
    /// SHA-256 of exactly the first `checkpoint` bytes. Verified against the partial
    /// *and* the source before any resume, which is what distinguishes this from the
    /// size comparison that would let two same-length files pass.
    pub prefix_sha256: String,
    /// Whole-file digest, once the transfer has one to compare against.
    pub final_sha256: Option<String>,
    pub finalization: Finalization,
}

impl ResumeRecord {
    /// A fresh record for a transfer that has written nothing yet.
    ///
    /// The empty prefix hashes to SHA-256 of no bytes, which is a real digest of a
    /// real (empty) prefix rather than a placeholder — so the verification path has
    /// no special case for "nothing written yet".
    pub fn new(source: FileFacts, temporary_path: impl Into<String>) -> Self {
        Self {
            source,
            temporary_path: temporary_path.into(),
            checkpoint: Bytes::ZERO,
            prefix_sha256: EMPTY_SHA256.to_string(),
            final_sha256: None,
            finalization: Finalization::Pending,
        }
    }
}

/// SHA-256 of the empty input.
pub const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// One row of the queue.
///
/// Split from [`JobSnapshot`] on purpose. The snapshot is the wire contract — what
/// the drawer draws, generated into TypeScript. This is the engine's own record, and
/// it holds things the interface has no business seeing: digests, the owned temporary
/// path, the deduplication keys, the backoff clock. Sending them would widen the IPC
/// surface for no one's benefit, and `resume` in particular is a safety mechanism
/// that must never be steerable from the shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: JobId,
    pub session: SessionId,
    pub server_id: ServerId,
    pub kind: JobKind,
    pub direction: Direction,
    pub remote_path: String,
    pub local_path: PathBuf,
    /// `None` while a folder is still being walked, or when the server withholds it.
    pub size: Option<Bytes>,
    /// Displayed progress. Optimistic by design; see the module comment.
    pub transferred: Bytes,
    pub state: JobState,
    pub order: Order,
    /// Completed attempts, including the one that is running.
    pub attempts: u32,
    /// When the scheduler may pick this job up again after a retryable failure.
    /// `None` means now.
    pub retry_at: Option<DateTime<Utc>>,
    /// Set once "apply to remaining" propagates a decision from a sibling.
    pub conflict_policy: Option<ConflictAction>,
    /// What was decided for *this* job specifically, whether by prompt or policy.
    pub chosen: Option<ConflictAction>,
    /// Folder jobs own their children.
    pub parent: Option<JobId>,
    /// Which gesture created this job, and which item of it. Together these are unique,
    /// so re-sending an enqueue after an uncertain IPC delivery finds the existing job
    /// instead of creating a second one.
    pub batch: BatchId,
    pub item: String,
    pub resume: Option<ResumeRecord>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    /// Recent throughput, recomputed by the scheduler rather than stored durably.
    #[serde(skip)]
    pub speed_bps: Option<Bytes>,
}

/// One item of an enqueue request, before the queue owns it.
///
/// Separate from [`Job`] because a caller supplies exactly these fields and the queue
/// supplies the rest: the id, the position, the batch, and every piece of state. A
/// caller that could set `attempts` or `resume` could authorise a resume from outside
/// the engine, which is precisely what must not be possible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobSpec {
    pub session: SessionId,
    pub server_id: ServerId,
    pub kind: JobKind,
    pub direction: Direction,
    pub remote_path: String,
    pub local_path: PathBuf,
    /// Known up front for a file whose source was already stat'd; `None` otherwise.
    pub size: Option<Bytes>,
    pub parent: Option<JobId>,
    /// Stable within the batch, so re-sending the request finds this job again rather
    /// than making a second one. A path is the obvious choice and is what the enqueue
    /// path uses.
    pub item: String,
}

impl Job {
    /// A job as the queue first owns it: not started, nothing decided, no attempts.
    pub fn new(id: JobId, order: Order, batch: BatchId, spec: JobSpec) -> Self {
        Self {
            id,
            session: spec.session,
            server_id: spec.server_id,
            kind: spec.kind,
            direction: spec.direction,
            remote_path: spec.remote_path,
            local_path: spec.local_path,
            size: spec.size,
            transferred: Bytes::ZERO,
            // A folder has to be walked before anything in it can be queued.
            state: match spec.kind {
                JobKind::File => JobState::Queued,
                JobKind::Folder => JobState::Scanning,
            },
            order,
            attempts: 0,
            retry_at: None,
            conflict_policy: None,
            chosen: None,
            parent: spec.parent,
            batch,
            item: spec.item,
            resume: None,
            created_at: Utc::now(),
            started_at: None,
            speed_bps: None,
        }
    }

    /// Whether the scheduler may dispatch this job right now.
    ///
    /// A job waiting out its backoff is `Queued` rather than in a state of its own:
    /// it *is* queued, it simply is not eligible yet, and inventing a state for the
    /// gap would mean the drawer had to explain the difference.
    pub fn is_eligible(&self, now: DateTime<Utc>) -> bool {
        matches!(self.state, JobState::Queued) && self.retry_at.is_none_or(|at| at <= now)
    }

    /// Whether this job is holding one of the concurrency slider's slots.
    ///
    /// A folder job is `Transferring` while its children move, but it is a container:
    /// it owns no lane and no bytes of its own, so counting it would silently spend a
    /// slot on bookkeeping.
    pub fn holds_slot(&self) -> bool {
        self.kind == JobKind::File && self.state.occupies_transfer_slot()
    }

    /// Bytes still to move, when that is known.
    pub fn remaining(&self) -> Option<Bytes> {
        self.size
            .map(|size| Bytes(size.get().saturating_sub(self.transferred.get())))
    }

    /// The offset a resume would start from. Zero unless a verified checkpoint exists,
    /// and deliberately not derived from `transferred`.
    pub fn verified_offset(&self) -> Bytes {
        match &self.resume {
            Some(record) if self.chosen == Some(ConflictAction::Resume) => record.checkpoint,
            _ => Bytes::ZERO,
        }
    }

    /// What the drawer and the flow gutter render.
    pub fn snapshot(&self) -> JobSnapshot {
        JobSnapshot {
            id: self.id,
            session: self.session,
            server_id: self.server_id,
            kind: self.kind,
            direction: self.direction,
            remote_path: self.remote_path.clone(),
            local_path: self.local_path.clone(),
            size: self.size,
            transferred: self.transferred,
            state: self.state.clone(),
            order: self.order,
            speed_bps: self.speed_bps,
            eta_secs: eta(self.remaining(), self.speed_bps),
            attempts: self.attempts,
            retry_at: self.retry_at,
            conflict_policy: self.conflict_policy,
            parent: self.parent,
            started_at: self.started_at,
        }
    }
}

/// Seconds left at the current rate.
///
/// `None` below a floor rather than a very large number: dividing by a rate that is
/// almost zero produces "4 days remaining" on a transfer that is briefly stalled,
/// which is worse than admitting we do not know. The design draws an em dash here.
pub fn eta(remaining: Option<Bytes>, speed: Option<Bytes>) -> Option<u32> {
    const FLOOR_BPS: u64 = 5 * 1024;
    match (remaining, speed) {
        (Some(left), Some(rate)) if rate.get() >= FLOOR_BPS => {
            Some((left.get() / rate.get()).min(u64::from(u32::MAX)) as u32)
        }
        _ => None,
    }
}

/// Something that happened to a job. Proposed by whoever observed it; only [`advance`]
/// decides whether it counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobEvent {
    /// A folder job finished walking. Carries how many children were enqueued, because
    /// a folder with nothing in it is finished rather than waiting forever.
    Scanned {
        children: u32,
    },
    /// The scheduler picked this job up and preparation has begun.
    Prepare,
    /// Preparation found a conflict and opened the sheet.
    Ask {
        prompt: PromptId,
    },
    /// The sheet was answered.
    Answered {
        action: ConflictAction,
    },
    /// The destination was left alone deliberately — by an answered sheet, a settings
    /// default, or a policy inherited from a sibling.
    Skip,
    /// Bytes are moving. `resume_from` is a *verified* offset; zero for a fresh start.
    Start {
        resume_from: Bytes,
    },
    /// Displayed progress. Never advances a checkpoint.
    Progress {
        transferred: Bytes,
        speed: Option<Bytes>,
    },
    /// The content is at the destination but has not been proven correct yet.
    Verify,
    /// Proven, and finalised.
    Complete,
    Pause {
        reason: PauseReason,
    },
    /// Back into the queue from `Paused`.
    Unpause,
    Fail {
        error: EngineError,
    },
    Cancel,
    /// A person pressed retry on a failed job, which clears the attempt count. The
    /// scheduler's own backoff never uses this — it would make the cap unreachable.
    Retry,
    /// A sibling's "apply to remaining" reached this job.
    AdoptPolicy {
        action: ConflictAction,
    },
    /// Size learned after the fact — a folder's total, or a stat that arrived late.
    Size {
        size: Bytes,
    },
}

/// Move `job` in response to `event`, returning whether it applied.
///
/// `false` means the event was not legal from the job's current state. See the module
/// comment: that is a race being resolved, not a fault.
pub fn advance(job: &mut Job, event: JobEvent) -> bool {
    use JobState as S;

    match (&job.state, event) {
        // ---------------------------------------------------------------- scanning
        (S::Scanning, JobEvent::Scanned { children }) => {
            // An empty folder is done. Its children would otherwise never arrive to
            // complete it, and the drawer would show a folder stuck at "scanning".
            job.state = if children == 0 {
                S::Done {
                    at: Utc::now(),
                    skipped: false,
                }
            } else {
                S::Transferring
            };
            true
        }

        // --------------------------------------------------------------- preparing
        (S::Queued, JobEvent::Prepare) => {
            job.state = S::Preparing;
            job.retry_at = None;
            true
        }
        (S::Preparing, JobEvent::Ask { prompt }) => {
            job.state = S::AwaitingPrompt { prompt };
            true
        }
        (S::AwaitingPrompt { .. }, JobEvent::Answered { action }) => {
            job.chosen = Some(action);
            job.state = match action {
                ConflictAction::Skip => S::Done {
                    at: Utc::now(),
                    skipped: true,
                },
                _ => S::Preparing,
            };
            true
        }
        (S::Preparing, JobEvent::Skip) => {
            job.chosen = Some(ConflictAction::Skip);
            job.state = S::Done {
                at: Utc::now(),
                skipped: true,
            };
            true
        }

        // ------------------------------------------------------------- transferring
        (S::Preparing, JobEvent::Start { resume_from }) => {
            job.state = S::Transferring;
            job.attempts += 1;
            job.started_at.get_or_insert_with(Utc::now);
            // Displayed progress starts where the resume does, so the bar does not
            // rewind to zero on a job that is half done.
            job.transferred = resume_from;
            true
        }
        (S::Transferring, JobEvent::Progress { transferred, speed }) => {
            job.transferred = transferred;
            job.speed_bps = speed;
            true
        }
        (S::Transferring, JobEvent::Verify) => {
            job.state = S::Verifying;
            job.speed_bps = None;
            true
        }
        (S::Transferring | S::Verifying, JobEvent::Complete) => {
            // A job that completes without a known size has moved exactly as many
            // bytes as it moved; recording that makes the finished row honest.
            job.size.get_or_insert(job.transferred);
            job.speed_bps = None;
            job.state = S::Done {
                at: Utc::now(),
                skipped: false,
            };
            true
        }

        // -------------------------------------------------------------------- pause
        (
            S::Scanning
            | S::Queued
            | S::Preparing
            | S::AwaitingPrompt { .. }
            | S::Transferring
            | S::Verifying,
            JobEvent::Pause { reason },
        ) => {
            job.speed_bps = None;
            job.state = S::Paused { reason };
            true
        }
        (S::Paused { .. }, JobEvent::Unpause) => {
            job.state = S::Queued;
            job.retry_at = None;
            true
        }

        // ------------------------------------------------------------------ failure
        (
            S::Scanning
            | S::Queued
            | S::Preparing
            | S::AwaitingPrompt { .. }
            | S::Transferring
            | S::Verifying,
            JobEvent::Fail { error },
        ) => {
            job.speed_bps = None;
            // A failure before the transfer started still counts as an attempt, or a
            // job that fails during preparation every time would retry forever.
            if !matches!(job.state, S::Transferring | S::Verifying) {
                job.attempts += 1;
            }
            if error.is_retryable() && job.attempts < MAX_ATTEMPTS {
                job.retry_at = Some(Utc::now() + backoff(job.attempts));
                job.state = S::Queued;
            } else {
                job.state = S::Failed {
                    error,
                    attempts: job.attempts,
                };
            }
            true
        }
        (S::Failed { .. }, JobEvent::Retry) => {
            job.attempts = 0;
            job.retry_at = None;
            job.speed_bps = None;
            job.state = S::Queued;
            true
        }

        // --------------------------------------------------------------- cancelling
        // Cancelling a finished job is a no-op below, not an error: the row is simply
        // already gone from the active list by the time the click lands.
        (
            S::Scanning
            | S::Queued
            | S::Preparing
            | S::AwaitingPrompt { .. }
            | S::Transferring
            | S::Verifying
            | S::Paused { .. },
            JobEvent::Cancel,
        ) => {
            job.speed_bps = None;
            job.state = S::Cancelled { at: Utc::now() };
            true
        }

        // ------------------------------------------------------------- side effects
        // These carry no state change, so they are legal wherever the fact they
        // record can still matter — which is anywhere the job is not finished.
        (state, JobEvent::AdoptPolicy { action }) if !state.is_terminal() => {
            job.conflict_policy = Some(action);
            true
        }
        (state, JobEvent::Size { size }) if !state.is_terminal() => {
            job.size = Some(size);
            true
        }

        _ => false,
    }
}

/// How long to wait before the next attempt: 1s, 2s, 4s.
///
/// Exponential rather than fixed because the failures this retries are congestion and
/// timeouts, and hammering a server that is already struggling makes it worse.
pub fn backoff(attempts: u32) -> TimeDelta {
    /// Where the doubling stops. [`MAX_ATTEMPTS`] means it is never reached in
    /// practice, but a shift of 64 is undefined and this makes it unreachable.
    const CAP: u32 = 6;
    TimeDelta::seconds(1i64 << attempts.saturating_sub(1).min(CAP))
}

/// Assign fresh, evenly spaced positions.
///
/// Reordering normally rewrites one row, but repeatedly dropping jobs between the same
/// two neighbours halves the gap each time and eventually runs out of integers. When
/// that happens the queue is spread out again — rare, and cheap when it is not.
pub fn renumber(jobs: &mut [&mut Job]) {
    for (index, job) in jobs.iter_mut().enumerate() {
        job.order = Order((index as i64 + 1) * ORDER_STRIDE);
    }
}

/// A position between two neighbours, or `None` when the gap is exhausted and the
/// caller must [`renumber`].
pub fn between(before: Option<Order>, after: Option<Order>) -> Option<Order> {
    match (before, after) {
        (None, None) => Some(Order(ORDER_STRIDE)),
        (Some(before), None) => Some(Order(before.get().saturating_add(ORDER_STRIDE))),
        (None, Some(after)) => Some(Order(after.get().saturating_sub(ORDER_STRIDE))),
        (Some(before), Some(after)) => {
            let gap = after.get() - before.get();
            (gap > 1).then(|| Order(before.get() + gap / 2))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: JobKind) -> JobSpec {
        JobSpec {
            session: Uuid::new_v4(),
            server_id: Uuid::new_v4(),
            kind,
            direction: Direction::Down,
            remote_path: "/remote/file".into(),
            local_path: PathBuf::from("/local/file"),
            size: None,
            parent: None,
            item: "/remote/file".into(),
        }
    }

    fn job() -> Job {
        Job::new(
            Uuid::new_v4(),
            Order(ORDER_STRIDE),
            Uuid::new_v4(),
            spec(JobKind::File),
        )
    }

    fn folder() -> Job {
        Job::new(
            Uuid::new_v4(),
            Order(ORDER_STRIDE),
            Uuid::new_v4(),
            spec(JobKind::Folder),
        )
    }

    fn in_state(state: JobState) -> Job {
        let mut job = job();
        job.state = state;
        job
    }

    fn network() -> EngineError {
        EngineError::network("connection reset")
    }

    /// Every state a job can be in, once each.
    fn all_states() -> Vec<JobState> {
        vec![
            JobState::Scanning,
            JobState::Queued,
            JobState::Preparing,
            JobState::AwaitingPrompt {
                prompt: Uuid::new_v4(),
            },
            JobState::Transferring,
            JobState::Verifying,
            JobState::Paused {
                reason: PauseReason::User,
            },
            JobState::Failed {
                error: network(),
                attempts: 3,
            },
            JobState::Done {
                at: Utc::now(),
                skipped: false,
            },
            JobState::Cancelled { at: Utc::now() },
        ]
    }

    /// Every event, once each.
    fn all_events() -> Vec<JobEvent> {
        vec![
            JobEvent::Scanned { children: 2 },
            JobEvent::Prepare,
            JobEvent::Ask {
                prompt: Uuid::new_v4(),
            },
            JobEvent::Answered {
                action: ConflictAction::Overwrite,
            },
            JobEvent::Skip,
            JobEvent::Start {
                resume_from: Bytes::ZERO,
            },
            JobEvent::Progress {
                transferred: Bytes(1),
                speed: None,
            },
            JobEvent::Verify,
            JobEvent::Complete,
            JobEvent::Pause {
                reason: PauseReason::User,
            },
            JobEvent::Unpause,
            JobEvent::Fail { error: network() },
            JobEvent::Cancel,
            JobEvent::Retry,
            JobEvent::AdoptPolicy {
                action: ConflictAction::Overwrite,
            },
            JobEvent::Size { size: Bytes(10) },
        ]
    }

    /// The point of a single transition function: no pair can wedge a job, and no
    /// pair can move it somewhere the rules do not allow.
    #[test]
    fn every_state_event_pair_is_decided() {
        for state in all_states() {
            for event in all_events() {
                let mut job = in_state(state.clone());
                let before = job.state.clone();
                let applied = advance(&mut job, event.clone());

                if !applied {
                    assert_eq!(
                        job.state, before,
                        "{state:?} + {event:?} was rejected but changed the job anyway"
                    );
                }
                // A terminal job is finished. The only thing that may move it is a
                // person pressing retry on a failure.
                if before.is_terminal() && applied {
                    assert!(
                        matches!(
                            (&before, &event),
                            (JobState::Failed { .. }, JobEvent::Retry)
                        ),
                        "{before:?} is terminal but {event:?} moved it"
                    );
                }
            }
        }
    }

    #[test]
    fn a_transfer_runs_start_to_finish() {
        let mut job = job();
        assert!(advance(&mut job, JobEvent::Prepare));
        assert!(advance(
            &mut job,
            JobEvent::Start {
                resume_from: Bytes::ZERO
            }
        ));
        assert_eq!(job.attempts, 1);
        assert!(job.started_at.is_some());
        assert!(advance(
            &mut job,
            JobEvent::Progress {
                transferred: Bytes(512),
                speed: Some(Bytes(256)),
            }
        ));
        assert!(advance(&mut job, JobEvent::Verify));
        assert_eq!(job.speed_bps, None, "a verifying job is not moving bytes");
        assert!(advance(&mut job, JobEvent::Complete));
        assert!(matches!(job.state, JobState::Done { skipped: false, .. }));
        assert_eq!(job.size, Some(Bytes(512)), "an unknown size is learned");
    }

    #[test]
    fn a_retryable_failure_goes_back_to_the_queue_until_the_attempts_run_out() {
        let mut job = job();
        for attempt in 1..MAX_ATTEMPTS {
            advance(&mut job, JobEvent::Prepare);
            advance(
                &mut job,
                JobEvent::Start {
                    resume_from: Bytes::ZERO,
                },
            );
            assert!(advance(&mut job, JobEvent::Fail { error: network() }));
            assert!(
                matches!(job.state, JobState::Queued),
                "attempt {attempt} should have been requeued"
            );
            assert_eq!(job.attempts, attempt);
            assert!(job.retry_at.is_some(), "a requeued job waits its backoff");
            assert!(
                !job.is_eligible(Utc::now()),
                "it is not eligible before the backoff elapses"
            );
            assert!(job.is_eligible(Utc::now() + TimeDelta::minutes(1)));
        }

        advance(&mut job, JobEvent::Prepare);
        advance(
            &mut job,
            JobEvent::Start {
                resume_from: Bytes::ZERO,
            },
        );
        advance(&mut job, JobEvent::Fail { error: network() });
        assert!(
            matches!(job.state, JobState::Failed { attempts, .. } if attempts == MAX_ATTEMPTS),
            "the cap is reached, so it stops: {:?}",
            job.state
        );
    }

    #[test]
    fn failures_that_need_a_person_are_never_retried_automatically() {
        for error in [
            EngineError::Auth {
                message: "denied".into(),
            },
            EngineError::PermissionDenied { path: "/x".into() },
            EngineError::SourceChanged { path: "/x".into() },
            EngineError::IntegrityMismatch {
                expected: "a".into(),
                actual: "b".into(),
            },
            EngineError::LocalIo {
                path: "/x".into(),
                message: "No space left on device".into(),
            },
        ] {
            let mut job = in_state(JobState::Transferring);
            advance(
                &mut job,
                JobEvent::Fail {
                    error: error.clone(),
                },
            );
            assert!(
                matches!(job.state, JobState::Failed { .. }),
                "{error:?} should fail immediately, not requeue"
            );
            assert!(job.retry_at.is_none());
        }
    }

    /// The bug this prevents: a job that fails while preparing never reaches
    /// `Transferring`, so if only transfers counted attempts it would retry forever.
    #[test]
    fn a_failure_before_the_first_byte_still_spends_an_attempt() {
        let mut job = in_state(JobState::Preparing);
        advance(&mut job, JobEvent::Fail { error: network() });
        assert_eq!(job.attempts, 1);
    }

    #[test]
    fn retrying_by_hand_clears_the_attempts_but_the_scheduler_never_can() {
        let mut job = in_state(JobState::Failed {
            error: network(),
            attempts: MAX_ATTEMPTS,
        });
        assert!(advance(&mut job, JobEvent::Retry));
        assert_eq!(job.attempts, 0);
        assert!(matches!(job.state, JobState::Queued));
        assert!(job.is_eligible(Utc::now()), "a hand retry does not wait");
    }

    #[test]
    fn skipping_leaves_the_destination_alone_and_says_so() {
        let mut job = in_state(JobState::AwaitingPrompt {
            prompt: Uuid::new_v4(),
        });
        assert!(advance(
            &mut job,
            JobEvent::Answered {
                action: ConflictAction::Skip
            }
        ));
        assert!(matches!(job.state, JobState::Done { skipped: true, .. }));
        assert_eq!(job.chosen, Some(ConflictAction::Skip));
    }

    #[test]
    fn an_empty_folder_is_finished_rather_than_waiting_for_children() {
        let mut empty = folder();
        assert!(advance(&mut empty, JobEvent::Scanned { children: 0 }));
        assert!(matches!(empty.state, JobState::Done { .. }));

        let mut full = folder();
        advance(&mut full, JobEvent::Scanned { children: 3 });
        assert!(matches!(full.state, JobState::Transferring));
    }

    /// A folder is `Transferring` while its children move, but it owns no lane. If it
    /// counted, a queue of folders would spend every slot on bookkeeping and never
    /// transfer anything.
    #[test]
    fn a_folder_never_occupies_a_transfer_slot() {
        let mut parent = folder();
        advance(&mut parent, JobEvent::Scanned { children: 3 });
        assert!(!parent.holds_slot());

        let file = in_state(JobState::Transferring);
        assert!(file.holds_slot());
    }

    /// The distinction the whole module exists for: the bar may run ahead, the
    /// checkpoint may not.
    #[test]
    fn displayed_progress_never_becomes_a_resume_offset() {
        let mut job = in_state(JobState::Transferring);
        job.resume = Some(ResumeRecord::new(
            FileFacts {
                path: "/remote/file".into(),
                size: Bytes(1000),
                modified: None,
                digest: None,
            },
            "/local/.file.relaypart",
        ));
        advance(
            &mut job,
            JobEvent::Progress {
                transferred: Bytes(900),
                speed: Some(Bytes(100)),
            },
        );

        assert_eq!(job.transferred, Bytes(900));
        assert_eq!(
            job.resume.as_ref().unwrap().checkpoint,
            Bytes::ZERO,
            "progress must not advance a checkpoint"
        );
        assert_eq!(
            job.verified_offset(),
            Bytes::ZERO,
            "and no resume was authorised, so the offset is zero"
        );
    }

    #[test]
    fn a_resume_offset_needs_both_a_record_and_a_decision() {
        let mut job = in_state(JobState::Preparing);
        job.resume = Some(ResumeRecord {
            checkpoint: Bytes(400),
            ..ResumeRecord::new(
                FileFacts {
                    path: "/remote/file".into(),
                    size: Bytes(1000),
                    modified: None,
                    digest: None,
                },
                "/local/.file.relaypart",
            )
        });
        assert_eq!(
            job.verified_offset(),
            Bytes::ZERO,
            "a checkpoint alone does not authorise resuming"
        );

        job.chosen = Some(ConflictAction::Resume);
        assert_eq!(job.verified_offset(), Bytes(400));
    }

    #[test]
    fn backoff_grows_and_stops_growing() {
        assert_eq!(backoff(1), TimeDelta::seconds(1));
        assert_eq!(backoff(2), TimeDelta::seconds(2));
        assert_eq!(backoff(3), TimeDelta::seconds(4));
        assert!(
            backoff(60) <= TimeDelta::seconds(64),
            "no overflow, no panic"
        );
    }

    #[test]
    fn eta_admits_when_it_does_not_know() {
        assert_eq!(eta(Some(Bytes(1000)), Some(Bytes(100))), None, "too slow");
        assert_eq!(eta(None, Some(Bytes(1024 * 1024))), None, "no total");
        assert_eq!(eta(Some(Bytes(1024 * 1024)), None), None, "no rate");
        assert_eq!(
            eta(Some(Bytes(1024 * 1024)), Some(Bytes(1024 * 128))),
            Some(8)
        );
    }

    #[test]
    fn reordering_finds_a_position_until_the_gap_runs_out() {
        assert_eq!(between(None, None), Some(Order(ORDER_STRIDE)));
        assert_eq!(between(Some(Order(1024)), None), Some(Order(2048)));
        assert_eq!(between(None, Some(Order(1024))), Some(Order(0)));
        assert_eq!(
            between(Some(Order(1024)), Some(Order(2048))),
            Some(Order(1536))
        );
        assert_eq!(
            between(Some(Order(1024)), Some(Order(1025))),
            None,
            "adjacent positions have nowhere between them"
        );
    }

    #[test]
    fn renumbering_spreads_the_queue_back_out_in_place() {
        let (mut a, mut b, mut c) = (job(), job(), job());
        a.order = Order(1024);
        b.order = Order(1025);
        c.order = Order(1026);
        renumber(&mut [&mut a, &mut b, &mut c]);
        assert_eq!(
            (a.order, b.order, c.order),
            (Order(1024), Order(2048), Order(3072))
        );
        assert!(between(Some(a.order), Some(b.order)).is_some());
    }
}
