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

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

use crate::model::{
    Direction, FileFacts, JobId, PromptId, RemoteEntry, ServerConfig, Session, SessionId,
    SessionState,
};
use crate::protocol::{
    CheckpointSink, ProgressSink, Protocol, SecretSource, TransferLane, TransferReq, Transferred,
};
use crate::queue::ResumeRecord;
use crate::resume;
use crate::scheduler::{Report, Reporter, RunRequest, Scheduler};
use crate::wire::{Bytes, Seq};

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
/// The most transfer lanes this build will run on one session, whatever the backend
/// says it could take.
///
/// The scheduler's per-session cap is the smaller of this, the backend's own
/// `max_lanes`, and the concurrency slider. It exists so a backend that advertises a
/// generous limit cannot, on its own, turn one server into the whole queue.
pub const MAX_SESSION_LANES: u8 = 4;
/// Commands buffered before a caller has to wait for the actor.
const CMD_BUFFER: usize = 64;
/// How many times a lost connection is retried before it waits for a person.
///
/// The queue's jobs stay paused either way; this is only how long the session keeps
/// trying on its own before saying so and leaving the decision to someone who might
/// know whether the server is coming back.
pub const MAX_RECONNECTS: u32 = 10;
/// The wait before each attempt, in seconds, holding at the last one.
///
/// Quick at first, because most drops are a few seconds of nothing; slow later,
/// because a server that has been gone for a minute is not usually about to return
/// within one, and hammering it does not help.
const BACKOFF_SECS: [u64; 5] = [1, 2, 5, 10, 30];
/// Lane requests and completions buffered before a transfer task has to wait.
const LANE_BUFFER: usize = 8;
/// How long an unused lane is kept before it is closed.
pub const LANE_IDLE: Duration = Duration::from_secs(60);
/// How often idle lanes are looked at. Half the idle time, so a lane lives between
/// sixty and ninety seconds past its last use — close enough, and one timer.
const LANE_SWEEP: Duration = Duration::from_secs(30);

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
    /// Run one job the scheduler has already decided to start. Fire and forget: the
    /// job's life is told through its `Reporter`.
    Transfer(Box<RunRequest>),
    /// Try again now, and start the backoff over. A person pressing this knows
    /// something the timer does not.
    Reconnect,
    /// Stop one transfer. The actor owns its jobs' cancellation tokens, so nothing
    /// outside it has to keep a parallel map that could disagree.
    CancelJob {
        job: JobId,
        /// True for a pause, which is coming back to the bytes already written; false
        /// for a cancellation, which is not.
        keep_partial: bool,
        reply: oneshot::Sender<Result<()>>,
    },
    Disconnect,
}

/// Sent by a transfer task back to its actor, so the cancellation map is cleaned up
/// where it is owned rather than behind a shared counter nobody owns.
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
    /// Told when this session can take work and when it cannot. The session does not
    /// decide what runs; it reports whether it is able to run anything.
    pub queue: Scheduler,
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

        let (lane_tx, lane_rx) = mpsc::channel(LANE_BUFFER);
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
            queue: ctx.queue.clone(),
            lane_tx,
            idle_lanes: Vec::new(),
            running: HashMap::new(),
        };
        let join = ctx.rt.spawn(actor.run(cfg, ctx.secrets, cmd_rx, lane_rx));

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

    pub async fn cancel_job(&self, job: JobId, keep_partial: bool) -> Result<()> {
        self.call(|reply| SessionCmd::CancelJob {
            job,
            keep_partial,
            reply,
        })
        .await
    }

    /// Try to connect again now, whatever the backoff was going to do.
    pub async fn reconnect(&self) -> Result<()> {
        self.cmd
            .send(SessionCmd::Reconnect)
            .await
            .map_err(|_| closed())
    }

    pub async fn transfer(&self, run: RunRequest) -> Result<()> {
        self.cmd
            .send(SessionCmd::Transfer(Box::new(run)))
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
    queue: Scheduler,
    /// Where transfer tasks ask for a lane. They cannot open one themselves: the actor
    /// owns the backend, and this is the only way through it.
    lane_tx: mpsc::Sender<ActorRequest>,
    /// Lanes that finished a transfer and are waiting for the next one, with when each
    /// became idle. Bounded by the session's own cap, since a lane only ever comes back
    /// from a transfer that had one.
    idle_lanes: Vec<(Box<dyn TransferLane>, Instant)>,
    /// What the session is running, so a stop reaches the right transfer without a
    /// second map living somewhere else that could disagree.
    running: HashMap<JobId, Running>,
}

/// Why the serving loop stopped.
enum Outcome {
    /// The tab was closed, or the engine asked the session to stop.
    Closed,
    /// The connection went away. `retryable` is false for the answers a person has to
    /// give — a declined host key, rejected credentials — where retrying is either
    /// impossible or just the same rejection ten more times.
    Lost { reason: String, retryable: bool },
}

/// How a wait between attempts ended.
enum Idle {
    Reconnect,
    Close,
}

/// The handles on one in-flight transfer.
struct Running {
    cancel: CancellationToken,
    keep_partial: Arc<AtomicBool>,
}

impl SessionActor {
    async fn run(
        mut self,
        cfg: ServerConfig,
        secrets: Arc<dyn SecretSource>,
        mut cmd_rx: mpsc::Receiver<SessionCmd>,
        mut lane_rx: mpsc::Receiver<ActorRequest>,
    ) {
        let (internal_tx, mut internal_rx) = mpsc::channel::<Internal>(LANE_BUFFER);

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

        // Attempts since the last time this session was *working*, so a connection
        // that comes back and drops again gets the full allowance rather than the
        // remains of the previous outage's.
        let mut attempt = 0u32;
        loop {
            let outcome = match self.dial(&cfg, &secrets).await {
                Ok(max_lanes) => {
                    attempt = 0;
                    self.serve(
                        &mut cmd_rx,
                        &mut lane_rx,
                        &mut internal_rx,
                        &internal_tx,
                        max_lanes,
                    )
                    .await
                }
                Err(err) => Outcome::Lost {
                    retryable: worth_retrying(&err),
                    reason: err.to_string(),
                },
            };

            let Outcome::Lost { reason, retryable } = outcome else {
                break;
            };

            // Every transfer on this session is doomed: their lanes were channels on
            // the connection that just went. Stopping them here rather than waiting
            // for the queue to ask is what keeps a dying attempt from renaming its
            // partial onto the destination underneath the attempt that replaces it.
            // The bytes are kept — this is a pause, and the job is coming back to them.
            self.stop_all(true);
            // Pooled lanes are channels on the connection that just went. Dropping
            // them here means a reconnect opens fresh ones rather than handing a
            // transfer a channel to a server that is no longer there.
            self.idle_lanes.clear();
            // The queue pauses this session's work rather than failing it: the jobs
            // are fine, the connection is not.
            self.queue.session_down(self.id).await;
            self.backend.disconnect().await;

            attempt += 1;
            let give_up = !retryable || attempt > MAX_RECONNECTS;
            if give_up {
                self.out.disconnected(reason, retryable).await;
                // Not the end of the actor. A person can still press Reconnect, and
                // the queue still holds this session's paused jobs waiting for them
                // to. Exiting here would strand both.
                match self.idle(None, &mut cmd_rx).await {
                    Idle::Reconnect => attempt = 0,
                    Idle::Close => break,
                }
                continue;
            }

            let wait = backoff(attempt);
            self.out
                .log(
                    LogKind::Status,
                    format!(
                        "Connection lost: {reason}. Retrying in {}s…",
                        wait.as_secs()
                    ),
                )
                .await;
            self.out
                .emit(EngineEvent::SessionState {
                    id: self.id,
                    state: SessionState::Reconnecting {
                        attempt,
                        retry_in_secs: wait.as_secs() as u32,
                    },
                })
                .await;
            match self.idle(Some(wait), &mut cmd_rx).await {
                // Pressing "Reconnect now" resets the backoff: the person knows
                // something the timer does not.
                Idle::Reconnect => attempt = 0,
                Idle::Close => break,
            }
        }

        self.out.log(LogKind::Status, "Disconnecting.").await;
        for (lane, _) in std::mem::take(&mut self.idle_lanes) {
            lane.close().await;
        }
        self.backend.disconnect().await;
        self.out.disconnected("closed", false).await;
        self.queue.session_down(self.id).await;
    }

    /// Connect, land in a directory, and tell the queue this session can take work.
    async fn dial(&mut self, cfg: &ServerConfig, secrets: &Arc<dyn SecretSource>) -> Result<u8> {
        self.out
            .log(
                LogKind::Status,
                format!("Connecting to {}…", cfg.endpoint()),
            )
            .await;

        let connected = tokio::select! {
            // A connect can park on a host-key sheet; closing the tab must still work.
            _ = self.cancel.cancelled() => Err(EngineError::Cancelled),
            result = self.backend.connect(cfg, secrets.as_ref(), Arc::clone(&self.interact)) => result,
        };
        let info = match connected {
            Ok(info) => info,
            Err(err) => {
                self.out.log(LogKind::Error, err.to_string()).await;
                return Err(err);
            }
        };

        let home = cfg
            .initial_remote_path
            .clone()
            .unwrap_or_else(|| info.home_path.clone());
        let max_lanes = MAX_SESSION_LANES.min(self.backend.capabilities().max_lanes.max(1));
        self.out.log(LogKind::Response, "Authenticated.").await;
        self.out
            .emit(EngineEvent::SessionState {
                id: self.id,
                state: SessionState::Connected {
                    info: Box::new(info),
                },
            })
            .await;

        // The landing directory, so a fresh tab is not empty while the user waits, and
        // a reconnected one comes back where it was rather than blank.
        let cancel = self.cancel.clone();
        if let Ok(entries) =
            with_deadline(self.backend.list(&home), "list", OP_DEADLINE, &cancel).await
        {
            self.out.listing(&home, entries).await;
        }

        // The queue may dispatch to this session from here on, and not before. It also
        // resumes whatever the last drop paused, which is what makes a reconnect
        // continue the queue instead of merely restoring a tab.
        self.queue.session_up(self.id, cfg.id, max_lanes).await;
        Ok(max_lanes)
    }

    /// Serve commands until the connection dies or the session is closed.
    async fn serve(
        &mut self,
        cmd_rx: &mut mpsc::Receiver<SessionCmd>,
        lane_rx: &mut mpsc::Receiver<ActorRequest>,
        internal_rx: &mut mpsc::Receiver<Internal>,
        internal_tx: &mpsc::Sender<Internal>,
        _max_lanes: u8,
    ) -> Outcome {
        let mut keepalive = tokio::time::interval(KEEPALIVE);
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        keepalive.tick().await; // The first tick is immediate, and we just connected.

        let mut sweep = tokio::time::interval(LANE_SWEEP);
        sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        sweep.tick().await;

        loop {
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => return Outcome::Closed,
                _ = sweep.tick() => self.close_idle_lanes().await,
                Some(Internal::LaneFinished(job)) = internal_rx.recv() => {
                    self.running.remove(&job);
                }
                // A transfer asking the backend for something. Served here because
                // the actor is the only thing that may touch it.
                Some(request) = lane_rx.recv() => match request {
                    ActorRequest::Lane { reply } => {
                        // A channel that is already open beats opening another one: on
                        // a queue of small files the handshake is most of the work.
                        let lane = match self.idle_lanes.pop() {
                            Some((lane, _)) => Ok(lane),
                            None => self.backend.open_lane().await,
                        };
                        let _ = reply.send(lane);
                    }
                    ActorRequest::ReturnLane { lane } => {
                        self.idle_lanes.push((lane, Instant::now()));
                    }
                    ActorRequest::Exists { path, reply } => {
                        let taken = with_deadline(
                            self.backend.stat(&path), "stat", OP_DEADLINE, &self.cancel.clone(),
                        )
                        .await;
                        // A stat that fails is not proof the path is free, so a name
                        // that cannot be checked counts as taken and the search moves
                        // on. Guessing the other way overwrites somebody's file.
                        let _ = reply.send(!matches!(taken, Ok(None)));
                    }
                },
                cmd = cmd_rx.recv() => {
                    let Some(cmd) = cmd else { return Outcome::Closed };
                    match cmd {
                        SessionCmd::Disconnect => return Outcome::Closed,
                        // Already connected, so there is nothing to reconnect.
                        SessionCmd::Reconnect => {}
                        cmd => if let ControlFlow::Break(reason) =
                            self.handle(cmd, internal_tx).await
                        {
                            return Outcome::Lost { reason, retryable: true };
                        }
                    }
                }
                _ = keepalive.tick() => {
                    if let ControlFlow::Break(reason) = self.keepalive().await {
                        return Outcome::Lost { reason, retryable: true };
                    }
                }
            }
        }
    }

    /// Wait out a backoff, or wait indefinitely for a person.
    ///
    /// Commands keep being answered while this waits — with an error, because there is
    /// no connection behind them. A caller left holding a reply channel that never
    /// answers is worse than one told plainly that the session is down.
    async fn idle(
        &mut self,
        wait: Option<Duration>,
        cmd_rx: &mut mpsc::Receiver<SessionCmd>,
    ) -> Idle {
        let deadline = wait.map(|wait| tokio::time::Instant::now() + wait);
        loop {
            let tick = async {
                match deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => return Idle::Close,
                _ = tick => return Idle::Reconnect,
                cmd = cmd_rx.recv() => match cmd {
                    None | Some(SessionCmd::Disconnect) => return Idle::Close,
                    Some(SessionCmd::Reconnect) => return Idle::Reconnect,
                    // Stopping a transfer needs no connection, and a person cancelling
                    // a job while the banner counts down must not be told to wait for
                    // a server that may never come back.
                    Some(SessionCmd::CancelJob { job, keep_partial, reply }) => {
                        let _ = reply.send(self.cancel_job(job, keep_partial).await);
                    }
                    Some(cmd) => refuse(cmd),
                },
            }
        }
    }

    /// Run one command. `Break` means the connection is gone and the actor is done.
    async fn handle(
        &mut self,
        cmd: SessionCmd,
        internal_tx: &mpsc::Sender<Internal>,
    ) -> ControlFlow<String> {
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
            SessionCmd::Transfer(run) => {
                self.start_transfer(run, internal_tx).await;
                ControlFlow::Continue(())
            }
            SessionCmd::CancelJob {
                job,
                keep_partial,
                reply,
            } => {
                let _ = reply.send(self.cancel_job(job, keep_partial).await);
                ControlFlow::Continue(())
            }
            // The serving loop takes these before they reach here.
            SessionCmd::Disconnect | SessionCmd::Reconnect => ControlFlow::Continue(()),
        }
    }

    /// Close pooled lanes that have gone unused.
    ///
    /// A channel costs the server a file handle and a little memory whether or not
    /// anyone is transferring, so a session that moved a hundred files an hour ago
    /// should not still be holding four open connections for it.
    async fn close_idle_lanes(&mut self) {
        let now = Instant::now();
        let (keep, stale): (Vec<_>, Vec<_>) = std::mem::take(&mut self.idle_lanes)
            .into_iter()
            .partition(|(_, since)| now.duration_since(*since) < LANE_IDLE);
        self.idle_lanes = keep;
        for (lane, _) in stale {
            lane.close().await;
        }
    }

    /// Stop every transfer this session is running.
    ///
    /// Synchronous on purpose: it is called on the way out of a lost connection, and
    /// awaiting anything there would let a transfer get further before it is told.
    fn stop_all(&mut self, keep_partial: bool) {
        for running in self.running.values() {
            running.keep_partial.store(keep_partial, Ordering::SeqCst);
            running.cancel.cancel();
        }
    }

    /// Interrupt one running transfer.
    ///
    /// A job this session is not running is not an error. The queue lives elsewhere
    /// now, so by the time a cancellation arrives the transfer may have finished on
    /// its own, or may never have started — both are races the caller already handles,
    /// and neither is a failure to report.
    async fn cancel_job(&mut self, job: JobId, keep_partial: bool) -> Result<()> {
        if let Some(running) = self.running.get(&job) {
            // Set before cancelling, so the transfer cannot notice the cancellation
            // and act on a stale answer to "do these bytes matter".
            running.keep_partial.store(keep_partial, Ordering::SeqCst);
            running.cancel.cancel();
        }
        Ok(())
    }

    /// A network failure means the connection is gone. Anything else is one operation
    /// failing on a session that still works — a missing path is not a dead session.
    ///
    /// `Break` carries the reason rather than announcing it: the reconnect loop above
    /// decides whether this becomes a banner counting down or a disconnection, and it
    /// needs the words either way.
    async fn check_fatal<T>(&mut self, out: &Result<T>) -> ControlFlow<String> {
        match out {
            Err(err @ EngineError::Network { .. }) => {
                self.out.log(LogKind::Error, err.to_string()).await;
                ControlFlow::Break(err.to_string())
            }
            _ => ControlFlow::Continue(()),
        }
    }

    async fn keepalive(&mut self) -> ControlFlow<String> {
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
                ControlFlow::Break(err.to_string())
            }
        }
    }

    /// Prepare a transfer and hand it to its own task.
    ///
    /// Everything needing the backend happens here — the destination stat, opening the
    /// lane — because the actor is the only thing allowed to touch it. The task gets
    /// facts and a lane, and never reaches back.
    ///
    /// Whether this job should be running at all is not decided here. The scheduler
    /// owns that, and it has already spent a slot to ask for this one.
    async fn start_transfer(&mut self, run: Box<RunRequest>, internal_tx: &mpsc::Sender<Internal>) {
        let cancel = self.cancel.clone();
        let destination = match run.direction {
            Direction::Down => local_facts(&run.local_path).await,
            Direction::Up => {
                let stat = with_deadline(
                    self.backend.stat(&run.remote_path),
                    "stat",
                    OP_DEADLINE,
                    &cancel,
                )
                .await;
                match stat {
                    Ok(entry) => entry.map(|e| remote_facts(&run.remote_path, &e)),
                    Err(err) => {
                        run.report.send(finished(run.job, Err(err))).await;
                        return;
                    }
                }
            }
        };

        // The source as it is right now. Its size draws the progress bar; its size and
        // time together are the cheap half of deciding whether a partial may be
        // continued. Best effort: a server that withholds either is a state the drawer
        // and the resume check both already handle.
        let source = match run.direction {
            Direction::Down => with_deadline(
                self.backend.stat(&run.remote_path),
                "stat",
                OP_DEADLINE,
                &cancel,
            )
            .await
            .ok()
            .flatten()
            .map(|entry| remote_facts(&run.remote_path, &entry)),
            Direction::Up => local_facts(&run.local_path).await,
        };
        let size = source.as_ref().map(|facts| facts.size);

        // A child of the session token, so closing the tab stops every transfer on it
        // without the engine having to enumerate them.
        let cancel = self.cancel.child_token();
        // Discard by default: a transfer that stops for any reason other than a
        // deliberate pause leaves nothing behind.
        let keep_partial = Arc::new(AtomicBool::new(false));
        self.running.insert(
            run.job,
            Running {
                cancel: cancel.clone(),
                keep_partial: Arc::clone(&keep_partial),
            },
        );
        self.rt.spawn(run_transfer(TransferTask {
            session: self.id,
            run,
            size,
            source,
            destination,
            actor: self.lane_tx.clone(),
            interact: Arc::clone(&self.interact),
            cancel,
            keep_partial,
            done: internal_tx.clone(),
        }));
    }
}

/// Everything one transfer owns, once the actor has let go of it.
struct TransferTask {
    session: SessionId,
    run: Box<RunRequest>,
    size: Option<Bytes>,
    /// The source as it was when this transfer was dispatched.
    source: Option<FileFacts>,
    /// Facts about what is already at the destination, when anything is.
    destination: Option<FileFacts>,
    actor: mpsc::Sender<ActorRequest>,
    interact: Arc<dyn Interact>,
    cancel: CancellationToken,
    keep_partial: Arc<AtomicBool>,
    done: mpsc::Sender<Internal>,
}

/// What a transfer task needs the backend for.
///
/// It cannot touch the backend itself — the actor owns it exclusively — so it asks.
/// The lane is asked for *after* the conflict is settled: opening a channel for a file
/// that is about to be skipped costs a round trip and a lane the queue could have
/// spent on something that was going to move. The exception is a job with a resume
/// record, which needs a lane to read the remote prefix before it can decide whether
/// resuming is even on the menu.
enum ActorRequest {
    Lane {
        reply: oneshot::Sender<Result<Box<dyn TransferLane>>>,
    },
    /// A lane whose transfer finished cleanly, offered back for the next one.
    ///
    /// Only ever a lane that completed: after a cancellation or a protocol error its
    /// state is uncertain, and reusing it would carry that uncertainty into a transfer
    /// that has done nothing wrong.
    ReturnLane { lane: Box<dyn TransferLane> },
    /// Whether a remote path is taken, for finding a free name for keep-both.
    Exists {
        path: String,
        reply: oneshot::Sender<bool>,
    },
}

/// Ask the actor for a lane.
async fn open_lane(actor: &mpsc::Sender<ActorRequest>) -> Result<Box<dyn TransferLane>> {
    let (reply, wait) = oneshot::channel();
    match actor.send(ActorRequest::Lane { reply }).await {
        Ok(()) => wait.await.unwrap_or_else(|_| Err(closed())),
        Err(_) => Err(closed()),
    }
}

async fn run_transfer(task: TransferTask) {
    let TransferTask {
        session,
        run,
        size,
        source,
        destination,
        actor,
        interact,
        cancel,
        keep_partial,
        done,
    } = task;
    let job = run.job;
    let report = run.report.clone();
    let finish = |result| async {
        report.send(finished(job, result)).await;
        let _ = done.send(Internal::LaneFinished(job)).await;
    };

    // A job with a resume record needs its lane before the sheet opens, because
    // whether "Resume" is even offered depends on reading the remote prefix back.
    let mut lane = match &run.resume {
        Some(_) => match open_lane(&actor).await {
            Ok(lane) => Some(lane),
            Err(err) => return finish(Err(err)).await,
        },
        None => None,
    };

    // Nothing here decides to resume. It decides whether resuming is *offerable*, and
    // a refusal carries the reason so the sheet can say why the option is missing.
    let verified = match (&run.resume, lane.as_deref_mut()) {
        (Some(record), Some(lane)) => {
            match resume::check(
                resume::Verify {
                    record,
                    direction: run.direction,
                    source_now: source.as_ref(),
                    local_path: &run.local_path,
                    remote_path: &run.remote_path,
                },
                lane,
            )
            .await
            {
                Ok(prefix) => Some((record.checkpoint, prefix)),
                Err(reason) => {
                    tracing::info!(%job, reason, "resume refused; the transfer restarts");
                    None
                }
            }
        }
        _ => None,
    };

    let decision = resolve_conflict(&Conflict {
        session,
        run: &run,
        size,
        destination: destination.as_ref(),
        resume_allowed: verified.is_some(),
        interact: interact.as_ref(),
    })
    .await;

    let action = match decision {
        Ok(action) => action,
        Err(outcome) => return finish(outcome).await,
    };
    report
        .send(Report::Decided {
            job,
            action: action.action,
            apply_to_remaining: action.apply_to_remaining,
        })
        .await;
    if action.action == ConflictAction::Skip {
        // The scheduler finishes the job on the decision alone. A lane opened for the
        // resume check has done nothing wrong, so it goes back to the pool.
        if let Some(lane) = lane {
            release_lane(&actor, lane).await;
        }
        let _ = done.send(Internal::LaneFinished(job)).await;
        return;
    }

    // Keep-both means a new destination, so it is settled before anything opens: the
    // partial file's name and the finalisation target are both derived from it.
    let (remote_path, local_path) = match action.action {
        ConflictAction::KeepBoth => match free_name(&run, &actor).await {
            Ok(paths) => paths,
            Err(err) => return finish(Err(err)).await,
        },
        _ => (run.remote_path.clone(), run.local_path.clone()),
    };

    let mut lane = match lane {
        Some(lane) => lane,
        None => match open_lane(&actor).await {
            Ok(lane) => lane,
            Err(err) => return finish(Err(err)).await,
        },
    };

    // Only a verified checkpoint *and* a decision to use it produce an offset.
    let (offset, prefix) = match (action.action, verified) {
        (ConflictAction::Resume, Some((checkpoint, prefix))) => (checkpoint, Some(prefix)),
        _ => (Bytes::ZERO, None),
    };

    let temporary_path = temp_path(run.direction, &remote_path, &local_path, job);
    // Ownership is recorded before a byte moves, and the scheduler confirms it is on
    // disk before this returns. A partial with no record behind it is not resumable by
    // anything, which is the whole point of writing it first.
    report
        .started(Report::Started {
            job,
            size,
            resume_from: offset,
            record: ResumeRecord {
                checkpoint: offset,
                prefix_sha256: prefix
                    .as_ref()
                    .map_or_else(|| crate::queue::EMPTY_SHA256.to_string(), |p| p.snapshot()),
                ..ResumeRecord::new(
                    source.clone().unwrap_or_else(|| source_facts(&run, size)),
                    temporary_path,
                )
            },
        })
        .await;

    let progress = {
        let report = report.clone();
        ProgressSink::new(move |bytes| report.progress(job, Bytes(bytes)))
    };
    let checkpoint = {
        let report = report.clone();
        CheckpointSink::new(move |offset, digest| report.checkpoint(job, Bytes(offset), digest))
    };

    let req = TransferReq {
        job,
        remote_path: remote_path.clone(),
        local_path: local_path.clone(),
        // A verified offset or zero. Nothing else can put a number here.
        offset: offset.get(),
        prefix,
        progress,
        checkpoint,
        cancel,
        keep_partial,
    };

    let moved = match run.direction {
        Direction::Down => lane.download(&req).await,
        Direction::Up => lane.upload(&req).await,
    };
    let moved = match moved {
        Ok(moved) => moved,
        Err(err) => {
            // After a cancellation or a protocol error the lane's state is uncertain,
            // so it is closed rather than offered to the next transfer.
            lane.close().await;
            report.send(finished(job, Err(err))).await;
            let _ = done.send(Internal::LaneFinished(job)).await;
            return;
        }
    };

    // The bytes are in this job's temporary file and the destination is still
    // untouched. Everything below happens in that gap, which is the only place where
    // finding a problem still costs nothing.
    let outcome = publish(Publishing {
        job,
        run: &run,
        req: &req,
        moved,
        resumed: offset.get() > 0,
        lane: lane.as_mut(),
        report: &report,
    })
    .await;

    match &outcome {
        Ok(_) => release_lane(&actor, lane).await,
        Err(_) => lane.close().await,
    }
    report.send(finished(job, outcome)).await;
    let _ = done.send(Internal::LaneFinished(job)).await;
}

/// Offer a finished lane back to the actor, closing it if the actor has gone.
async fn release_lane(actor: &mpsc::Sender<ActorRequest>, lane: Box<dyn TransferLane>) {
    if let Err(rejected) = actor.send(ActorRequest::ReturnLane { lane }).await
        && let ActorRequest::ReturnLane { lane } = rejected.0
    {
        lane.close().await;
    }
}

struct Publishing<'a> {
    job: JobId,
    run: &'a RunRequest,
    req: &'a TransferReq,
    moved: Transferred,
    /// Only a resumed transfer is a splice of two attempts, and only a splice needs
    /// the source it was spliced from to have held still.
    resumed: bool,
    lane: &'a mut dyn TransferLane,
    report: &'a Reporter,
}

/// Check what arrived, then publish it.
///
/// The order is the whole point. A resumed transfer took its first bytes from one
/// reading of the source and its last from another, and if the source moved in between
/// the result is a file that never existed — the right length, the right name, and
/// wrong. Noticing that after the rename is noticing it too late: the destination the
/// user had is already gone.
async fn publish(ctx: Publishing<'_>) -> Result<Bytes> {
    let Publishing {
        job,
        run,
        req,
        moved,
        resumed,
        lane,
        report,
    } = ctx;

    if resumed {
        report.send(Report::Verifying { job }).await;
        if let Some(record) = &run.resume
            && let Err(reason) = resume::source_unchanged(
                &record.source,
                moved.source_now.as_ref(),
                Bytes(moved.final_size),
            )
        {
            lane.discard(&moved.temporary_path).await;
            return Err(EngineError::SourceChanged {
                path: format!("{}: {reason}", record.source.path),
            });
        }
    }

    // A destination that is not the length the source says it is has nothing to do
    // with resuming — a truncated write or a short read produces it too — so this is
    // checked for every transfer, not only for spliced ones.
    if moved.final_size != moved.bytes + req.offset {
        lane.discard(&moved.temporary_path).await;
        return Err(EngineError::IntegrityMismatch {
            expected: format!("{} bytes", moved.final_size),
            actual: format!("{} bytes", moved.bytes + req.offset),
        });
    }

    // The intent, recorded before the rename it describes. A crash in the window
    // between them leaves a destination that may already be correct, and a record that
    // says so is the difference between reconciling it and transferring it again.
    report
        .finalising(Report::Finalising {
            job,
            digest: moved.digest.clone(),
        })
        .await;

    lane.finalise(req, &moved)
        .await
        .map(|outcome| Bytes(outcome.final_size))
}

fn finished(job: JobId, result: Result<Bytes>) -> Report {
    Report::Finished { job, result }
}

struct Conflict<'a> {
    session: SessionId,
    run: &'a RunRequest,
    size: Option<Bytes>,
    destination: Option<&'a FileFacts>,
    /// Whether the engine has *already proven* a resume would be safe. The sheet
    /// offers the option only when this is true, so a person is never given a choice
    /// the engine cannot honour.
    resume_allowed: bool,
    interact: &'a dyn Interact,
}

/// What a person, or a policy, decided about an existing destination.
struct Decision {
    action: ConflictAction,
    apply_to_remaining: bool,
}

/// Decide what happens to a destination that already exists.
///
/// `Err` is a terminal outcome for the job, which is what it carries.
async fn resolve_conflict(ctx: &Conflict<'_>) -> std::result::Result<Decision, Result<Bytes>> {
    let Some(existing) = ctx.destination else {
        // Nothing is in the way, so there is nothing to ask about — but this job may
        // still own a verified partial from an earlier attempt. Continuing it is not a
        // conflict and needs no decision: the only file involved is one this engine
        // wrote and has just proven still matches the source.
        return Ok(Decision {
            action: if ctx.resume_allowed {
                ConflictAction::Resume
            } else {
                ConflictAction::Overwrite
            },
            apply_to_remaining: false,
        });
    };

    if let Some(action) = ctx.run.conflict {
        // A policy that says "resume" cannot override the verification: it is a
        // preference, and this is a proof. Falling back to overwrite restarts from
        // zero, which is always safe.
        let action = match action {
            ConflictAction::Resume if !ctx.resume_allowed => ConflictAction::Overwrite,
            other => other,
        };
        return Ok(Decision {
            action,
            apply_to_remaining: false,
        });
    }

    // The id is chosen here so the drawer can say what the job is waiting for
    // *before* an answer exists.
    let prompt_id: PromptId = Uuid::new_v4();
    ctx.run
        .report
        .send(Report::Asking {
            job: ctx.run.job,
            prompt: prompt_id,
        })
        .await;

    let source = source_facts(ctx.run, ctx.size);
    let (local, remote) = match ctx.run.direction {
        Direction::Down => (existing.clone(), source),
        Direction::Up => (source, existing.clone()),
    };
    let reply = ctx
        .interact
        .ask_with_id(
            prompt_id,
            ctx.session,
            Prompt::Conflict {
                local,
                remote,
                direction: ctx.run.direction,
                remaining: ctx.run.remaining,
                resume_allowed: ctx.resume_allowed,
            },
        )
        .await;

    match reply {
        PromptReply::Conflict {
            action,
            apply_to_remaining,
        } => Ok(Decision {
            action,
            apply_to_remaining,
        }),
        // Dismissing the sheet is a decision to leave the file alone.
        _ => Ok(Decision {
            action: ConflictAction::Skip,
            apply_to_remaining: false,
        }),
    }
}

/// What we know about the file the transfer reads from. No digest is computed here —
/// nothing in this function authorises a resume, and a `None` says so out loud.
fn source_facts(run: &RunRequest, size: Option<Bytes>) -> FileFacts {
    let path = match run.direction {
        Direction::Down => run.remote_path.clone(),
        Direction::Up => run.local_path.display().to_string(),
    };
    FileFacts {
        path,
        size: size.unwrap_or(Bytes::ZERO),
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

/// Where the partial for this transfer lives, so the resume record can claim it before
/// the backend creates it.
fn temp_path(direction: Direction, remote: &str, local: &Path, job: JobId) -> String {
    match direction {
        Direction::Down => crate::protocol::local_partial(local, job)
            .display()
            .to_string(),
        Direction::Up => {
            let (parent, name) = match remote.rsplit_once('/') {
                Some(("", name)) => ("", name),
                Some((parent, name)) => (parent, name),
                None => ("", remote),
            };
            format!("{parent}/{}", crate::protocol::partial_name(name, job))
        }
    }
}

/// A destination that is not taken, for keep-both.
///
/// The design's copy is `name (2).ext`, counting up. The extension is kept where the
/// name has one, because `report (2).pdf` opens and `report.pdf (2)` does not.
async fn free_name(
    run: &RunRequest,
    actor: &mpsc::Sender<ActorRequest>,
) -> Result<(String, PathBuf)> {
    const LIMIT: u32 = 1000;
    for n in 2..=LIMIT {
        match run.direction {
            Direction::Down => {
                let candidate = run.local_path.with_file_name(numbered(
                    &run.local_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy(),
                    n,
                ));
                // `try_exists` rather than `exists`: a path we cannot look at is not a
                // path we may write to.
                if !tokio::fs::try_exists(&candidate).await.unwrap_or(true) {
                    return Ok((run.remote_path.clone(), candidate));
                }
            }
            Direction::Up => {
                let (parent, name) = match run.remote_path.rsplit_once('/') {
                    Some((parent, name)) => (parent, name),
                    None => ("", run.remote_path.as_str()),
                };
                let candidate = format!("{parent}/{}", numbered(name, n));
                let (reply, wait) = oneshot::channel();
                actor
                    .send(ActorRequest::Exists {
                        path: candidate.clone(),
                        reply,
                    })
                    .await
                    .map_err(|_| closed())?;
                if !wait.await.unwrap_or(true) {
                    return Ok((candidate, run.local_path.clone()));
                }
            }
        }
    }
    Err(EngineError::LocalIo {
        path: run.local_path.display().to_string(),
        message: format!("every name up to ({LIMIT}) is taken"),
    })
}

/// `report.pdf` + 2 → `report (2).pdf`; `README` + 2 → `README (2)`.
fn numbered(name: &str, n: u32) -> String {
    match name.rsplit_once('.') {
        // A leading dot is the whole name of a dotfile, not an extension.
        Some((stem, ext)) if !stem.is_empty() => format!("{stem} ({n}).{ext}"),
        _ => format!("{name} ({n})"),
    }
}

/// How long to wait before attempt `n`, counting from one.
fn backoff(attempt: u32) -> Duration {
    let index = (attempt.max(1) as usize - 1).min(BACKOFF_SECS.len() - 1);
    Duration::from_secs(BACKOFF_SECS[index])
}

/// Whether a failed connection is worth trying again without asking anyone.
///
/// The excluded ones are all answers rather than accidents: a declined host key, a
/// rejected password, a protocol this build does not speak. Retrying them produces
/// the same answer ten more times, and ten more prompts along the way.
fn worth_retrying(err: &EngineError) -> bool {
    !matches!(
        err,
        EngineError::Auth { .. }
            | EngineError::TrustRejected { .. }
            | EngineError::Cancelled
            | EngineError::Unsupported { .. }
            | EngineError::PermissionDenied { .. }
    )
}

/// Answer a command that arrived while there is no connection behind it.
///
/// Every variant is answered rather than dropped: a caller holding a reply channel
/// that never answers waits for ever, which is worse than being told the session is
/// down.
fn refuse(cmd: SessionCmd) {
    fn no_connection<T>() -> Result<T> {
        Err(EngineError::network("the session is not connected"))
    }
    match cmd {
        SessionCmd::List { reply, .. } => {
            let _ = reply.send(no_connection());
        }
        SessionCmd::Stat { reply, .. } => {
            let _ = reply.send(no_connection());
        }
        SessionCmd::Mkdir { reply, .. }
        | SessionCmd::Rename { reply, .. }
        | SessionCmd::RemoveFile { reply, .. }
        | SessionCmd::RemoveDir { reply, .. }
        | SessionCmd::CancelJob { reply, .. } => {
            let _ = reply.send(no_connection());
        }
        SessionCmd::ReadFile { reply, .. } => {
            let _ = reply.send(no_connection());
        }
        // The queue is told the session is down and pauses the job itself, so a
        // dispatch that lands in the gap needs no answer of its own.
        SessionCmd::Transfer(_) | SessionCmd::Disconnect | SessionCmd::Reconnect => {}
    }
}
