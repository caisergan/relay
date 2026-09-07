//! One task that decides what runs.
//!
//! Phase 1 let each session queue its own transfers, two at a time. That is not a
//! queue policy, it is a cap, and it cannot answer the questions phase 2 asks: which
//! of nine queued files goes next, what happens when the concurrency slider moves, how
//! a paused job differs from a job whose server went away, what "retry" means. Those
//! are global questions, so there is one global scheduler and the sessions became
//! workers it drives.
//!
//! **The scheduler never blocks on anything slow.** It does not touch the network, it
//! does not wait for a person, and it does not wait for the disk except where a write
//! must be durable before the queue may rely on it. Everything else is a message: a
//! transfer reports through a [`Reporter`], a session's arrival and departure arrive as
//! messages, and the database's slow writes go through
//! [`QueueStore::save_detached`]. A scheduler that awaited a stalled server would stop
//! every other session, which is the exact failure it exists to prevent.
//!
//! **Every state change goes through [`advance`].** The scheduler decides *which* job
//! and *when*; it never decides what a state means. That belongs to [`crate::queue`],
//! and keeping it there is why a race — a cancel arriving as a transfer finishes —
//! resolves the same way here as it does in that module's tests.
//!
//! **The dispatcher is a trait.** [`Dispatcher`] is how a job becomes bytes on a wire.
//! The session actor implements it; the tests implement it with something that
//! finishes on command. Every rule in this module — the caps, the priority, the
//! backoff, the pause semantics — is therefore testable without a server.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::error::{EngineError, Result};
use crate::events::EngineEvent;
use crate::interact::ConflictAction;
use crate::job::{JobKind, JobSnapshot, JobState, PauseReason, QueueOp, QueueStats};
use crate::model::{Direction, JobId, PromptId, SessionId};
use crate::queue::{BatchId, Job, JobEvent, JobSpec, ORDER_STRIDE, advance, between, position};
use crate::store::QueueStore;
use crate::wire::{Bytes, Order};

/// How long a session's throughput is averaged over.
///
/// A cumulative average stops moving after the first minute, so a transfer that
/// stalls goes on reporting a healthy rate. A short trailing window falls to zero,
/// which is the honest answer and the one the ETA needs.
const RATE_WINDOW: Duration = Duration::from_secs(3);
/// Below this the sample span is too short to divide by without inventing precision.
const RATE_MIN_SPAN: Duration = Duration::from_millis(400);
/// How often displayed progress is flushed to the database.
///
/// A write per progress tick would put an fsync in the transfer's hot path for a
/// number that does not have to be exact — and never authorises a resume anyway.
const PROGRESS_FLUSH: Duration = Duration::from_secs(5);
/// Messages buffered before a reporting transfer has to wait.
const INBOX: usize = 256;

/// Everything the scheduler is told.
enum Msg {
    Enqueue {
        batch: BatchId,
        specs: Vec<JobSpec>,
        reply: oneshot::Sender<Result<Vec<JobId>>>,
    },
    Control {
        op: QueueOp,
        reply: oneshot::Sender<Result<()>>,
    },
    /// A session finished connecting and can take work.
    SessionUp {
        session: SessionId,
        max_lanes: u8,
    },
    /// The connection dropped. Its jobs pause; they are not failed, and they are not
    /// requeued onto a session that cannot take them.
    SessionDown {
        session: SessionId,
    },
    /// The tab was closed. Unlike `SessionDown`, its work is not coming back.
    SessionClosed {
        session: SessionId,
    },
    Concurrency {
        limit: u8,
    },
    Report(Report),
    Snapshot {
        reply: oneshot::Sender<Vec<JobSnapshot>>,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

/// What a running transfer tells the scheduler.
///
/// Deliberately narrow. A transfer reports what happened to it; it does not decide
/// what that means for the queue, which job runs next, or whether an error is worth
/// retrying.
#[derive(Debug)]
pub enum Report {
    /// A conflict sheet is open for this job.
    Asking {
        job: JobId,
        prompt: PromptId,
    },
    /// The conflict was decided, by an answer or by a policy already in force.
    Decided {
        job: JobId,
        action: ConflictAction,
        /// The person ticked "apply to remaining", so every other job in this batch
        /// inherits the decision instead of asking again.
        apply_to_remaining: bool,
    },
    /// Bytes are moving.
    Started {
        job: JobId,
        /// Learned during preparation, when it was not known at enqueue time.
        size: Option<Bytes>,
        /// A *verified* offset. Zero unless the resume checks passed.
        resume_from: Bytes,
    },
    /// A folder job finished walking, and this many children were queued under it.
    /// Zero means an empty folder, which is finished rather than waiting.
    Scanned {
        job: JobId,
        children: u32,
    },
    Progress {
        job: JobId,
        transferred: Bytes,
    },
    /// The transfer ended, one way or another.
    Finished {
        job: JobId,
        result: std::result::Result<Bytes, EngineError>,
    },
}

/// A cheap handle a running transfer keeps, so it can report without holding the
/// scheduler.
#[derive(Clone)]
pub struct Reporter {
    tx: mpsc::Sender<Msg>,
}

impl std::fmt::Debug for Reporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Reporter")
    }
}

impl Reporter {
    pub async fn send(&self, report: Report) {
        // A closed scheduler means the app is shutting down; the report has nowhere
        // useful to go and dropping it is correct.
        let _ = self.tx.send(Msg::Report(report)).await;
    }

    /// For the progress sink, which is called from inside the transfer loop and must
    /// never park it. Dropping a tick is safe: progress is coalesced downstream and
    /// the final state is always sent with `send`.
    pub fn progress(&self, job: JobId, transferred: Bytes) {
        let _ = self
            .tx
            .try_send(Msg::Report(Report::Progress { job, transferred }));
    }
}

/// One job, as handed to whatever will actually move its bytes.
#[derive(Debug, Clone)]
pub struct RunRequest {
    pub job: JobId,
    pub session: SessionId,
    pub direction: Direction,
    pub remote_path: String,
    pub local_path: PathBuf,
    /// Where to start. Non-zero only after resume verification has passed.
    pub offset: Bytes,
    /// A decision already made for this job — from settings, or inherited from a
    /// sibling through "apply to remaining". `None` means ask.
    pub conflict: Option<ConflictAction>,
    /// How many other jobs in this batch a person's "apply to remaining" would cover.
    pub remaining: u32,
    pub report: Reporter,
}

/// How a job becomes bytes on a wire.
#[async_trait]
pub trait Dispatcher: Send + Sync {
    /// Start one job. Returns as soon as the work is under way; everything after that
    /// arrives through the request's [`Reporter`].
    async fn dispatch(&self, run: RunRequest) -> Result<()>;

    /// Stop one running job. Best effort — a job that has already finished is not an
    /// error, it is a race that the completion won.
    async fn abort(&self, session: SessionId, job: JobId);
}

pub struct SchedulerContext {
    pub store: QueueStore,
    pub events: mpsc::Sender<EngineEvent>,
    pub dispatcher: Arc<dyn Dispatcher>,
    pub rt: tokio::runtime::Handle,
    pub concurrency: u8,
}

/// The handle the engine keeps.
#[derive(Clone)]
pub struct Scheduler {
    tx: mpsc::Sender<Msg>,
}

impl Scheduler {
    /// Start the scheduler, restoring whatever the last run left behind.
    ///
    /// Recovery happens before the task starts taking work, so the first thing anyone
    /// can enqueue lands on a queue that already knows what it was doing.
    pub async fn spawn(ctx: SchedulerContext) -> Result<Self> {
        let restored = ctx.store.recover().await?;
        let (tx, rx) = mpsc::channel(INBOX);
        let inner = Inner {
            jobs: restored.into_iter().map(|job| (job.id, job)).collect(),
            sessions: HashMap::new(),
            rates: HashMap::new(),
            dirty: HashMap::new(),
            store: ctx.store,
            events: ctx.events,
            dispatcher: ctx.dispatcher,
            concurrency: ctx.concurrency.max(1),
            me: Reporter { tx: tx.clone() },
        };
        ctx.rt.spawn(inner.run(rx));
        Ok(Self { tx })
    }

    async fn call<T>(&self, make: impl FnOnce(oneshot::Sender<T>) -> Msg) -> Result<T> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(make(tx)).await.map_err(|_| stopped())?;
        rx.await.map_err(|_| stopped())
    }

    /// Queue one gesture's worth of work.
    ///
    /// Idempotent on `batch`: a command re-sent after an uncertain delivery returns the
    /// job ids it made the first time rather than queueing everything twice.
    pub async fn enqueue(&self, batch: BatchId, specs: Vec<JobSpec>) -> Result<Vec<JobId>> {
        self.call(|reply| Msg::Enqueue {
            batch,
            specs,
            reply,
        })
        .await?
    }

    pub async fn control(&self, op: QueueOp) -> Result<()> {
        self.call(|reply| Msg::Control { op, reply }).await?
    }

    pub async fn session_up(&self, session: SessionId, max_lanes: u8) {
        let _ = self.tx.send(Msg::SessionUp { session, max_lanes }).await;
    }

    pub async fn session_down(&self, session: SessionId) {
        let _ = self.tx.send(Msg::SessionDown { session }).await;
    }

    pub async fn session_closed(&self, session: SessionId) {
        let _ = self.tx.send(Msg::SessionClosed { session }).await;
    }

    pub async fn set_concurrency(&self, limit: u8) {
        let _ = self.tx.send(Msg::Concurrency { limit }).await;
    }

    /// Every job, in queue order — what a remounting UI reconciles against.
    pub async fn snapshot(&self) -> Vec<JobSnapshot> {
        self.call(|reply| Msg::Snapshot { reply })
            .await
            .unwrap_or_default()
    }

    /// One job's state, for a walk checking whether the folder it is enumerating has
    /// been cancelled out from under it.
    pub async fn state(&self, job: JobId) -> Option<JobState> {
        self.snapshot()
            .await
            .into_iter()
            .find(|snapshot| snapshot.id == job)
            .map(|snapshot| snapshot.state)
    }

    pub fn reporter(&self) -> Reporter {
        Reporter {
            tx: self.tx.clone(),
        }
    }

    /// Flush pending progress and stop. Called on app exit.
    pub async fn shutdown(&self) {
        let _ = self.call(|reply| Msg::Shutdown { reply }).await;
    }
}

/// What the scheduler knows about a session it can dispatch to.
struct Slot {
    /// The backend's own limit. SFTP multiplexes channels on one connection; FTP needs
    /// a whole control connection per lane, which is why this is not a constant.
    max_lanes: u8,
    connected: bool,
}

struct Inner {
    jobs: HashMap<JobId, Job>,
    sessions: HashMap<SessionId, Slot>,
    rates: HashMap<JobId, Rate>,
    /// Displayed progress not yet written to the database.
    dirty: HashMap<JobId, Bytes>,
    store: QueueStore,
    events: mpsc::Sender<EngineEvent>,
    dispatcher: Arc<dyn Dispatcher>,
    concurrency: u8,
    me: Reporter,
}

impl Inner {
    async fn run(mut self, mut rx: mpsc::Receiver<Msg>) {
        let mut flush = tokio::time::interval(PROGRESS_FLUSH);
        flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        flush.tick().await;

        loop {
            // A job waiting out its backoff has no event to wake it, so the loop sleeps
            // exactly until the earliest one is due rather than polling.
            let wake = self.next_backoff();
            tokio::select! {
                msg = rx.recv() => {
                    let Some(msg) = msg else { break };
                    if let Msg::Shutdown { reply } = msg {
                        self.flush_progress().await;
                        let _ = reply.send(());
                        break;
                    }
                    self.handle(msg).await;
                }
                _ = tokio::time::sleep(wake) => {}
                _ = flush.tick() => self.flush_progress().await,
            }
            self.pump().await;
        }
    }

    async fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Enqueue {
                batch,
                specs,
                reply,
            } => {
                let outcome = self.enqueue(batch, specs).await;
                let _ = reply.send(outcome);
            }
            Msg::Control { op, reply } => {
                let outcome = self.control(op).await;
                let _ = reply.send(outcome);
            }
            Msg::SessionUp { session, max_lanes } => {
                self.sessions.insert(
                    session,
                    Slot {
                        max_lanes: max_lanes.max(1),
                        connected: true,
                    },
                );
                // Whatever the drop paused comes back on its own. A job a *person*
                // paused does not: that decision outlives the connection.
                let waiting: Vec<JobId> = self
                    .jobs
                    .values()
                    .filter(|job| {
                        job.session == session
                            && matches!(
                                job.state,
                                JobState::Paused {
                                    reason: PauseReason::SessionDown
                                }
                            )
                    })
                    .map(|job| job.id)
                    .collect();
                for id in waiting {
                    self.apply(id, JobEvent::Unpause).await;
                }
            }
            Msg::SessionDown { session } => {
                if let Some(slot) = self.sessions.get_mut(&session) {
                    slot.connected = false;
                }
                self.pause_session(session, PauseReason::SessionDown).await;
            }
            Msg::SessionClosed { session } => {
                self.sessions.remove(&session);
                self.pause_session(session, PauseReason::SessionDown).await;
            }
            Msg::Concurrency { limit } => {
                self.concurrency = limit.max(1);
                // Lowering the slider must take effect on work already running, or the
                // control would only apply to a queue that has not started yet.
                self.throttle().await;
            }
            Msg::Report(report) => self.report(report).await,
            Msg::Snapshot { reply } => {
                let _ = reply.send(self.snapshots());
            }
            Msg::Shutdown { reply } => {
                let _ = reply.send(());
            }
        }
    }

    // ------------------------------------------------------------------ enqueue

    async fn enqueue(&mut self, batch: BatchId, specs: Vec<JobSpec>) -> Result<Vec<JobId>> {
        let mut next = self.tail();
        let jobs: Vec<Job> = specs
            .into_iter()
            .map(|spec| {
                next = Order(next.get() + ORDER_STRIDE);
                Job::new(Uuid::new_v4(), next, batch, spec)
            })
            .collect();

        // The database decides what is new. It holds the deduplication key, and it is
        // the only party that still knows about a batch from before a restart.
        let stored = self.store.insert_batch(jobs).await?;
        let ids = stored.iter().map(|job| job.id).collect();
        for job in stored {
            self.publish(&job).await;
            self.jobs.insert(job.id, job);
        }
        self.stats().await;
        Ok(ids)
    }

    /// The last position in the queue, so new work lands after existing work.
    fn tail(&self) -> Order {
        self.jobs
            .values()
            .map(|job| job.order)
            .max()
            .unwrap_or(Order(0))
    }

    // ------------------------------------------------------------------ control

    async fn control(&mut self, op: QueueOp) -> Result<()> {
        match op {
            QueueOp::Cancel { job } => {
                self.abort(job).await;
                self.apply(job, JobEvent::Cancel).await;
                Ok(())
            }
            QueueOp::Retry { job } => {
                self.require(job)?;
                self.apply(job, JobEvent::Retry).await;
                Ok(())
            }
            QueueOp::Pause { job } => {
                self.require(job)?;
                self.abort(job).await;
                self.apply(
                    job,
                    JobEvent::Pause {
                        reason: PauseReason::User,
                    },
                )
                .await;
                Ok(())
            }
            QueueOp::Resume { job } => {
                self.require(job)?;
                self.apply(job, JobEvent::Unpause).await;
                Ok(())
            }
            QueueOp::PauseAll => {
                for id in self.ids() {
                    self.abort(id).await;
                    self.apply(
                        id,
                        JobEvent::Pause {
                            reason: PauseReason::User,
                        },
                    )
                    .await;
                }
                Ok(())
            }
            QueueOp::ResumeAll => {
                for id in self.ids() {
                    self.apply(id, JobEvent::Unpause).await;
                }
                Ok(())
            }
            QueueOp::Reorder { job, after } => self.reorder(job, after).await,
            QueueOp::ClearCompleted => {
                self.store.clear_completed().await?;
                self.jobs.retain(|_, job| {
                    !matches!(
                        job.state,
                        JobState::Done { .. } | JobState::Cancelled { .. }
                    )
                });
                self.stats().await;
                Ok(())
            }
        }
    }

    /// Move `job` to sit immediately after `after`, or to the front when `after` is
    /// `None`.
    async fn reorder(&mut self, job: JobId, after: Option<JobId>) -> Result<()> {
        self.require(job)?;
        if Some(job) == after {
            return Ok(());
        }

        let mut ordered: Vec<(JobId, Order)> = self
            .jobs
            .values()
            .filter(|j| j.id != job)
            .map(|j| (j.id, j.order))
            .collect();
        ordered.sort_by_key(|(_, order)| *order);

        let (before, next) = match after {
            None => (None, ordered.first().map(|(_, order)| *order)),
            Some(target) => {
                let at = ordered
                    .iter()
                    .position(|(id, _)| *id == target)
                    .ok_or_else(|| missing(target))?;
                (
                    Some(ordered[at].1),
                    ordered.get(at + 1).map(|(_, order)| *order),
                )
            }
        };

        match between(before, next) {
            Some(order) => {
                if let Some(target) = self.jobs.get_mut(&job) {
                    target.order = order;
                }
            }
            // Repeatedly dropping jobs between the same pair halves the gap each time
            // and eventually runs out of integers. Spreading the queue out again is
            // rare, and cheap when it is not.
            None => {
                let mut sequence: Vec<JobId> = Vec::with_capacity(ordered.len() + 1);
                if after.is_none() {
                    sequence.push(job);
                }
                for (id, _) in &ordered {
                    sequence.push(*id);
                    if Some(*id) == after {
                        sequence.push(job);
                    }
                }
                for (index, id) in sequence.into_iter().enumerate() {
                    if let Some(target) = self.jobs.get_mut(&id) {
                        target.order = position(index);
                    }
                }
            }
        }

        // Persisted individually rather than in one statement because the store's API
        // is per job; the queue is small enough that this is not worth a bulk path.
        for job in self.jobs.values() {
            self.store.save_detached(job.clone());
        }
        self.publish_all().await;
        Ok(())
    }

    fn require(&self, job: JobId) -> Result<()> {
        self.jobs
            .contains_key(&job)
            .then_some(())
            .ok_or(missing(job))
    }

    fn ids(&self) -> Vec<JobId> {
        self.jobs.keys().copied().collect()
    }

    // ------------------------------------------------------------------- reports

    async fn report(&mut self, report: Report) {
        match report {
            Report::Asking { job, prompt } => {
                self.apply(job, JobEvent::Ask { prompt }).await;
            }
            Report::Decided {
                job,
                action,
                apply_to_remaining,
            } => {
                if apply_to_remaining {
                    self.propagate(job, action).await;
                }
                self.apply(job, JobEvent::Answered { action }).await;
            }
            Report::Started {
                job,
                size,
                resume_from,
            } => {
                if let Some(size) = size {
                    self.apply(job, JobEvent::Size { size }).await;
                }
                self.rates.insert(job, Rate::new());
                self.apply(job, JobEvent::Start { resume_from }).await;
            }
            Report::Scanned { job, children } => {
                self.apply(job, JobEvent::Scanned { children }).await;
                // Children can finish while the walk is still going, so the folder's
                // completion is decided here as well as on every child's finish.
                self.refresh_folder(job).await;
            }
            Report::Progress { job, transferred } => {
                let speed = self
                    .rates
                    .get_mut(&job)
                    .and_then(|rate| rate.observe(transferred.get()));
                if self
                    .apply_quietly(job, JobEvent::Progress { transferred, speed })
                    .await
                {
                    self.dirty.insert(job, transferred);
                    if let Some(job) = self.jobs.get(&job) {
                        let snapshot = job.snapshot();
                        // `try_send`: a dropped progress tick is invisible, and the
                        // coordinator coalesces them anyway. Every other update below
                        // goes out with `send`.
                        let _ = self.events.try_send(EngineEvent::JobUpdate {
                            job: Box::new(snapshot),
                        });
                    }
                }
            }
            Report::Finished { job, result } => {
                self.rates.remove(&job);
                self.dirty.remove(&job);
                let event = match result {
                    Ok(transferred) => {
                        self.apply_quietly(
                            job,
                            JobEvent::Progress {
                                transferred,
                                speed: None,
                            },
                        )
                        .await;
                        JobEvent::Complete
                    }
                    Err(EngineError::Cancelled) => JobEvent::Cancel,
                    Err(error) => JobEvent::Fail { error },
                };
                self.apply(job, event).await;
                if let Some(parent) = self.jobs.get(&job).and_then(|job| job.parent) {
                    self.refresh_folder(parent).await;
                }
            }
        }
    }

    /// Give every other job in this batch the decision a person just made.
    ///
    /// Scoped to the batch, not the queue: "apply to remaining" answers a question
    /// about the files in *this* gesture, and silently overwriting a decision that
    /// belongs to a different drag would be a policy nobody asked for.
    async fn propagate(&mut self, from: JobId, action: ConflictAction) {
        let Some(batch) = self.jobs.get(&from).map(|job| job.batch) else {
            return;
        };
        let siblings: Vec<JobId> = self
            .jobs
            .values()
            .filter(|job| job.batch == batch && job.id != from && !job.state.is_terminal())
            .map(|job| job.id)
            .collect();
        for id in siblings {
            self.apply_quietly(id, JobEvent::AdoptPolicy { action })
                .await;
        }
    }

    /// Roll a folder's children up into it, and finish it when they are all done.
    ///
    /// Called from both ends because either can be last: a walk that is still going
    /// when the final queued child finishes, or a child that finishes after the walk
    /// has already ended. A folder still `Scanning` is never completed here — more
    /// children may yet be found, and `advance` rejects the attempt.
    async fn refresh_folder(&mut self, parent: JobId) {
        if !self
            .jobs
            .get(&parent)
            .is_some_and(|job| job.kind == JobKind::Folder)
        {
            return;
        }
        let outstanding = self
            .jobs
            .values()
            .filter(|job| job.parent == Some(parent) && !job.state.is_terminal())
            .count();
        let moved: Bytes = self
            .jobs
            .values()
            .filter(|job| job.parent == Some(parent))
            .map(|job| job.transferred)
            .fold(Bytes::ZERO, |sum, bytes| sum + bytes);

        self.apply_quietly(
            parent,
            JobEvent::Progress {
                transferred: moved,
                speed: None,
            },
        )
        .await;
        let scanning = matches!(
            self.jobs.get(&parent).map(|job| &job.state),
            Some(JobState::Scanning)
        );
        if outstanding == 0 && !scanning {
            self.apply(parent, JobEvent::Complete).await;
        } else if let Some(job) = self.jobs.get(&parent) {
            let snapshot = job.snapshot();
            let _ = self
                .events
                .send(EngineEvent::JobUpdate {
                    job: Box::new(snapshot),
                })
                .await;
        }
    }

    // ------------------------------------------------------------------ dispatch

    /// Start whatever may start, in queue order.
    ///
    /// Runs after every message. Scanning the whole table each time is deliberate: it
    /// is a few thousand comparisons on a queue of a few thousand jobs, and the
    /// alternative — an incrementally maintained ready set — is a second source of
    /// truth about eligibility that can disagree with the first.
    async fn pump(&mut self) {
        let now = Utc::now();
        let global = usize::from(self.concurrency);

        let mut used = self.jobs.values().filter(|job| job.holds_slot()).count();
        let mut per_session: HashMap<SessionId, usize> = HashMap::new();
        for job in self.jobs.values().filter(|job| job.holds_slot()) {
            *per_session.entry(job.session).or_default() += 1;
        }

        let mut ready: Vec<(Order, JobId)> = self
            .jobs
            .values()
            .filter(|job| job.kind == JobKind::File && job.is_eligible(now))
            .filter(|job| {
                self.sessions
                    .get(&job.session)
                    .is_some_and(|slot| slot.connected)
            })
            .map(|job| (job.order, job.id))
            .collect();
        ready.sort();

        for (_, id) in ready {
            if used >= global {
                break;
            }
            let Some(job) = self.jobs.get(&id) else {
                continue;
            };
            let session = job.session;
            let cap = self
                .sessions
                .get(&session)
                .map_or(0, |slot| usize::from(slot.max_lanes));
            let running = per_session.entry(session).or_default();
            if *running >= cap {
                continue;
            }

            let run = RunRequest {
                job: id,
                session,
                direction: job.direction,
                remote_path: job.remote_path.clone(),
                local_path: job.local_path.clone(),
                offset: job.verified_offset(),
                conflict: job.chosen.or(job.conflict_policy),
                remaining: self.remaining_in_batch(job.batch, id),
                report: self.me.clone(),
            };

            self.apply(id, JobEvent::Prepare).await;
            match self.dispatcher.dispatch(run).await {
                Ok(()) => {
                    used += 1;
                    *running += 1;
                }
                // The session could not take it — it is going away, or its lanes are
                // gone. Pausing is right: the job is fine, the connection is not.
                Err(_) => {
                    self.apply(
                        id,
                        JobEvent::Pause {
                            reason: PauseReason::SessionDown,
                        },
                    )
                    .await;
                }
            }
        }
        self.stats().await;
    }

    fn remaining_in_batch(&self, batch: BatchId, except: JobId) -> u32 {
        self.jobs
            .values()
            .filter(|job| {
                job.batch == batch
                    && job.id != except
                    && job.chosen.is_none()
                    && !job.state.is_terminal()
            })
            .count()
            .min(u32::MAX as usize) as u32
    }

    /// Bring the number of running transfers back under a lowered limit.
    ///
    /// The jobs furthest down the queue stop first, so lowering the slider does not
    /// pause the work a person is most likely watching.
    async fn throttle(&mut self) {
        let global = usize::from(self.concurrency);
        let mut running: Vec<(Order, JobId)> = self
            .jobs
            .values()
            .filter(|job| job.holds_slot())
            .map(|job| (job.order, job.id))
            .collect();
        running.sort();
        while running.len() > global {
            let Some((_, id)) = running.pop() else { break };
            self.abort(id).await;
            self.apply(
                id,
                JobEvent::Pause {
                    reason: PauseReason::Throttled,
                },
            )
            .await;
        }
        // Anything the *previous* limit throttled is eligible again when it rises.
        let throttled: Vec<JobId> = self
            .jobs
            .values()
            .filter(|job| {
                matches!(
                    job.state,
                    JobState::Paused {
                        reason: PauseReason::Throttled
                    }
                )
            })
            .map(|job| job.id)
            .collect();
        for id in throttled {
            self.apply(id, JobEvent::Unpause).await;
        }
    }

    async fn pause_session(&mut self, session: SessionId, reason: PauseReason) {
        let affected: Vec<JobId> = self
            .jobs
            .values()
            .filter(|job| job.session == session && !job.state.is_terminal())
            .map(|job| job.id)
            .collect();
        for id in affected {
            self.apply(id, JobEvent::Pause { reason }).await;
        }
    }

    async fn abort(&self, job: JobId) {
        if let Some(target) = self.jobs.get(&job)
            && target.holds_slot()
        {
            self.dispatcher.abort(target.session, job).await;
        }
    }

    // ------------------------------------------------------------- state changes

    /// Apply an event and tell everyone about it.
    async fn apply(&mut self, id: JobId, event: JobEvent) -> bool {
        let Some(job) = self.jobs.get_mut(&id) else {
            return false;
        };
        if !advance(job, event) {
            return false;
        }
        let job = job.clone();

        // A finished job's outcome must be on disk before the UI is told it happened;
        // everything else can be written behind the scheduler.
        if job.state.is_terminal() {
            if let Err(err) = self.store.save(job.clone()).await {
                tracing::error!(id = %job.id, %err, "could not persist a finished job");
            }
        } else {
            self.store.save_detached(job.clone());
        }

        self.publish(&job).await;
        true
    }

    /// Apply an event without publishing it — for the many small updates that are
    /// only meaningful once something else is sent.
    async fn apply_quietly(&mut self, id: JobId, event: JobEvent) -> bool {
        self.jobs
            .get_mut(&id)
            .is_some_and(|job| advance(job, event))
    }

    async fn publish(&self, job: &Job) {
        let _ = self
            .events
            .send(EngineEvent::JobUpdate {
                job: Box::new(job.snapshot()),
            })
            .await;
    }

    async fn publish_all(&self) {
        for job in self.snapshots() {
            let _ = self
                .events
                .send(EngineEvent::JobUpdate { job: Box::new(job) })
                .await;
        }
    }

    fn snapshots(&self) -> Vec<JobSnapshot> {
        let mut all: Vec<JobSnapshot> = self.jobs.values().map(|job| job.snapshot()).collect();
        all.sort_by_key(|job| job.order);
        all
    }

    async fn stats(&self) {
        let mut stats = QueueStats::default();
        for job in self.jobs.values() {
            match &job.state {
                JobState::Failed { .. } => stats.failed += 1,
                JobState::Done { .. } => stats.done += 1,
                JobState::Queued | JobState::Paused { .. } | JobState::Scanning => {
                    stats.queued += 1
                }
                JobState::Cancelled { .. } => {}
                _ => stats.active += 1,
            }
            if job.holds_slot() {
                stats.speed_bps += job.speed_bps.unwrap_or(Bytes::ZERO);
            }
        }
        let _ = self.events.send(EngineEvent::QueueStats { stats }).await;
    }

    /// How long until the earliest backing-off job is eligible.
    fn next_backoff(&self) -> Duration {
        const IDLE: Duration = Duration::from_secs(3600);
        let now = Utc::now();
        self.jobs
            .values()
            .filter(|job| matches!(job.state, JobState::Queued))
            .filter_map(|job| job.retry_at)
            .map(|at| (at - now).to_std().unwrap_or(Duration::ZERO))
            .min()
            .unwrap_or(IDLE)
    }

    async fn flush_progress(&mut self) {
        if self.dirty.is_empty() {
            return;
        }
        let updates: Vec<(JobId, Bytes)> = self.dirty.drain().collect();
        if let Err(err) = self.store.save_progress(updates).await {
            tracing::warn!(%err, "could not persist queue progress");
        }
    }
}

/// Bytes per second over a short trailing window.
struct Rate {
    samples: std::collections::VecDeque<(Instant, u64)>,
}

impl Rate {
    fn new() -> Self {
        Self {
            samples: std::collections::VecDeque::new(),
        }
    }

    fn observe(&mut self, bytes: u64) -> Option<Bytes> {
        let now = Instant::now();
        self.samples.push_back((now, bytes));
        while self
            .samples
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) > RATE_WINDOW)
        {
            self.samples.pop_front();
        }
        let (first_at, first_bytes) = *self.samples.front()?;
        let span = now.duration_since(first_at);
        if span < RATE_MIN_SPAN {
            return None;
        }
        let moved = bytes.saturating_sub(first_bytes);
        Some(Bytes((moved as f64 / span.as_secs_f64()) as u64))
    }
}

fn missing(job: JobId) -> EngineError {
    EngineError::NotFound {
        path: job.to_string(),
    }
}

fn stopped() -> EngineError {
    EngineError::protocol("the transfer queue is not running")
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use crate::model::Direction;

    use super::*;

    /// A dispatcher that records what it was asked to run and does nothing until a
    /// test says so. Every rule in this module is about *when* a job runs, so the
    /// thing that runs it is exactly what a test should be able to hold still.
    #[derive(Default)]
    struct Bench {
        started: Mutex<Vec<RunRequest>>,
        aborted: Mutex<Vec<JobId>>,
        refuse: Mutex<bool>,
    }

    #[async_trait]
    impl Dispatcher for Bench {
        async fn dispatch(&self, run: RunRequest) -> Result<()> {
            if *self.refuse.lock().unwrap() {
                return Err(EngineError::network("no lane"));
            }
            self.started.lock().unwrap().push(run);
            Ok(())
        }

        async fn abort(&self, _session: SessionId, job: JobId) {
            self.aborted.lock().unwrap().push(job);
        }
    }

    impl Bench {
        fn running(&self) -> Vec<JobId> {
            self.started.lock().unwrap().iter().map(|r| r.job).collect()
        }

        fn take(&self) -> Vec<RunRequest> {
            std::mem::take(&mut self.started.lock().unwrap())
        }
    }

    struct Harness {
        scheduler: Scheduler,
        bench: Arc<Bench>,
        session: SessionId,
        events: mpsc::Receiver<EngineEvent>,
    }

    async fn harness(concurrency: u8, max_lanes: u8) -> Harness {
        let bench = Arc::new(Bench::default());
        let (tx, events) = mpsc::channel(1024);
        let scheduler = Scheduler::spawn(SchedulerContext {
            store: QueueStore::in_memory().await.unwrap(),
            events: tx,
            dispatcher: Arc::clone(&bench) as Arc<dyn Dispatcher>,
            rt: tokio::runtime::Handle::current(),
            concurrency,
        })
        .await
        .unwrap();

        let session = Uuid::new_v4();
        scheduler.session_up(session, max_lanes).await;
        Harness {
            scheduler,
            bench,
            session,
            events,
        }
    }

    fn spec(session: SessionId, name: &str) -> JobSpec {
        JobSpec {
            session,
            server_id: Uuid::new_v4(),
            kind: JobKind::File,
            direction: Direction::Down,
            remote_path: format!("/remote/{name}"),
            local_path: PathBuf::from(format!("/local/{name}")),
            size: Some(Bytes(1_000_000)),
            parent: None,
            item: name.into(),
        }
    }

    /// Report a job through the whole sequence a real transfer would: it starts, then
    /// it ends. A `Finished` with no `Started` is a runner bug, so tests that mean to
    /// exercise the queue should not accidentally send one.
    async fn finish(h: &Harness, job: JobId, result: std::result::Result<Bytes, EngineError>) {
        h.scheduler
            .reporter()
            .send(Report::Started {
                job,
                size: None,
                resume_from: Bytes::ZERO,
            })
            .await;
        h.scheduler
            .reporter()
            .send(Report::Finished { job, result })
            .await;
    }

    /// The scheduler answers on its own task, so a test that wants to observe the
    /// result of a message has to let that task run first.
    async fn settle(h: &Harness) {
        for _ in 0..8 {
            tokio::task::yield_now().await;
            let _ = h.scheduler.snapshot().await;
        }
    }

    async fn states(h: &Harness) -> Vec<(String, JobState)> {
        h.scheduler
            .snapshot()
            .await
            .into_iter()
            .map(|job| {
                (
                    job.remote_path.rsplit('/').next().unwrap_or("").to_string(),
                    job.state,
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn the_concurrency_limit_is_a_limit() {
        let h = harness(2, 8).await;
        let batch = Uuid::new_v4();
        let specs = (0..5).map(|i| spec(h.session, &format!("f{i}"))).collect();
        h.scheduler.enqueue(batch, specs).await.unwrap();
        settle(&h).await;

        assert_eq!(
            h.bench.running().len(),
            2,
            "five queued, two slots, two running"
        );
    }

    /// The design's per-server limit exists because FTP needs a whole control
    /// connection per lane. A global slider of 8 must not open 8 lanes on a backend
    /// that says it has 2.
    #[tokio::test]
    async fn a_backend_lane_limit_beats_a_higher_global_limit() {
        let h = harness(8, 2).await;
        let batch = Uuid::new_v4();
        let specs = (0..6).map(|i| spec(h.session, &format!("f{i}"))).collect();
        h.scheduler.enqueue(batch, specs).await.unwrap();
        settle(&h).await;

        assert_eq!(h.bench.running().len(), 2);
    }

    #[tokio::test]
    async fn jobs_start_in_queue_order() {
        let h = harness(1, 8).await;
        let batch = Uuid::new_v4();
        h.scheduler
            .enqueue(
                batch,
                vec![
                    spec(h.session, "first"),
                    spec(h.session, "second"),
                    spec(h.session, "third"),
                ],
            )
            .await
            .unwrap();
        settle(&h).await;

        let started = h.bench.take();
        assert_eq!(started.len(), 1);
        assert!(started[0].remote_path.ends_with("first"));
    }

    /// Reordering is the drawer's drag handle. A job dragged to the front is the next
    /// one to run, and that has to be true of a queue that is already moving.
    #[tokio::test]
    async fn reordering_changes_what_runs_next() {
        let h = harness(1, 8).await;
        let batch = Uuid::new_v4();
        let ids = h
            .scheduler
            .enqueue(
                batch,
                vec![
                    spec(h.session, "a"),
                    spec(h.session, "b"),
                    spec(h.session, "c"),
                ],
            )
            .await
            .unwrap();
        settle(&h).await;
        let _ = h.bench.take();

        // Put the last job at the front, then let the running one finish.
        h.scheduler
            .control(QueueOp::Reorder {
                job: ids[2],
                after: None,
            })
            .await
            .unwrap();
        finish(&h, ids[0], Ok(Bytes(1_000_000))).await;
        settle(&h).await;

        assert_eq!(
            h.bench.running(),
            vec![ids[2]],
            "the job dragged to the front goes next"
        );
    }

    #[tokio::test]
    async fn a_paused_job_does_not_run_and_a_resumed_one_does() {
        let h = harness(1, 8).await;
        let batch = Uuid::new_v4();
        let ids = h
            .scheduler
            .enqueue(batch, vec![spec(h.session, "a")])
            .await
            .unwrap();
        settle(&h).await;

        h.scheduler
            .control(QueueOp::Pause { job: ids[0] })
            .await
            .unwrap();
        settle(&h).await;
        assert_eq!(h.bench.aborted.lock().unwrap().as_slice(), &[ids[0]]);
        assert!(matches!(
            states(&h).await[0].1,
            JobState::Paused {
                reason: PauseReason::User
            }
        ));

        let _ = h.bench.take();
        h.scheduler
            .control(QueueOp::Resume { job: ids[0] })
            .await
            .unwrap();
        settle(&h).await;
        assert_eq!(h.bench.running(), vec![ids[0]]);
    }

    /// The bug this prevents: a dropped connection pauses a session's work, the
    /// connection comes back, and a job a *person* paused starts moving again.
    #[tokio::test]
    async fn reconnecting_resumes_what_the_drop_paused_and_nothing_else() {
        let h = harness(4, 8).await;
        let batch = Uuid::new_v4();
        let ids = h
            .scheduler
            .enqueue(
                batch,
                vec![spec(h.session, "auto"), spec(h.session, "byhand")],
            )
            .await
            .unwrap();
        settle(&h).await;

        h.scheduler
            .control(QueueOp::Pause { job: ids[1] })
            .await
            .unwrap();
        h.scheduler.session_down(h.session).await;
        settle(&h).await;
        let _ = h.bench.take();

        h.scheduler.session_up(h.session, 8).await;
        settle(&h).await;

        assert_eq!(
            h.bench.running(),
            vec![ids[0]],
            "the connection came back; the person's decision did not expire with it"
        );
    }

    #[tokio::test]
    async fn a_session_that_goes_away_pauses_its_work_rather_than_failing_it() {
        let h = harness(4, 8).await;
        let batch = Uuid::new_v4();
        h.scheduler
            .enqueue(batch, vec![spec(h.session, "a"), spec(h.session, "b")])
            .await
            .unwrap();
        settle(&h).await;

        h.scheduler.session_down(h.session).await;
        settle(&h).await;

        for (name, state) in states(&h).await {
            assert!(
                matches!(
                    state,
                    JobState::Paused {
                        reason: PauseReason::SessionDown
                    }
                ),
                "{name} is {state:?}"
            );
        }
    }

    #[tokio::test]
    async fn lowering_the_limit_stops_work_already_running() {
        let h = harness(4, 8).await;
        let batch = Uuid::new_v4();
        let specs = (0..4).map(|i| spec(h.session, &format!("f{i}"))).collect();
        h.scheduler.enqueue(batch, specs).await.unwrap();
        settle(&h).await;
        assert_eq!(h.bench.running().len(), 4);

        h.scheduler.set_concurrency(2).await;
        settle(&h).await;

        let running = states(&h)
            .await
            .into_iter()
            .filter(|(_, state)| state.occupies_transfer_slot())
            .count();
        assert_eq!(running, 2, "the slider applies to work in flight");
        assert_eq!(
            h.bench.aborted.lock().unwrap().len(),
            2,
            "and the transfers it stopped were actually stopped"
        );
    }

    #[tokio::test]
    async fn raising_the_limit_starts_what_it_throttled() {
        let h = harness(4, 8).await;
        let batch = Uuid::new_v4();
        let specs = (0..4).map(|i| spec(h.session, &format!("f{i}"))).collect();
        h.scheduler.enqueue(batch, specs).await.unwrap();
        settle(&h).await;

        h.scheduler.set_concurrency(1).await;
        settle(&h).await;
        let _ = h.bench.take();
        h.scheduler.set_concurrency(4).await;
        settle(&h).await;

        let running = states(&h)
            .await
            .into_iter()
            .filter(|(_, state)| state.occupies_transfer_slot())
            .count();
        assert_eq!(running, 4);
    }

    #[tokio::test]
    async fn a_finished_job_frees_its_slot_for_the_next_one() {
        let h = harness(1, 8).await;
        let batch = Uuid::new_v4();
        let ids = h
            .scheduler
            .enqueue(batch, vec![spec(h.session, "a"), spec(h.session, "b")])
            .await
            .unwrap();
        settle(&h).await;
        let _ = h.bench.take();

        finish(&h, ids[0], Ok(Bytes(1_000_000))).await;
        settle(&h).await;

        assert_eq!(h.bench.running(), vec![ids[1]]);
        assert!(matches!(
            states(&h).await[0].1,
            JobState::Done { skipped: false, .. }
        ));
    }

    /// Trust and integrity failures must reach the user rather than being retried,
    /// and a retryable one must not stop on the first attempt.
    #[tokio::test]
    async fn a_permission_failure_stops_and_a_network_failure_waits() {
        let h = harness(2, 8).await;
        let batch = Uuid::new_v4();
        let ids = h
            .scheduler
            .enqueue(batch, vec![spec(h.session, "a"), spec(h.session, "b")])
            .await
            .unwrap();
        settle(&h).await;

        finish(
            &h,
            ids[0],
            Err(EngineError::PermissionDenied {
                path: "/remote/a".into(),
            }),
        )
        .await;
        finish(&h, ids[1], Err(EngineError::network("reset"))).await;
        settle(&h).await;

        let states = states(&h).await;
        assert!(
            matches!(states[0].1, JobState::Failed { .. }),
            "a permission failure is for a person, not a retry: {:?}",
            states[0].1
        );
        assert!(
            matches!(states[1].1, JobState::Queued),
            "a network failure waits its backoff: {:?}",
            states[1].1
        );
    }

    #[tokio::test]
    async fn cancelling_stops_the_transfer_and_the_job() {
        let h = harness(1, 8).await;
        let batch = Uuid::new_v4();
        let ids = h
            .scheduler
            .enqueue(batch, vec![spec(h.session, "a")])
            .await
            .unwrap();
        settle(&h).await;

        h.scheduler
            .control(QueueOp::Cancel { job: ids[0] })
            .await
            .unwrap();
        settle(&h).await;

        assert_eq!(h.bench.aborted.lock().unwrap().as_slice(), &[ids[0]]);
        assert!(matches!(states(&h).await[0].1, JobState::Cancelled { .. }));
    }

    #[tokio::test]
    async fn retrying_a_failed_job_puts_it_back_to_work() {
        let h = harness(1, 8).await;
        let batch = Uuid::new_v4();
        let ids = h
            .scheduler
            .enqueue(batch, vec![spec(h.session, "a")])
            .await
            .unwrap();
        settle(&h).await;
        finish(
            &h,
            ids[0],
            Err(EngineError::Auth {
                message: "denied".into(),
            }),
        )
        .await;
        settle(&h).await;
        let _ = h.bench.take();

        h.scheduler
            .control(QueueOp::Retry { job: ids[0] })
            .await
            .unwrap();
        settle(&h).await;

        assert_eq!(h.bench.running(), vec![ids[0]]);
    }

    /// "Apply to remaining" answers a question about the files in *this* gesture.
    /// Reaching into a different drag's jobs would be a policy nobody asked for.
    #[tokio::test]
    async fn apply_to_remaining_stops_at_the_batch_it_was_asked_about() {
        let h = harness(1, 8).await;
        let mine = Uuid::new_v4();
        let theirs = Uuid::new_v4();
        let ids = h
            .scheduler
            .enqueue(mine, vec![spec(h.session, "a"), spec(h.session, "b")])
            .await
            .unwrap();
        let others = h
            .scheduler
            .enqueue(theirs, vec![spec(h.session, "c")])
            .await
            .unwrap();
        settle(&h).await;

        h.scheduler
            .reporter()
            .send(Report::Decided {
                job: ids[0],
                action: ConflictAction::Overwrite,
                apply_to_remaining: true,
            })
            .await;
        settle(&h).await;

        let policies: HashMap<JobId, Option<ConflictAction>> = h
            .scheduler
            .snapshot()
            .await
            .into_iter()
            .map(|job| (job.id, job.conflict_policy))
            .collect();
        assert_eq!(policies[&ids[1]], Some(ConflictAction::Overwrite));
        assert_eq!(
            policies[&others[0]], None,
            "a different gesture keeps its own question"
        );
    }

    #[tokio::test]
    async fn a_dispatcher_that_cannot_take_the_job_pauses_it_rather_than_losing_it() {
        let h = harness(1, 8).await;
        *h.bench.refuse.lock().unwrap() = true;
        let batch = Uuid::new_v4();
        h.scheduler
            .enqueue(batch, vec![spec(h.session, "a")])
            .await
            .unwrap();
        settle(&h).await;

        assert!(matches!(
            states(&h).await[0].1,
            JobState::Paused {
                reason: PauseReason::SessionDown
            }
        ));
    }

    #[tokio::test]
    async fn nothing_is_dispatched_to_a_session_that_never_connected() {
        let h = harness(4, 8).await;
        let stranger = Uuid::new_v4();
        h.scheduler
            .enqueue(Uuid::new_v4(), vec![spec(stranger, "a")])
            .await
            .unwrap();
        settle(&h).await;

        assert!(h.bench.running().is_empty());
        assert!(matches!(states(&h).await[0].1, JobState::Queued));
    }

    #[tokio::test]
    async fn clearing_completed_leaves_failed_work_alone() {
        let h = harness(2, 8).await;
        let batch = Uuid::new_v4();
        let ids = h
            .scheduler
            .enqueue(batch, vec![spec(h.session, "a"), spec(h.session, "b")])
            .await
            .unwrap();
        settle(&h).await;
        finish(&h, ids[0], Ok(Bytes(10))).await;
        finish(
            &h,
            ids[1],
            Err(EngineError::Auth {
                message: "denied".into(),
            }),
        )
        .await;
        settle(&h).await;

        h.scheduler.control(QueueOp::ClearCompleted).await.unwrap();
        settle(&h).await;

        let left = states(&h).await;
        assert_eq!(left.len(), 1);
        assert!(matches!(left[0].1, JobState::Failed { .. }));
    }

    #[tokio::test]
    async fn a_command_for_a_job_that_does_not_exist_is_an_error_not_a_panic() {
        let h = harness(1, 8).await;
        let err = h
            .scheduler
            .control(QueueOp::Pause {
                job: Uuid::new_v4(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::NotFound { .. }));
    }

    #[tokio::test]
    async fn progress_reaches_the_ui_and_the_queue_reports_a_speed() {
        let mut h = harness(1, 8).await;
        let batch = Uuid::new_v4();
        let ids = h
            .scheduler
            .enqueue(batch, vec![spec(h.session, "a")])
            .await
            .unwrap();
        settle(&h).await;
        h.scheduler
            .reporter()
            .send(Report::Started {
                job: ids[0],
                size: Some(Bytes(1_000_000)),
                resume_from: Bytes::ZERO,
            })
            .await;
        h.scheduler
            .reporter()
            .send(Report::Progress {
                job: ids[0],
                transferred: Bytes(250_000),
            })
            .await;
        settle(&h).await;

        let snapshot = h.scheduler.snapshot().await;
        assert_eq!(snapshot[0].transferred, Bytes(250_000));
        assert!(matches!(snapshot[0].state, JobState::Transferring));

        let mut saw_update = false;
        while let Ok(event) = h.events.try_recv() {
            if let EngineEvent::JobUpdate { job } = event
                && job.transferred == Bytes(250_000)
            {
                saw_update = true;
            }
        }
        assert!(saw_update, "the drawer is told about progress");
    }

    #[tokio::test]
    async fn a_folder_finishes_when_its_last_child_does() {
        let h = harness(4, 8).await;
        let batch = Uuid::new_v4();
        let parent_spec = JobSpec {
            kind: JobKind::Folder,
            ..spec(h.session, "dir")
        };
        let parent = h.scheduler.enqueue(batch, vec![parent_spec]).await.unwrap()[0];

        let children = h
            .scheduler
            .enqueue(
                batch,
                vec![
                    JobSpec {
                        parent: Some(parent),
                        ..spec(h.session, "dir/one")
                    },
                    JobSpec {
                        parent: Some(parent),
                        ..spec(h.session, "dir/two")
                    },
                ],
            )
            .await
            .unwrap();
        h.scheduler
            .reporter()
            .send(Report::Scanned {
                job: parent,
                children: 2,
            })
            .await;
        settle(&h).await;

        finish(&h, children[0], Ok(Bytes(400))).await;
        settle(&h).await;
        let by_id: HashMap<JobId, JobState> = h
            .scheduler
            .snapshot()
            .await
            .into_iter()
            .map(|job| (job.id, job.state))
            .collect();
        assert!(
            !by_id[&parent].is_terminal(),
            "one child left, so the folder is not finished"
        );

        finish(&h, children[1], Ok(Bytes(600))).await;
        settle(&h).await;

        let done = h.scheduler.snapshot().await;
        let folder = done.iter().find(|job| job.id == parent).unwrap();
        assert!(matches!(folder.state, JobState::Done { .. }));
        assert_eq!(
            folder.transferred,
            Bytes(1000),
            "a folder's progress is the sum of its children's"
        );
    }

    /// A big folder queues children as it walks, so the first ones can finish while
    /// the walk is still going. Completing the folder there would report a directory
    /// as fully transferred when most of it had not been looked at yet.
    #[tokio::test]
    async fn a_folder_still_being_walked_is_not_finished_by_its_first_children() {
        let h = harness(4, 8).await;
        let batch = Uuid::new_v4();
        let parent = h
            .scheduler
            .enqueue(
                batch,
                vec![JobSpec {
                    kind: JobKind::Folder,
                    ..spec(h.session, "dir")
                }],
            )
            .await
            .unwrap()[0];
        let first = h
            .scheduler
            .enqueue(
                batch,
                vec![JobSpec {
                    parent: Some(parent),
                    ..spec(h.session, "dir/one")
                }],
            )
            .await
            .unwrap()[0];
        settle(&h).await;

        // Every child queued so far is done, but the walk has not reported in.
        finish(&h, first, Ok(Bytes(400))).await;
        settle(&h).await;
        let folder =
            |snapshot: Vec<JobSnapshot>| snapshot.into_iter().find(|job| job.id == parent).unwrap();
        assert!(
            matches!(
                folder(h.scheduler.snapshot().await).state,
                JobState::Scanning
            ),
            "the folder is still being walked, so it is not finished"
        );

        let second = h
            .scheduler
            .enqueue(
                batch,
                vec![JobSpec {
                    parent: Some(parent),
                    ..spec(h.session, "dir/two")
                }],
            )
            .await
            .unwrap()[0];
        h.scheduler
            .reporter()
            .send(Report::Scanned {
                job: parent,
                children: 2,
            })
            .await;
        settle(&h).await;
        assert!(
            !folder(h.scheduler.snapshot().await).state.is_terminal(),
            "the walk finished, but a child has not"
        );

        finish(&h, second, Ok(Bytes(600))).await;
        settle(&h).await;
        assert!(matches!(
            folder(h.scheduler.snapshot().await).state,
            JobState::Done { .. }
        ));
    }

    /// A folder is `Transferring` while its children move. If it counted against the
    /// slider, a queue of folders would spend every slot on bookkeeping.
    #[tokio::test]
    async fn a_folder_is_never_dispatched_and_never_spends_a_slot() {
        let h = harness(1, 8).await;
        let batch = Uuid::new_v4();
        h.scheduler
            .enqueue(
                batch,
                vec![
                    JobSpec {
                        kind: JobKind::Folder,
                        ..spec(h.session, "dir")
                    },
                    spec(h.session, "file"),
                ],
            )
            .await
            .unwrap();
        settle(&h).await;

        let started = h.bench.take();
        assert_eq!(started.len(), 1);
        assert!(
            started[0].remote_path.ends_with("file"),
            "the folder is a container, not a transfer"
        );
    }
}
