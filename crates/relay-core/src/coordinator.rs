//! The authoritative projection of engine state, and the ordered stream that keeps a
//! UI in step with it.
//!
//! The recovery protocol, which the frontend bridge implements on the other side:
//!
//! 1. `subscribe()` first, and buffer whatever arrives.
//! 2. `snapshot(id)` second. It is taken under the same lock that assigns sequence
//!    numbers, so it is exactly consistent at its own `seq` watermark.
//! 3. Apply the snapshot, then replay only buffered envelopes with `seq > watermark`.
//! 4. On a gap, a closed channel, a changed `epoch`, or a remount: go back to step 1.
//!
//! Overflow is not silently absorbed. A subscriber that cannot keep up is dropped,
//! its channel closes, and the bridge resnapshots — which is recoverable. Skipping an
//! update to make room is not.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use specta::Type;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::events::{EngineEvent, ListingSnapshot, LogLine};
use crate::interact::PromptRequest;
use crate::job::{JobSnapshot, JobState, QueueStats};
use crate::model::{Session, SessionId};
use crate::wire::Seq;

/// Envelopes a subscriber may fall behind by before it is dropped and told to
/// resnapshot. Progress is already coalesced, so reaching this means a stalled UI.
pub const SUBSCRIBER_BUFFER: usize = 512;
/// Per-session protocol log retained in memory, fetched on demand rather than pushed.
pub const MAX_LOG_LINES: usize = 500;
/// Finished jobs kept for the drawer's Completed tab.
pub const MAX_FINISHED_JOBS: usize = 500;

pub type SubscriptionId = Uuid;

/// One sequenced update. `epoch` changes whenever the engine restarts, which
/// invalidates every watermark a client is holding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EngineEnvelope {
    pub epoch: Uuid,
    pub seq: Seq,
    pub update: EngineEvent,
}

/// A consistent view of everything the UI renders. Logs are deliberately absent:
/// they are bounded separately and fetched on demand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EngineSnapshot {
    pub epoch: Uuid,
    /// Apply only buffered envelopes with a strictly greater `seq`.
    pub seq: Seq,
    pub sessions: Vec<Session>,
    pub jobs: Vec<JobSnapshot>,
    pub stats: QueueStats,
    /// Unanswered questions, so a remount reopens the sheet instead of losing it.
    pub prompts: Vec<PromptRequest>,
    pub listings: Vec<ListingSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SnapshotError {
    /// The subscription was dropped (overflow, unsubscribe, or engine restart).
    /// Subscribe again and retry; do not reuse the old watermark.
    UnknownSubscription,
}

pub struct Subscription {
    pub id: SubscriptionId,
    pub rx: mpsc::Receiver<EngineEnvelope>,
}

struct Inner {
    seq: Seq,
    sessions: HashMap<SessionId, Session>,
    jobs: HashMap<Uuid, JobSnapshot>,
    finished_order: VecDeque<Uuid>,
    stats: QueueStats,
    listings: HashMap<SessionId, ListingSnapshot>,
    logs: HashMap<SessionId, VecDeque<LogLine>>,
    subscribers: HashMap<SubscriptionId, mpsc::Sender<EngineEnvelope>>,
}

pub struct Coordinator {
    epoch: Uuid,
    inner: Mutex<Inner>,
}

impl Coordinator {
    pub fn new() -> Self {
        Self {
            epoch: Uuid::new_v4(),
            inner: Mutex::new(Inner {
                seq: Seq::ZERO,
                sessions: HashMap::new(),
                jobs: HashMap::new(),
                finished_order: VecDeque::new(),
                stats: QueueStats::default(),
                listings: HashMap::new(),
                logs: HashMap::new(),
                subscribers: HashMap::new(),
            }),
        }
    }

    pub fn epoch(&self) -> Uuid {
        self.epoch
    }

    pub fn subscribe(&self) -> Subscription {
        let (tx, rx) = mpsc::channel(SUBSCRIBER_BUFFER);
        let id = Uuid::new_v4();
        self.lock().subscribers.insert(id, tx);
        Subscription { id, rx }
    }

    pub fn unsubscribe(&self, id: SubscriptionId) {
        self.lock().subscribers.remove(&id);
    }

    pub fn subscriber_count(&self) -> usize {
        self.lock().subscribers.len()
    }

    /// Apply an event to the projection, assign it a sequence number, and fan it out.
    ///
    /// Returns the assigned sequence number, or `None` if the event was dropped as
    /// stale — currently only an out-of-order listing, which must not clobber a newer
    /// directory the user has already navigated to.
    pub fn publish(&self, event: EngineEvent) -> Option<Seq> {
        let mut inner = self.lock();
        if !inner.apply(&event) {
            return None;
        }
        let seq = inner.seq.advance();
        let envelope = EngineEnvelope {
            epoch: self.epoch,
            seq,
            update: event,
        };

        // try_send only: publishing must never await, because it runs inside engine
        // tasks and holds the projection lock.
        let mut dropped = Vec::new();
        for (id, tx) in inner.subscribers.iter() {
            if tx.try_send(envelope.clone()).is_err() {
                dropped.push(*id);
            }
        }
        for id in dropped {
            inner.subscribers.remove(&id);
        }
        Some(envelope.seq)
    }

    pub fn snapshot(&self, subscription: SubscriptionId) -> Result<EngineSnapshot, SnapshotError> {
        self.snapshot_with_prompts(subscription, Vec::new())
    }

    /// Snapshot including the broker's unanswered prompts. Taken under the projection
    /// lock so the watermark and the contents cannot disagree.
    pub fn snapshot_with_prompts(
        &self,
        subscription: SubscriptionId,
        prompts: Vec<PromptRequest>,
    ) -> Result<EngineSnapshot, SnapshotError> {
        let inner = self.lock();
        if !inner.subscribers.contains_key(&subscription) {
            return Err(SnapshotError::UnknownSubscription);
        }

        let mut sessions: Vec<Session> = inner.sessions.values().cloned().collect();
        sessions.sort_by_key(|s| s.id);
        let mut jobs: Vec<JobSnapshot> = inner.jobs.values().cloned().collect();
        jobs.sort_by_key(|j| (j.order, j.id));
        let mut listings: Vec<ListingSnapshot> = inner.listings.values().cloned().collect();
        listings.sort_by_key(|l| l.session);

        Ok(EngineSnapshot {
            epoch: self.epoch,
            seq: inner.seq,
            sessions,
            jobs,
            stats: inner.stats,
            prompts,
            listings,
        })
    }

    /// Bounded per-session log backlog, newest last.
    pub fn logs(&self, session: SessionId, limit: usize) -> Vec<LogLine> {
        let inner = self.lock();
        match inner.logs.get(&session) {
            None => Vec::new(),
            Some(lines) => lines.iter().rev().take(limit).rev().cloned().collect(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("coordinator projection poisoned")
    }
}

impl Default for Coordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl Inner {
    /// Fold an event into the projection. Returns false to drop it as stale.
    fn apply(&mut self, event: &EngineEvent) -> bool {
        match event {
            EngineEvent::SessionOpened { session } => {
                self.sessions.insert(session.id, (**session).clone());
            }
            EngineEvent::SessionState { id, state } => {
                if let Some(session) = self.sessions.get_mut(id) {
                    session.state = state.clone();
                }
            }
            EngineEvent::SessionClosed { id } => {
                self.sessions.remove(id);
                self.listings.remove(id);
                self.logs.remove(id);
            }
            EngineEvent::Latency { id, ms } => {
                if let Some(session) = self.sessions.get_mut(id) {
                    session.latency_ms = Some(*ms);
                }
            }
            EngineEvent::Listing { listing } => {
                if let Some(current) = self.listings.get(&listing.session)
                    && current.request > listing.request
                {
                    return false;
                }
                if let Some(session) = self.sessions.get_mut(&listing.session) {
                    session.remote_path = Some(listing.path.clone());
                }
                self.listings.insert(listing.session, (**listing).clone());
            }
            EngineEvent::JobUpdate { job } => {
                let was_terminal = self
                    .jobs
                    .get(&job.id)
                    .map(|j| j.state.is_terminal())
                    .unwrap_or(false);
                self.jobs.insert(job.id, (**job).clone());
                if job.state.is_terminal() && !was_terminal {
                    self.finished_order.push_back(job.id);
                    self.prune_finished();
                }
            }
            EngineEvent::QueueStats { stats } => self.stats = *stats,
            EngineEvent::Log { id, line } => {
                let buf = self.logs.entry(*id).or_default();
                if buf.len() == MAX_LOG_LINES {
                    buf.pop_front();
                }
                buf.push_back(line.clone());
            }
            // Activity is rendered from the stream; phase 4 adds a retained feed.
            EngineEvent::Activity { .. } => {}
            // Prompt state is owned by the broker, which the snapshot reads directly.
            EngineEvent::PromptOpened { .. } | EngineEvent::PromptClosed { .. } => {}
        }
        true
    }

    /// Keep the Completed tab bounded without ever discarding a live job.
    fn prune_finished(&mut self) {
        while self.finished_order.len() > MAX_FINISHED_JOBS {
            if let Some(id) = self.finished_order.pop_front()
                && let Some(job) = self.jobs.get(&id)
                && matches!(
                    job.state,
                    JobState::Done { .. } | JobState::Cancelled { .. }
                )
            {
                self.jobs.remove(&id);
            }
        }
    }
}

/// Convenience for engine code that has an event to publish and a clock.
pub fn log_line(kind: crate::events::LogKind, line: impl Into<String>) -> LogLine {
    LogLine {
        at: Utc::now(),
        kind,
        line: line.into(),
    }
}
