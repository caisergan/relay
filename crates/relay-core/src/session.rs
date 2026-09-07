//! One task per session, owning its connection exclusively.
//!
//! Nothing outside the actor holds the backend. Browse commands arrive on a channel
//! and are answered in order; transfers are handed a [`TransferLane`] and move into
//! their own tasks, so a 4 GB download cannot stop a directory listing. That split is
//! the whole reason [`crate::protocol`] separates lanes from the protocol handle.
//!
//! The lifecycle rules that matter, all of which have tests:
//!
//! - Every remote operation has a deadline. A server that accepts a request and then
//!   says nothing must not wedge the session or hold up shutdown.
//! - Closing cancels first, denies outstanding prompts second, and only then waits.
//!   A transfer parked on an unanswered conflict sheet would otherwise never finish.
//! - The join is awaited with a grace period; aborting is the fallback, not the plan.

use std::collections::{HashMap, VecDeque};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::coordinator::log_line;
use crate::error::{EngineError, Result};
use crate::events::{ActivityEntry, EngineEvent, ListingSnapshot, LogKind};
use crate::interact::{ConflictAction, Interact, Prompt, PromptReply};
use crate::job::{JobSnapshot, JobState};
use crate::model::{
    Direction, FileFacts, JobId, PromptId, RemoteEntry, ServerConfig, ServerId, Session, SessionId,
    SessionState,
};
use crate::protocol::{
    ProgressSink, Protocol, SecretSource, TransferLane, TransferOutcome, TransferReq,
};
use crate::wire::{Bytes, Order, Seq};

/// Keepalive interval. Doubles as the latency probe behind the session header's dot.
pub const KEEPALIVE: Duration = Duration::from_secs(30);
/// How long a single browse operation may take before it is abandoned. Generous: a
/// directory with 100k entries over a slow link is slow, not broken.
pub const OP_DEADLINE: Duration = Duration::from_secs(60);
/// The keepalive is one small round trip, so it gets a much tighter bound — this is
/// the probe that decides the connection is gone.
pub const KEEPALIVE_DEADLINE: Duration = Duration::from_secs(15);
/// How long `close` waits for the actor to finish before giving up on it.
pub const CLOSE_GRACE: Duration = Duration::from_secs(5);
/// Concurrent ad-hoc transfers per session in phase 1. The real scheduler is phase 2;
/// until then this is a cap, not a queue policy.
pub const MAX_ADHOC_LANES: u8 = 2;
/// Commands buffered before a caller has to wait for the actor.
const CMD_BUFFER: usize = 64;

/// What the actor is asked to do. Every variant that produces a value carries its own
/// reply channel, so callers await exactly their own answer.
pub enum SessionCmd {
    List {
        path: String,
        reply: oneshot::Sender<Result<Vec<RemoteEntry>>>,
    },
    Stat {
        path: String,
        reply: oneshot::Sender<Result<Option<RemoteEntry>>>,
    },
    Mkdir {
        path: String,
        reply: oneshot::Sender<Result<()>>,
    },
    Rename {
        from: String,
        to: String,
        reply: oneshot::Sender<Result<()>>,
    },
    RemoveFile {
        path: String,
        reply: oneshot::Sender<Result<()>>,
    },
    RemoveDir {
        path: String,
        reply: oneshot::Sender<Result<()>>,
    },
    ReadFile {
        path: String,
        max: u64,
        reply: oneshot::Sender<Result<Vec<u8>>>,
    },
    /// Fire and forget: the job's life is told entirely in `JobUpdate` events.
    Transfer(Box<TransferOrder>),
    /// Stop one transfer. The actor owns its jobs' cancellation tokens, so nothing
    /// outside it has to keep a parallel map that could disagree.
    CancelJob {
        job: JobId,
        reply: oneshot::Sender<Result<()>>,
    },
    Disconnect,
}

/// A transfer as the session receives it. The queue that will eventually produce these
/// is phase 2; phase 1 builds one per user gesture.
#[derive(Debug, Clone)]
pub struct TransferOrder {
    pub job: JobId,
    pub server_id: ServerId,
    pub direction: Direction,
    pub remote_path: String,
    pub local_path: PathBuf,
    /// Source size when it is already known, so the UI can draw a determinate bar.
    pub size: Option<Bytes>,
    pub order: Order,
    /// The settings default, when there is one. `None` opens the conflict sheet.
    pub conflict: Option<ConflictAction>,
}

/// Sent by a transfer task back to its actor. Keeps lane accounting in one place
/// rather than behind a shared counter nobody owns.
enum Internal {
    LaneFinished(JobId),
}

/// Everything the actor needs that is not the connection itself.
pub struct SessionContext {
    pub id: SessionId,
    pub events: mpsc::Sender<EngineEvent>,
    pub interact: Arc<dyn Interact>,
    pub secrets: Arc<dyn SecretSource>,
    pub rt: tokio::runtime::Handle,
}

/// The handle the engine keeps. Cloning is deliberately not offered: one owner, and
/// `close` means closed.
pub struct SessionHandle {
    id: SessionId,
    cmd: mpsc::Sender<SessionCmd>,
    cancel: CancellationToken,
    join: StdMutex<Option<JoinHandle<()>>>,
}

impl SessionHandle {
    /// Start the actor. Returns as soon as the task is spawned — connecting happens
    /// inside it and its outcome arrives as `SessionState` events, because a connect
    /// can sit on a host-key sheet for minutes and no command should block on that.
    pub fn spawn(backend: Box<dyn Protocol>, cfg: ServerConfig, ctx: SessionContext) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel(CMD_BUFFER);
        let cancel = CancellationToken::new();

        let actor = SessionActor {
            id: ctx.id,
            backend,
            out: Emitter {
                id: ctx.id,
                events: ctx.events.clone(),
                listing_seq: Arc::new(AtomicU64::new(0)),
            },
            interact: Arc::clone(&ctx.interact),
            cancel: cancel.clone(),
            rt: ctx.rt.clone(),
            max_lanes: MAX_ADHOC_LANES,
            in_flight: 0,
            pending: VecDeque::new(),
            running: HashMap::new(),
        };
        let join = ctx.rt.spawn(actor.run(cfg, ctx.secrets, cmd_rx));

        Self {
            id: ctx.id,
            cmd: cmd_tx,
            cancel,
            join: StdMutex::new(Some(join)),
        }
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    async fn call<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<Result<T>>) -> SessionCmd,
    ) -> Result<T> {
        let (tx, rx) = oneshot::channel();
        self.cmd.send(make(tx)).await.map_err(|_| closed())?;
        rx.await.map_err(|_| closed())?
    }

    pub async fn list_dir(&self, path: &str) -> Result<Vec<RemoteEntry>> {
        self.call(|reply| SessionCmd::List {
            path: path.to_string(),
            reply,
        })
        .await
    }

    pub async fn stat(&self, path: &str) -> Result<Option<RemoteEntry>> {
        self.call(|reply| SessionCmd::Stat {
            path: path.to_string(),
            reply,
        })
        .await
    }

    pub async fn mkdir(&self, path: &str) -> Result<()> {
        self.call(|reply| SessionCmd::Mkdir {
            path: path.to_string(),
            reply,
        })
        .await
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        self.call(|reply| SessionCmd::Rename {
            from: from.to_string(),
            to: to.to_string(),
            reply,
        })
        .await
    }

    pub async fn remove_file(&self, path: &str) -> Result<()> {
        self.call(|reply| SessionCmd::RemoveFile {
            path: path.to_string(),
            reply,
        })
        .await
    }

    pub async fn remove_dir(&self, path: &str) -> Result<()> {
        self.call(|reply| SessionCmd::RemoveDir {
            path: path.to_string(),
            reply,
        })
        .await
    }

    pub async fn read_file(&self, path: &str, max: u64) -> Result<Vec<u8>> {
        self.call(|reply| SessionCmd::ReadFile {
            path: path.to_string(),
            max,
            reply,
        })
        .await
    }

    pub async fn cancel_job(&self, job: JobId) -> Result<()> {
        self.call(|reply| SessionCmd::CancelJob { job, reply })
            .await
    }

    pub async fn transfer(&self, order: TransferOrder) -> Result<()> {
        self.cmd
            .send(SessionCmd::Transfer(Box::new(order)))
            .await
            .map_err(|_| closed())
    }

    /// Stop everything this session owns and wait for it, bounded.
    ///
    /// Cancel before disconnect: a transfer blocked on an unanswered prompt only wakes
    /// because its session's prompts get denied, which the engine does between these
    /// two steps.
    pub async fn close(&self) {
        self.cancel.cancel();
        // Best effort: if the actor has already exited, the send fails and that is fine.
        let _ = self.cmd.send(SessionCmd::Disconnect).await;

        let handle = self.join.lock().expect("session join poisoned").take();
        let Some(handle) = handle else { return };
        // Abort is the fallback, not the plan: cancellation reaches into in-flight
        // operations, so a session that still has not stopped is one that cannot.
        let aborter = handle.abort_handle();
        if tokio::time::timeout(CLOSE_GRACE, handle).await.is_err() {
            tracing::warn!(session = %self.id, "session did not stop in time; aborting it");
            aborter.abort();
        }
    }

    /// Cancellation for children of this session, so a job token dies with its session.
    pub fn child_token(&self) -> CancellationToken {
        self.cancel.child_token()
    }
}

fn closed() -> EngineError {
    EngineError::protocol("the session is closed")
}

/// The deadline and the cancellation every remote operation gets.
///
/// A free function, not a method: the actor owns the backend exclusively, so
/// `self.op(self.backend.list(..))` would borrow `self` twice — hence the token
/// arriving as a separate argument rather than through `&self`.
///
/// Cancellation matters as much as the deadline here. Without it a listing that a
/// server accepted and never answered would hold the actor's loop for the whole
/// deadline, so closing the tab would wait too, and the caller would sit on a reply
/// channel belonging to a session that is already gone.
async fn with_deadline<T>(
    fut: impl std::future::Future<Output = Result<T>>,
    name: &str,
    deadline: Duration,
    cancel: &CancellationToken,
) -> Result<T> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(EngineError::Cancelled),
        result = tokio::time::timeout(deadline, fut) => match result {
            Ok(result) => result,
            Err(_) => Err(EngineError::Timeout {
                operation: name.to_string(),
                after_secs: deadline.as_secs() as u32,
            }),
        },
    }
}

/// The half of a session that talks to the UI.
///
/// Split from [`SessionActor`] for a concrete reason: an `async fn(&self)` on the actor
/// holds `&SessionActor` across an await, which would make the actor's future require
/// `Protocol: Sync`. Backends are `Send` and not `Sync` — each is owned by exactly one
/// task, which is the entire design. Emitting borrows only this, which is both.
#[derive(Clone)]
struct Emitter {
    id: SessionId,
    events: mpsc::Sender<EngineEvent>,
    /// Monotonic per session. A pane ignores a listing older than the one it is waiting
    /// for, so a slow reply cannot replace the directory the user already moved to.
    listing_seq: Arc<AtomicU64>,
}

impl Emitter {
    async fn emit(&self, event: EngineEvent) {
        // A closed channel means the engine is shutting down; there is no one to tell.
        let _ = self.events.send(event).await;
    }

    async fn log(&self, kind: LogKind, line: impl Into<String>) {
        self.emit(EngineEvent::Log {
            id: self.id,
            line: log_line(kind, line),
        })
        .await;
    }

    async fn listing(&self, path: &str, entries: Vec<RemoteEntry>) {
        let request = Seq(self.listing_seq.fetch_add(1, Ordering::SeqCst) + 1);
        self.emit(EngineEvent::Listing {
            listing: Box::new(ListingSnapshot {
                session: self.id,
                path: path.to_string(),
                request,
                entries,
                at: Utc::now(),
            }),
        })
        .await;
    }

    async fn job(&self, order: &TransferOrder, state: JobState, transferred: Bytes) {
        self.emit(EngineEvent::JobUpdate {
            job: Box::new(snapshot_for(self.id, order, state, transferred, None, None)),
        })
        .await;
    }

    /// Successful mutations belong in the activity feed. Failures already reach the
    /// caller as an error, so they are logged rather than announced twice.
    async fn audit(&self, out: &Result<()>, text: String) {
        match out {
            Ok(()) => {
                self.emit(EngineEvent::Activity {
                    id: self.id,
                    entry: ActivityEntry {
                        id: Uuid::new_v4(),
                        at: Utc::now(),
                        text,
                        detail: None,
                    },
                })
                .await;
            }
            Err(err) => self.log(LogKind::Error, err.to_string()).await,
        }
    }

    async fn disconnected(&self, reason: impl Into<String>, unexpected: bool) {
        self.emit(EngineEvent::SessionState {
            id: self.id,
            state: SessionState::Disconnected {
                reason: reason.into(),
                unexpected,
            },
        })
        .await;
    }
}

struct SessionActor {
    id: SessionId,
    backend: Box<dyn Protocol>,
    out: Emitter,
    interact: Arc<dyn Interact>,
    cancel: CancellationToken,
    rt: tokio::runtime::Handle,
    max_lanes: u8,
    in_flight: u8,
    pending: VecDeque<Box<TransferOrder>>,
    /// Cancellation tokens for the transfers this session is running, so `CancelJob`
    /// reaches the right one without a second map living somewhere else.
    running: HashMap<JobId, CancellationToken>,
}

impl SessionActor {
    async fn run(
        mut self,
        cfg: ServerConfig,
        secrets: Arc<dyn SecretSource>,
        mut cmd_rx: mpsc::Receiver<SessionCmd>,
    ) {
        let (internal_tx, mut internal_rx) =
            mpsc::channel::<Internal>(MAX_ADHOC_LANES as usize + 1);

        self.out
            .emit(EngineEvent::SessionOpened {
                session: Box::new(Session {
                    id: self.id,
                    server_id: cfg.id,
                    name: cfg.name.clone(),
                    proto: cfg.proto,
                    state: SessionState::Connecting,
                    latency_ms: None,
                    remote_path: None,
                }),
            })
            .await;
        self.out
            .log(
                LogKind::Status,
                format!("Connecting to {}…", cfg.endpoint()),
            )
            .await;

        let connected = tokio::select! {
            // A connect can park on a host-key sheet; closing the tab must still work.
            _ = self.cancel.cancelled() => {
                self.out.disconnected("closed before the connection was established", false).await;
                self.backend.disconnect().await;
                return;
            }
            result = self.backend.connect(&cfg, secrets.as_ref(), Arc::clone(&self.interact)) => result,
        };

        let info = match connected {
            Ok(info) => info,
            Err(err) => {
                self.out.log(LogKind::Error, err.to_string()).await;
                // Declining a host key is a decision, not a fault: no "connection lost".
                let unexpected = !matches!(
                    err,
                    EngineError::TrustRejected { .. } | EngineError::Cancelled
                );
                self.out.disconnected(err.to_string(), unexpected).await;
                self.backend.disconnect().await;
                return;
            }
        };

        let home = cfg
            .initial_remote_path
            .clone()
            .unwrap_or_else(|| info.home_path.clone());
        self.max_lanes = MAX_ADHOC_LANES.min(self.backend.capabilities().max_lanes.max(1));
        self.out.log(LogKind::Response, "Authenticated.").await;
        self.out
            .emit(EngineEvent::SessionState {
                id: self.id,
                state: SessionState::Connected {
                    info: Box::new(info),
                },
            })
            .await;

        // The landing directory, so a fresh tab is not empty while the user waits.
        let cancel = self.cancel.clone();
        if let Ok(entries) =
            with_deadline(self.backend.list(&home), "list", OP_DEADLINE, &cancel).await
        {
            self.out.listing(&home, entries).await;
        }

        let mut keepalive = tokio::time::interval(KEEPALIVE);
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        keepalive.tick().await; // The first tick is immediate, and we just connected.

        loop {
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => break,
                Some(Internal::LaneFinished(job)) = internal_rx.recv() => {
                    self.running.remove(&job);
                    self.in_flight = self.in_flight.saturating_sub(1);
                    if let Some(next) = self.pending.pop_front() {
                        self.start_transfer(next, &internal_tx).await;
                    }
                }
                cmd = cmd_rx.recv() => {
                    let Some(cmd) = cmd else { break };
                    if matches!(cmd, SessionCmd::Disconnect) {
                        break;
                    }
                    // A broken connection ends the actor; it has already said so.
                    if self.handle(cmd, &internal_tx).await.is_break() {
                        return;
                    }
                }
                _ = keepalive.tick() => {
                    if self.keepalive().await.is_break() {
                        return;
                    }
                }
            }
        }

        self.out.log(LogKind::Status, "Disconnecting.").await;
        self.backend.disconnect().await;
        self.out.disconnected("closed", false).await;
    }

    /// Run one command. `Break` means the connection is gone and the actor is done.
    async fn handle(
        &mut self,
        cmd: SessionCmd,
        internal_tx: &mpsc::Sender<Internal>,
    ) -> ControlFlow<()> {
        let cancel = self.cancel.clone();
        match cmd {
            SessionCmd::List { path, reply } => {
                let out =
                    with_deadline(self.backend.list(&path), "list", OP_DEADLINE, &cancel).await;
                if let Ok(entries) = &out {
                    self.out.listing(&path, entries.clone()).await;
                }
                let flow = self.check_fatal(&out).await;
                let _ = reply.send(out);
                flow
            }
            SessionCmd::Stat { path, reply } => {
                let out =
                    with_deadline(self.backend.stat(&path), "stat", OP_DEADLINE, &cancel).await;
                let flow = self.check_fatal(&out).await;
                let _ = reply.send(out);
                flow
            }
            SessionCmd::Mkdir { path, reply } => {
                let out =
                    with_deadline(self.backend.mkdir(&path), "mkdir", OP_DEADLINE, &cancel).await;
                self.out.audit(&out, format!("Created {path}")).await;
                let flow = self.check_fatal(&out).await;
                let _ = reply.send(out);
                flow
            }
            SessionCmd::Rename { from, to, reply } => {
                let out = with_deadline(
                    self.backend.rename(&from, &to),
                    "rename",
                    OP_DEADLINE,
                    &cancel,
                )
                .await;
                self.out
                    .audit(&out, format!("Renamed {from} to {to}"))
                    .await;
                let flow = self.check_fatal(&out).await;
                let _ = reply.send(out);
                flow
            }
            SessionCmd::RemoveFile { path, reply } => {
                let out = with_deadline(
                    self.backend.remove_file(&path),
                    "delete",
                    OP_DEADLINE,
                    &cancel,
                )
                .await;
                self.out.audit(&out, format!("Deleted {path}")).await;
                let flow = self.check_fatal(&out).await;
                let _ = reply.send(out);
                flow
            }
            SessionCmd::RemoveDir { path, reply } => {
                let out = with_deadline(
                    self.backend.remove_dir(&path),
                    "delete",
                    OP_DEADLINE,
                    &cancel,
                )
                .await;
                self.out.audit(&out, format!("Deleted {path}")).await;
                let flow = self.check_fatal(&out).await;
                let _ = reply.send(out);
                flow
            }
            SessionCmd::ReadFile { path, max, reply } => {
                let out = with_deadline(
                    self.backend.read_file(&path, max),
                    "read",
                    OP_DEADLINE,
                    &cancel,
                )
                .await;
                let flow = self.check_fatal(&out).await;
                let _ = reply.send(out);
                flow
            }
            SessionCmd::Transfer(order) => {
                if self.in_flight >= self.max_lanes {
                    self.out.job(&order, JobState::Queued, Bytes::ZERO).await;
                    self.pending.push_back(order);
                } else {
                    self.start_transfer(order, internal_tx).await;
                }
                ControlFlow::Continue(())
            }
            SessionCmd::CancelJob { job, reply } => {
                let _ = reply.send(self.cancel_job(job).await);
                ControlFlow::Continue(())
            }
            // The run loop takes this one before it reaches here.
            SessionCmd::Disconnect => ControlFlow::Break(()),
        }
    }

    /// Cancel one job, whether it is moving bytes or still waiting for a lane.
    ///
    /// A queued order has no task to interrupt, so it is dropped here and reported as
    /// cancelled directly — otherwise it would start later and surprise the person who
    /// already told it to stop.
    async fn cancel_job(&mut self, job: JobId) -> Result<()> {
        if let Some(token) = self.running.get(&job) {
            token.cancel();
            return Ok(());
        }
        if let Some(index) = self.pending.iter().position(|o| o.job == job) {
            let order = self.pending.remove(index).expect("index from position");
            self.out.job(&order, JobState::Cancelled, Bytes::ZERO).await;
            return Ok(());
        }
        Err(EngineError::NotFound {
            path: job.to_string(),
        })
    }

    /// A network failure means the connection is gone. Anything else is one operation
    /// failing on a session that still works — a missing path is not a dead session.
    async fn check_fatal<T>(&mut self, out: &Result<T>) -> ControlFlow<()> {
        match out {
            Err(err @ EngineError::Network { .. }) => {
                self.out.log(LogKind::Error, err.to_string()).await;
                self.backend.disconnect().await;
                self.out.disconnected(err.to_string(), true).await;
                ControlFlow::Break(())
            }
            _ => ControlFlow::Continue(()),
        }
    }

    async fn keepalive(&mut self) -> ControlFlow<()> {
        let started = Instant::now();
        let cancel = self.cancel.clone();
        match with_deadline(
            self.backend.noop(),
            "keepalive",
            KEEPALIVE_DEADLINE,
            &cancel,
        )
        .await
        {
            Ok(()) => {
                let ms = started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
                self.out
                    .emit(EngineEvent::Latency { id: self.id, ms })
                    .await;
                ControlFlow::Continue(())
            }
            Err(err) => {
                // Unlike a browse operation, a failed keepalive *is* the liveness
                // check: there is nothing else it could mean.
                self.out
                    .log(LogKind::Error, format!("Keepalive failed: {err}"))
                    .await;
                self.backend.disconnect().await;
                self.out.disconnected(err.to_string(), true).await;
                ControlFlow::Break(())
            }
        }
    }

    /// Prepare a transfer and hand it to its own task.
    ///
    /// Everything needing the backend happens here — the destination stat, opening the
    /// lane — because the actor is the only thing allowed to touch it. The task gets
    /// facts and a lane, and never reaches back.
    async fn start_transfer(
        &mut self,
        order: Box<TransferOrder>,
        internal_tx: &mpsc::Sender<Internal>,
    ) {
        self.out.job(&order, JobState::Preparing, Bytes::ZERO).await;

        let cancel = self.cancel.clone();
        let destination = match order.direction {
            Direction::Down => local_facts(&order.local_path).await,
            Direction::Up => {
                let stat = with_deadline(
                    self.backend.stat(&order.remote_path),
                    "stat",
                    OP_DEADLINE,
                    &cancel,
                )
                .await;
                match stat {
                    Ok(entry) => entry.map(|e| remote_facts(&order.remote_path, &e)),
                    Err(err) => {
                        self.out.job(&order, failed(err), Bytes::ZERO).await;
                        return;
                    }
                }
            }
        };

        let lane = match self.backend.open_lane().await {
            Ok(lane) => lane,
            Err(err) => {
                self.out.job(&order, failed(err), Bytes::ZERO).await;
                return;
            }
        };

        // A child of the session token, so closing the tab stops every transfer on it
        // without the engine having to enumerate them.
        let cancel = self.cancel.child_token();
        self.running.insert(order.job, cancel.clone());
        self.in_flight += 1;
        self.rt.spawn(run_transfer(TransferTask {
            session: self.id,
            order,
            destination,
            lane,
            events: self.out.events.clone(),
            interact: Arc::clone(&self.interact),
            cancel,
            done: internal_tx.clone(),
        }));
    }
}

fn failed(error: EngineError) -> JobState {
    JobState::Failed { error, attempts: 1 }
}

/// Everything one transfer owns, once the actor has let go of it.
struct TransferTask {
    session: SessionId,
    order: Box<TransferOrder>,
    /// Facts about what is already at the destination, when anything is.
    destination: Option<FileFacts>,
    lane: Box<dyn TransferLane>,
    events: mpsc::Sender<EngineEvent>,
    interact: Arc<dyn Interact>,
    cancel: CancellationToken,
    done: mpsc::Sender<Internal>,
}

async fn run_transfer(task: TransferTask) {
    let TransferTask {
        session,
        order,
        destination,
        mut lane,
        events,
        interact,
        cancel,
        done,
    } = task;

    let decision = resolve_conflict(&Conflict {
        session,
        order: &order,
        destination: destination.as_ref(),
        interact: interact.as_ref(),
        events: &events,
    })
    .await;

    if let Err(state) = decision {
        publish(
            &events,
            snapshot_for(session, &order, state, Bytes::ZERO, None, None),
        )
        .await;
        lane.close().await;
        let _ = done.send(Internal::LaneFinished(order.job)).await;
        return;
    }

    let started_at = Utc::now();
    publish(
        &events,
        snapshot_for(
            session,
            &order,
            JobState::Transferring,
            Bytes::ZERO,
            None,
            Some(started_at),
        ),
    )
    .await;

    let progress = {
        let events = events.clone();
        let order = order.clone();
        let rate = Rate::new();
        ProgressSink::new(move |bytes| {
            let job = snapshot_for(
                session,
                &order,
                JobState::Transferring,
                Bytes(bytes),
                rate.observe(bytes),
                Some(started_at),
            );
            // Dropping a tick under backpressure is safe: the hub coalesces progress
            // anyway, and every terminal state is sent with `send`, not `try_send`.
            let _ = events.try_send(EngineEvent::JobUpdate { job: Box::new(job) });
        })
    };

    let req = TransferReq {
        job: order.job,
        remote_path: order.remote_path.clone(),
        local_path: order.local_path.clone(),
        // Phase 1 never resumes. An offset is only legitimate once phase 2 has verified
        // the source facts *and* the partial file's content prefix (ADR 005).
        offset: 0,
        progress,
        cancel,
    };

    let outcome: Result<TransferOutcome> = match order.direction {
        Direction::Down => lane.download(req).await,
        Direction::Up => lane.upload(req).await,
    };
    // Unconditionally: after a cancellation the lane's protocol state is uncertain, so
    // it is discarded rather than handed back to anything.
    lane.close().await;

    let (state, transferred) = match outcome {
        Ok(out) => (JobState::Done { at: Utc::now() }, Bytes(out.final_size)),
        Err(EngineError::Cancelled) => (JobState::Cancelled, Bytes::ZERO),
        Err(error) => (failed(error), Bytes::ZERO),
    };
    publish(
        &events,
        snapshot_for(session, &order, state, transferred, None, Some(started_at)),
    )
    .await;
    let _ = done.send(Internal::LaneFinished(order.job)).await;
}

struct Conflict<'a> {
    session: SessionId,
    order: &'a TransferOrder,
    destination: Option<&'a FileFacts>,
    interact: &'a dyn Interact,
    events: &'a mpsc::Sender<EngineEvent>,
}

/// Decide what happens to a destination that already exists.
///
/// `Ok(())` is the only way through; every other answer is a terminal state for the
/// job, which is what the `Err` carries.
async fn resolve_conflict(ctx: &Conflict<'_>) -> std::result::Result<(), JobState> {
    let Some(existing) = ctx.destination else {
        // Nothing is in the way, so there is nothing to ask about.
        return Ok(());
    };

    let action = match ctx.order.conflict {
        Some(policy) => policy,
        None => {
            // The id is chosen here so the drawer can say what the job is waiting for
            // *before* an answer exists.
            let prompt_id: PromptId = Uuid::new_v4();
            publish(
                ctx.events,
                snapshot_for(
                    ctx.session,
                    ctx.order,
                    JobState::AwaitingPrompt { prompt: prompt_id },
                    Bytes::ZERO,
                    None,
                    None,
                ),
            )
            .await;

            let (local, remote) = match ctx.order.direction {
                Direction::Down => (existing.clone(), source_facts(ctx.order)),
                Direction::Up => (source_facts(ctx.order), existing.clone()),
            };
            let reply = ctx
                .interact
                .ask_with_id(
                    prompt_id,
                    ctx.session,
                    Prompt::Conflict {
                        local,
                        remote,
                        direction: ctx.order.direction,
                        remaining: 0,
                        // Phase 2 owns verified resume. Offering it here would be
                        // offering a guarantee nothing has established yet.
                        resume_allowed: false,
                    },
                )
                .await;
            match reply {
                PromptReply::Conflict { action, .. } => action,
                // Dismissing the sheet is a decision to leave the file alone.
                _ => ConflictAction::Skip,
            }
        }
    };

    match action {
        ConflictAction::Overwrite => Ok(()),
        // No `Skipped` state exists: the drawer draws this as a job that did not run,
        // which is what happened. Phase 2 revisits it along with the scheduler.
        ConflictAction::Skip => Err(JobState::Cancelled),
        ConflictAction::KeepBoth | ConflictAction::Resume => {
            Err(failed(EngineError::Unsupported {
                operation: "keep-both and resume arrive with the phase 2 queue".into(),
            }))
        }
    }
}

async fn publish(events: &mpsc::Sender<EngineEvent>, job: JobSnapshot) {
    let _ = events
        .send(EngineEvent::JobUpdate { job: Box::new(job) })
        .await;
}

fn snapshot_for(
    session: SessionId,
    order: &TransferOrder,
    state: JobState,
    transferred: Bytes,
    speed_bps: Option<Bytes>,
    started_at: Option<DateTime<Utc>>,
) -> JobSnapshot {
    let eta_secs = match (order.size, speed_bps) {
        (Some(size), Some(speed)) if speed.get() > 0 && size.get() > transferred.get() => {
            Some(((size.get() - transferred.get()) / speed.get()).min(u64::from(u32::MAX)) as u32)
        }
        _ => None,
    };
    JobSnapshot {
        id: order.job,
        session,
        server_id: order.server_id,
        direction: order.direction,
        remote_path: order.remote_path.clone(),
        local_path: order.local_path.clone(),
        size: order.size,
        transferred,
        state,
        order: order.order,
        speed_bps,
        eta_secs,
        conflict_policy: order.conflict,
        parent: None,
        started_at,
    }
}

/// What we know about the file the transfer reads from. Phase 1 computes no digest —
/// nothing here authorises a resume, and a `None` digest says so out loud.
fn source_facts(order: &TransferOrder) -> FileFacts {
    let path = match order.direction {
        Direction::Down => order.remote_path.clone(),
        Direction::Up => order.local_path.display().to_string(),
    };
    FileFacts {
        path,
        size: order.size.unwrap_or(Bytes::ZERO),
        modified: None,
        digest: None,
    }
}

async fn local_facts(path: &Path) -> Option<FileFacts> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    Some(FileFacts {
        path: path.display().to_string(),
        size: Bytes(meta.len()),
        modified: meta.modified().ok().map(DateTime::<Utc>::from),
        digest: None,
    })
}

fn remote_facts(path: &str, entry: &RemoteEntry) -> FileFacts {
    FileFacts {
        path: path.to_string(),
        size: entry.size,
        modified: entry.modified,
        digest: None,
    }
}

/// Bytes per second over a short trailing window.
///
/// A cumulative average is the wrong number for a progress row: after the first few
/// seconds it barely moves, so a stalled transfer goes on reporting a healthy rate.
/// This reports the slope across recent samples instead, which does fall to zero.
struct Rate {
    samples: StdMutex<VecDeque<(Instant, u64)>>,
}

const RATE_WINDOW: Duration = Duration::from_secs(3);
/// Below this the sample span is too short to divide by without inventing precision.
const RATE_MIN_SPAN: Duration = Duration::from_millis(400);

impl Rate {
    fn new() -> Self {
        Self {
            samples: StdMutex::new(VecDeque::new()),
        }
    }

    fn observe(&self, bytes: u64) -> Option<Bytes> {
        let now = Instant::now();
        let mut samples = self.samples.lock().expect("rate window poisoned");
        samples.push_back((now, bytes));
        while samples.len() > 2 {
            match samples.front() {
                Some(&(at, _)) if now.duration_since(at) > RATE_WINDOW => {
                    samples.pop_front();
                }
                _ => break,
            }
        }
        let &(first_at, first_bytes) = samples.front()?;
        let span = now.duration_since(first_at);
        if span < RATE_MIN_SPAN {
            return None;
        }
        let moved = bytes.saturating_sub(first_bytes) as f64;
        Some(Bytes((moved / span.as_secs_f64()) as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order() -> TransferOrder {
        TransferOrder {
            job: Uuid::new_v4(),
            server_id: Uuid::nil(),
            direction: Direction::Down,
            remote_path: "/x".into(),
            local_path: PathBuf::from("/tmp/x"),
            size: None,
            order: Order(0),
            conflict: None,
        }
    }

    #[test]
    fn a_rate_needs_a_real_span_before_it_reports() {
        let rate = Rate::new();
        assert_eq!(rate.observe(0), None, "one sample is not a rate");
        assert_eq!(rate.observe(1024), None, "nor is a span of microseconds");
    }

    #[test]
    fn eta_needs_both_a_size_and_a_speed() {
        let job = snapshot_for(
            Uuid::new_v4(),
            &order(),
            JobState::Transferring,
            Bytes(10),
            Some(Bytes(100)),
            None,
        );
        assert_eq!(job.eta_secs, None, "no size means no honest estimate");

        let sized = TransferOrder {
            size: Some(Bytes(1000)),
            ..order()
        };
        let job = snapshot_for(
            Uuid::new_v4(),
            &sized,
            JobState::Transferring,
            Bytes(200),
            Some(Bytes(100)),
            None,
        );
        assert_eq!(
            job.eta_secs,
            Some(8),
            "800 bytes left at 100 bytes a second"
        );
    }
}
