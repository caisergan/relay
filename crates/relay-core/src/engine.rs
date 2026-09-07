//! The engine's command surface: what the shell calls, and nothing else.
//!
//! This replaces phase 0's `DemoEngine`. The command names, arguments and return types
//! are unchanged — that was the point of proving the contract against a mock first —
//! but every one of them now reaches a real session actor over a real connection.
//!
//! The engine deliberately owns very little. Sessions own their connections and their
//! jobs; the coordinator owns the projection; the broker owns open prompts. What is
//! left here is routing: which session a command belongs to, and what the settings say
//! at the moment a transfer is created.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use uuid::Uuid;

use crate::error::{EngineError, Result};
use crate::hub::EngineHub;
use crate::job::QueueOp;
use crate::model::{
    Direction, JobId, Proto, RemoteEntry, ServerConfig, ServerId, ServerInfo, SessionId,
};
use crate::protocol::{Protocol, SecretSource};
use crate::secrets::KeyringSecrets;
use crate::servers::ServerStore;
use crate::session::{SessionContext, SessionHandle, TransferOrder};
use crate::settings::Settings;
use crate::sftp::SftpBackend;
use crate::trust::TrustStore;
use crate::wire::{Bytes, Order};

/// Space left between queue positions so phase 2 can reorder without renumbering.
const ORDER_STRIDE: i64 = 1024;
/// Cap on a `read_file`, for Quick Look and the built-in editor.
pub const READ_FILE_MAX: u64 = 512 * 1024;

/// How a session gets its backend. A trait so the whole engine can be exercised over
/// the in-memory backend, at the same seam the real one plugs into — the alternative
/// is testing everything except the part the shell actually calls.
pub trait BackendFactory: Send + Sync {
    fn build(&self, session: SessionId, cfg: &ServerConfig) -> Result<Box<dyn Protocol>>;
}

/// The production factory.
pub struct SftpFactory {
    trust: Arc<TrustStore>,
}

impl SftpFactory {
    pub fn new(trust: Arc<TrustStore>) -> Self {
        Self { trust }
    }
}

impl BackendFactory for SftpFactory {
    fn build(&self, session: SessionId, cfg: &ServerConfig) -> Result<Box<dyn Protocol>> {
        match cfg.proto {
            Proto::Sftp => Ok(Box::new(SftpBackend::new(session, Arc::clone(&self.trust)))),
            // Phase 3 adds the FTP family. Saying so beats attempting SFTP on port 21.
            other => Err(EngineError::Unsupported {
                operation: format!("{} is not available in this build", other.label()),
            }),
        }
    }
}

/// Where the engine keeps its files.
pub struct EnginePaths {
    pub servers: PathBuf,
    pub trust: PathBuf,
}

pub struct Engine {
    hub: Arc<EngineHub>,
    rt: tokio::runtime::Handle,
    factory: Arc<dyn BackendFactory>,
    secrets: Arc<dyn SecretSource>,
    servers: Arc<ServerStore>,
    sessions: Mutex<HashMap<SessionId, Arc<SessionHandle>>>,
    /// Which session a job belongs to, so `Cancel` reaches the actor that owns it.
    jobs: Mutex<HashMap<JobId, SessionId>>,
    settings: Mutex<Settings>,
    next_order: AtomicI64,
}

impl Engine {
    /// The real engine: SFTP backends, the OS keychain, files under the app data
    /// directory.
    pub fn new(hub: Arc<EngineHub>, rt: tokio::runtime::Handle, paths: EnginePaths) -> Self {
        let trust = Arc::new(TrustStore::load(paths.trust));
        Self::with_parts(
            hub,
            rt,
            Arc::new(SftpFactory::new(trust)),
            Arc::new(KeyringSecrets::new()),
            Arc::new(ServerStore::load(paths.servers)),
        )
    }

    pub fn with_parts(
        hub: Arc<EngineHub>,
        rt: tokio::runtime::Handle,
        factory: Arc<dyn BackendFactory>,
        secrets: Arc<dyn SecretSource>,
        servers: Arc<ServerStore>,
    ) -> Self {
        Self {
            hub,
            rt,
            factory,
            secrets,
            servers,
            sessions: Mutex::new(HashMap::new()),
            jobs: Mutex::new(HashMap::new()),
            settings: Mutex::new(Settings::default()),
            next_order: AtomicI64::new(ORDER_STRIDE),
        }
    }

    pub fn servers(&self) -> &Arc<ServerStore> {
        &self.servers
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().expect("settings poisoned").clone()
    }

    pub fn set_settings(&self, settings: Settings) -> Settings {
        let normalised = settings.normalised();
        *self.settings.lock().expect("settings poisoned") = normalised.clone();
        normalised
    }

    // ------------------------------------------------------------ sessions

    /// Open a session. Returns an id immediately; connecting happens in the actor and
    /// is reported as events, because a connect can be waiting on a host-key sheet.
    pub fn open_session(&self, cfg: ServerConfig) -> Result<SessionId> {
        let id = Uuid::new_v4();
        let backend = self.factory.build(id, &cfg)?;
        let handle = SessionHandle::spawn(
            backend,
            cfg,
            SessionContext {
                id,
                events: self.hub.events(),
                interact: Arc::clone(self.hub.prompts()) as Arc<_>,
                secrets: Arc::clone(&self.secrets),
                rt: self.rt.clone(),
            },
        );
        self.sessions
            .lock()
            .expect("sessions poisoned")
            .insert(id, Arc::new(handle));
        Ok(id)
    }

    /// Close a session and everything under it.
    ///
    /// The order is load-bearing. `close` cancels first, which stops transfers; then
    /// the broker denies this session's prompts, which releases anything parked on an
    /// unanswered sheet; only then is the join awaited. Denying prompts after the wait
    /// would mean waiting for a task that cannot finish.
    pub async fn close_session(&self, id: SessionId) {
        let handle = self.sessions.lock().expect("sessions poisoned").remove(&id);
        let Some(handle) = handle else { return };

        self.hub.prompts().deny_session(id);
        handle.close().await;
        self.jobs
            .lock()
            .expect("jobs poisoned")
            .retain(|_, session| *session != id);
        let _ = self
            .hub
            .events()
            .send(crate::events::EngineEvent::SessionClosed { id })
            .await;
    }

    /// Connect and immediately disconnect, for the server editor's "Test connection".
    ///
    /// Runs on a throwaway session id so its host-key prompt still reaches the UI
    /// through the ordinary path — testing a connection is exactly when a first-contact
    /// fingerprint matters most.
    pub async fn test_connection(&self, cfg: ServerConfig) -> Result<ServerInfo> {
        let id = Uuid::new_v4();
        let mut backend = self.factory.build(id, &cfg)?;
        let interact = Arc::clone(self.hub.prompts()) as Arc<_>;
        let result = backend.connect(&cfg, self.secrets.as_ref(), interact).await;
        backend.disconnect().await;
        self.hub.prompts().deny_session(id);
        result
    }

    // ------------------------------------------------------------ browsing

    pub async fn list_dir(&self, id: SessionId, path: &str) -> Result<Vec<RemoteEntry>> {
        self.session(id)?.list_dir(path).await
    }

    pub async fn stat(&self, id: SessionId, path: &str) -> Result<Option<RemoteEntry>> {
        self.session(id)?.stat(path).await
    }

    pub async fn mkdir(&self, id: SessionId, path: &str) -> Result<()> {
        self.session(id)?.mkdir(path).await
    }

    pub async fn rename(&self, id: SessionId, from: &str, to: &str) -> Result<()> {
        self.session(id)?.rename(from, to).await
    }

    pub async fn remove(&self, id: SessionId, path: &str, is_dir: bool) -> Result<()> {
        let session = self.session(id)?;
        if is_dir {
            session.remove_dir(path).await
        } else {
            session.remove_file(path).await
        }
    }

    pub async fn read_file(&self, id: SessionId, path: &str) -> Result<Vec<u8>> {
        self.session(id)?.read_file(path, READ_FILE_MAX).await
    }

    // ------------------------------------------------------------ transfers

    /// Queue one transfer. Returns as soon as the order is accepted; everything after
    /// that is events.
    pub async fn enqueue(
        &self,
        id: SessionId,
        server_id: ServerId,
        direction: Direction,
        remote_path: String,
        local_path: PathBuf,
    ) -> Result<JobId> {
        let session = self.session(id)?;
        let job = Uuid::new_v4();

        // Best effort: a size makes the progress bar determinate, and its absence is a
        // legitimate state the UI already draws. Not worth failing the transfer over.
        let size = match direction {
            Direction::Down => session
                .stat(&remote_path)
                .await
                .ok()
                .flatten()
                .map(|e| e.size),
            Direction::Up => tokio::fs::metadata(&local_path)
                .await
                .ok()
                .map(|m| Bytes(m.len())),
        };

        let order = TransferOrder {
            job,
            server_id,
            direction,
            remote_path,
            local_path,
            size,
            order: Order(self.next_order.fetch_add(ORDER_STRIDE, Ordering::SeqCst)),
            // Read now, not when the transfer starts: a person changing the default
            // mid-transfer should not retroactively change a decision already made.
            conflict: self.settings().default_conflict,
        };
        session.transfer(order).await?;
        self.jobs.lock().expect("jobs poisoned").insert(job, id);
        Ok(job)
    }

    /// Queue commands. Phase 1 owns cancellation honestly and says the rest is phase 2
    /// rather than accepting a command it cannot carry out.
    pub async fn queue_control(&self, op: QueueOp) -> Result<()> {
        match op {
            QueueOp::Cancel { job } => {
                let session = self
                    .jobs
                    .lock()
                    .expect("jobs poisoned")
                    .get(&job)
                    .copied()
                    .ok_or_else(|| EngineError::NotFound {
                        path: job.to_string(),
                    })?;
                self.session(session)?.cancel_job(job).await
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

    /// Stop every session, then the pump. Called on app exit.
    pub async fn shutdown(&self) {
        let ids: Vec<SessionId> = self
            .sessions
            .lock()
            .expect("sessions poisoned")
            .keys()
            .copied()
            .collect();
        for id in ids {
            self.close_session(id).await;
        }
        self.hub.shutdown().await;
    }

    fn session(&self, id: SessionId) -> Result<Arc<SessionHandle>> {
        self.sessions
            .lock()
            .expect("sessions poisoned")
            .get(&id)
            .cloned()
            .ok_or_else(|| EngineError::protocol(format!("no such session {id}")))
    }
}
