//! A working engine backed by [`crate::mock`], for phase 0 only.
//!
//! Phase 0 exit criterion 3 asks for a fake session wired end to end *through the real
//! IPC path* — real commands, real coordinator, real event stream, real prompts — so
//! that the contract is proven before any of it depends on a network. This module is
//! that engine. Phase 1 replaces it with the session actor and `SftpBackend`; the
//! command surface above it does not change, which is the point.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::coordinator::log_line;
use crate::error::{EngineError, Result};
use crate::events::{EngineEvent, ListingSnapshot, LogKind};
use crate::hub::EngineHub;
use crate::job::{JobSnapshot, JobState, QueueOp};
use crate::mock::{MockBackend, MockFs, MockOptions, NoSecrets};
use crate::model::{Direction, JobId, RemoteEntry, ServerConfig, Session, SessionId, SessionState};
use crate::protocol::{ProgressSink, Protocol, TransferReq};
use crate::wire::{Bytes, Order, Seq};

struct DemoSession {
    backend: Arc<AsyncMutex<MockBackend>>,
    cancel: CancellationToken,
    listing_seq: AtomicU64,
}

pub struct DemoEngine {
    hub: Arc<EngineHub>,
    rt: tokio::runtime::Handle,
    fs: MockFs,
    sessions: Mutex<HashMap<SessionId, Arc<DemoSession>>>,
    jobs: Mutex<HashMap<JobId, CancellationToken>>,
    next_order: AtomicI64,
}

impl DemoEngine {
    pub fn new(hub: Arc<EngineHub>, rt: tokio::runtime::Handle) -> Self {
        Self {
            hub,
            rt,
            fs: MockFs::seeded(),
            sessions: Mutex::new(HashMap::new()),
            jobs: Mutex::new(HashMap::new()),
            next_order: AtomicI64::new(1024),
        }
    }

    pub fn fs(&self) -> MockFs {
        self.fs.clone()
    }

    /// Open a session and connect it in the background, exactly as phase 1 will:
    /// the command returns an id immediately and the outcome arrives as events.
    pub fn open_session(&self, cfg: ServerConfig) -> SessionId {
        let id = Uuid::new_v4();
        let opts = MockOptions {
            op_latency: std::time::Duration::from_millis(120),
            chunk: 64 * 1024,
            chunk_delay: std::time::Duration::from_millis(30),
            prompt_host_key: true,
            ..MockOptions::default()
        };
        let backend = Arc::new(AsyncMutex::new(MockBackend::with_options(
            self.fs.clone(),
            id,
            opts,
        )));
        let session = Arc::new(DemoSession {
            backend: Arc::clone(&backend),
            cancel: CancellationToken::new(),
            listing_seq: AtomicU64::new(0),
        });
        self.sessions
            .lock()
            .expect("demo sessions poisoned")
            .insert(id, Arc::clone(&session));

        let events = self.hub.events();
        let prompts = Arc::clone(self.hub.prompts());
        let name = cfg.name.clone();
        let server_id = cfg.id;
        let proto = cfg.proto;

        self.rt.spawn(async move {
            let _ = events
                .send(EngineEvent::SessionOpened {
                    session: Box::new(Session {
                        id,
                        server_id,
                        name,
                        proto,
                        state: SessionState::Connecting,
                        latency_ms: None,
                        remote_path: None,
                    }),
                })
                .await;
            let _ = events
                .send(EngineEvent::Log {
                    id,
                    line: log_line(
                        LogKind::Status,
                        format!("Connecting to {}…", cfg.endpoint()),
                    ),
                })
                .await;

            let connected = {
                let mut backend = backend.lock().await;
                backend.connect(&cfg, &NoSecrets, prompts.clone()).await
            };

            match connected {
                Ok(info) => {
                    let home = info.home_path.clone();
                    let _ = events
                        .send(EngineEvent::SessionState {
                            id,
                            state: SessionState::Connected {
                                info: Box::new(info),
                            },
                        })
                        .await;
                    let _ = events
                        .send(EngineEvent::Log {
                            id,
                            line: log_line(LogKind::Response, "Authenticated."),
                        })
                        .await;
                    let _ = events.send(EngineEvent::Latency { id, ms: 24 }).await;

                    let entries = {
                        let mut backend = backend.lock().await;
                        backend.list(&home).await.unwrap_or_default()
                    };
                    let _ =
                        events
                            .send(EngineEvent::Listing {
                                listing: Box::new(ListingSnapshot {
                                    session: id,
                                    path: home,
                                    request: Seq(session
                                        .listing_seq
                                        .fetch_add(1, Ordering::SeqCst)
                                        + 1),
                                    entries,
                                    at: Utc::now(),
                                }),
                            })
                            .await;
                }
                Err(err) => {
                    let _ = events
                        .send(EngineEvent::Log {
                            id,
                            line: log_line(LogKind::Error, err.to_string()),
                        })
                        .await;
                    let _ = events
                        .send(EngineEvent::SessionState {
                            id,
                            state: SessionState::Disconnected {
                                reason: err.to_string(),
                                // A declined host key is a decision, not a fault.
                                unexpected: !matches!(err, EngineError::TrustRejected { .. }),
                            },
                        })
                        .await;
                }
            }
        });

        id
    }

    pub async fn list_dir(&self, id: SessionId, path: &str) -> Result<Vec<RemoteEntry>> {
        let session = self.session(id)?;
        let request = Seq(session.listing_seq.fetch_add(1, Ordering::SeqCst) + 1);
        let entries = {
            let mut backend = session.backend.lock().await;
            backend.list(path).await?
        };
        let _ = self
            .hub
            .events()
            .send(EngineEvent::Listing {
                listing: Box::new(ListingSnapshot {
                    session: id,
                    path: path.to_string(),
                    request,
                    entries: entries.clone(),
                    at: Utc::now(),
                }),
            })
            .await;
        Ok(entries)
    }

    /// Queue a transfer. Returns immediately; the job's life is told in events.
    pub async fn enqueue(
        &self,
        id: SessionId,
        direction: Direction,
        remote_path: String,
        local_path: PathBuf,
    ) -> Result<JobId> {
        let session = self.session(id)?;
        let job_id = Uuid::new_v4();
        let order = Order(self.next_order.fetch_add(1024, Ordering::SeqCst));

        let size = {
            let mut backend = session.backend.lock().await;
            match direction {
                Direction::Down => backend.stat(&remote_path).await?.map(|e| e.size),
                Direction::Up => std::fs::metadata(&local_path).ok().map(|m| Bytes(m.len())),
            }
        };

        let snapshot = JobSnapshot {
            id: job_id,
            session: id,
            server_id: Uuid::nil(),
            direction,
            remote_path: remote_path.clone(),
            local_path: local_path.clone(),
            size,
            transferred: Bytes::ZERO,
            state: JobState::Queued,
            order,
            speed_bps: None,
            eta_secs: None,
            conflict_policy: None,
            parent: None,
            started_at: None,
        };
        let events = self.hub.events();
        let _ = events
            .send(EngineEvent::JobUpdate {
                job: Box::new(snapshot.clone()),
            })
            .await;

        // The lane is taken under the lock and then owned by the transfer task, so
        // browsing carries on while the bytes move.
        let mut lane = {
            let mut backend = session.backend.lock().await;
            backend.open_lane().await?
        };
        let cancel = session.cancel.child_token();
        self.jobs
            .lock()
            .expect("demo jobs poisoned")
            .insert(job_id, cancel.clone());

        self.rt.spawn(async move {
            let mut running = JobSnapshot {
                state: JobState::Transferring,
                started_at: Some(Utc::now()),
                ..snapshot
            };
            let _ = events
                .send(EngineEvent::JobUpdate {
                    job: Box::new(running.clone()),
                })
                .await;

            let progress = {
                let events = events.clone();
                let template = running.clone();
                ProgressSink::new(move |bytes| {
                    let job = JobSnapshot {
                        transferred: Bytes(bytes),
                        ..template.clone()
                    };
                    // Dropping a progress tick under backpressure is safe: the pump
                    // coalesces them anyway and the terminal update is sent with `send`.
                    let _ = events.try_send(EngineEvent::JobUpdate { job: Box::new(job) });
                })
            };

            let req = TransferReq {
                job: job_id,
                remote_path,
                local_path,
                offset: 0,
                progress,
                cancel,
            };
            let outcome = match direction {
                Direction::Down => lane.download(req).await,
                Direction::Up => lane.upload(req).await,
            };
            lane.close().await;

            running.state = match outcome {
                Ok(ref out) => {
                    running.transferred = Bytes(out.final_size);
                    running.size = Some(Bytes(out.final_size));
                    JobState::Done { at: Utc::now() }
                }
                Err(EngineError::Cancelled) => JobState::Cancelled,
                Err(error) => JobState::Failed { error, attempts: 1 },
            };
            let _ = events
                .send(EngineEvent::JobUpdate {
                    job: Box::new(running),
                })
                .await;
        });

        Ok(job_id)
    }

    /// Phase 0 honours cancellation, because the demo owns a real token per job.
    /// Everything else is the phase 2 scheduler's job and says so.
    pub fn queue_control(&self, op: QueueOp) -> Result<()> {
        match op {
            QueueOp::Cancel { job } => {
                let token = self.jobs.lock().expect("demo jobs poisoned").remove(&job);
                match token {
                    Some(token) => {
                        token.cancel();
                        Ok(())
                    }
                    None => Err(EngineError::NotFound {
                        path: job.to_string(),
                    }),
                }
            }
            QueueOp::Retry { .. }
            | QueueOp::Pause { .. }
            | QueueOp::Resume { .. }
            | QueueOp::PauseAll
            | QueueOp::ResumeAll
            | QueueOp::Reorder { .. }
            | QueueOp::ClearCompleted => Err(EngineError::Unsupported {
                operation: "queue scheduling arrives with the phase 2 queue".into(),
            }),
        }
    }

    pub async fn close_session(&self, id: SessionId) {
        let session = self
            .sessions
            .lock()
            .expect("demo sessions poisoned")
            .remove(&id);
        let Some(session) = session else { return };

        // Order matters: stop the work, release anyone waiting on an answer, then
        // tell the UI. A prompt left pending here would hold a task forever.
        session.cancel.cancel();
        self.hub.prompts().deny_session(id);
        session.backend.lock().await.disconnect().await;
        let _ = self
            .hub
            .events()
            .send(EngineEvent::SessionClosed { id })
            .await;
    }

    fn session(&self, id: SessionId) -> Result<Arc<DemoSession>> {
        self.sessions
            .lock()
            .expect("demo sessions poisoned")
            .get(&id)
            .cloned()
            .ok_or_else(|| EngineError::protocol(format!("no such session {id}")))
    }
}
