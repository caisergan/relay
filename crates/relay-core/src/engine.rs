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
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uuid::Uuid;

use crate::error::{EngineError, Result};
use crate::hub::EngineHub;
use crate::job::{JobKind, QueueOp};
use crate::model::{
    Direction, JobId, Proto, RemoteEntry, ServerConfig, ServerId, ServerInfo, SessionId,
};
use crate::protocol::{Protocol, SecretSource};
use crate::queue::{BatchId, JobSpec};
use crate::scheduler::{Dispatcher, RunRequest, Scheduler, SchedulerContext};
use crate::secrets::{Credentials, KeyringSecrets, Overlay};
use crate::servers::ServerStore;
use crate::session::{SessionContext, SessionHandle};
use crate::settings::Settings;
use crate::sftp::SftpBackend;
use crate::store::QueueStore;
use crate::trust::TrustStore;

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
/// One thing a person asked to move. Part of the IPC surface: the interface builds
/// these from a drag or a click and sends the whole gesture at once.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TransferItem {
    pub session: SessionId,
    pub server_id: ServerId,
    pub direction: Direction,
    pub remote_path: String,
    pub local_path: PathBuf,
    /// A directory, which becomes a folder job the walker fills in.
    pub is_dir: bool,
}

pub struct EnginePaths {
    pub servers: PathBuf,
    pub trust: PathBuf,
    pub queue: PathBuf,
}

/// The sessions, shared by the engine that opens them and the scheduler that
/// dispatches to them.
///
/// A shared map rather than the scheduler holding the engine: the scheduler is built
/// first, because a session has to be able to report itself ready the moment it
/// connects. Handing it this instead of an `Engine` keeps that ordering possible and
/// stops the two from owning each other.
#[derive(Clone, Default)]
pub struct SessionRegistry(Arc<Mutex<HashMap<SessionId, Arc<SessionHandle>>>>);

impl SessionRegistry {
    fn get(&self, id: SessionId) -> Option<Arc<SessionHandle>> {
        self.0.lock().expect("sessions poisoned").get(&id).cloned()
    }

    fn insert(&self, id: SessionId, handle: Arc<SessionHandle>) {
        self.0.lock().expect("sessions poisoned").insert(id, handle);
    }

    fn remove(&self, id: SessionId) -> Option<Arc<SessionHandle>> {
        self.0.lock().expect("sessions poisoned").remove(&id)
    }

    fn ids(&self) -> Vec<SessionId> {
        self.0
            .lock()
            .expect("sessions poisoned")
            .keys()
            .copied()
            .collect()
    }
}

#[async_trait]
impl Dispatcher for SessionRegistry {
    async fn dispatch(&self, run: RunRequest) -> Result<()> {
        let session = run.session;
        self.get(session)
            .ok_or_else(|| EngineError::protocol(format!("no such session {session}")))?
            .transfer(run)
            .await
    }

    async fn abort(&self, session: SessionId, job: JobId) {
        if let Some(handle) = self.get(session) {
            let _ = handle.cancel_job(job).await;
        }
    }
}

pub struct Engine {
    hub: Arc<EngineHub>,
    rt: tokio::runtime::Handle,
    factory: Arc<dyn BackendFactory>,
    secrets: Arc<dyn SecretSource>,
    servers: Arc<ServerStore>,
    sessions: SessionRegistry,
    queue: Scheduler,
    settings: Mutex<Settings>,
}

impl Engine {
    /// The real engine: SFTP backends, the OS keychain, files under the app data
    /// directory.
    pub async fn new(
        hub: Arc<EngineHub>,
        rt: tokio::runtime::Handle,
        paths: EnginePaths,
    ) -> Result<Self> {
        let trust = Arc::new(TrustStore::load(paths.trust));
        Self::with_parts(
            hub,
            rt,
            Arc::new(SftpFactory::new(trust)),
            Arc::new(KeyringSecrets::new()),
            Arc::new(ServerStore::load(paths.servers)),
            QueueStore::open(paths.queue).await?,
        )
        .await
    }

    pub async fn with_parts(
        hub: Arc<EngineHub>,
        rt: tokio::runtime::Handle,
        factory: Arc<dyn BackendFactory>,
        secrets: Arc<dyn SecretSource>,
        servers: Arc<ServerStore>,
        store: QueueStore,
    ) -> Result<Self> {
        let settings = Settings::default();
        let sessions = SessionRegistry::default();
        // The scheduler starts before any session, because a session announces itself
        // ready as soon as it connects and has to have somewhere to announce it to.
        let queue = Scheduler::spawn(SchedulerContext {
            store,
            events: hub.events(),
            dispatcher: Arc::new(sessions.clone()) as Arc<dyn Dispatcher>,
            rt: rt.clone(),
            concurrency: settings.concurrency,
        })
        .await?;
        Ok(Self {
            hub,
            rt,
            factory,
            secrets,
            servers,
            sessions,
            queue,
            settings: Mutex::new(settings),
        })
    }

    /// The queue, for the snapshot the shell serves to a remounting UI.
    pub fn queue(&self) -> &Scheduler {
        &self.queue
    }

    pub fn servers(&self) -> &Arc<ServerStore> {
        &self.servers
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().expect("settings poisoned").clone()
    }

    pub async fn set_settings(&self, settings: Settings) -> Settings {
        let normalised = settings.normalised();
        *self.settings.lock().expect("settings poisoned") = normalised.clone();
        // The slider is only a setting if it reaches work already running.
        self.queue.set_concurrency(normalised.concurrency).await;
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
                queue: self.queue.clone(),
            },
        );
        self.sessions.insert(id, Arc::new(handle));
        Ok(id)
    }

    /// Close a session and everything under it.
    ///
    /// The order is load-bearing. `close` cancels first, which stops transfers; then
    /// the broker denies this session's prompts, which releases anything parked on an
    /// unanswered sheet; only then is the join awaited. Denying prompts after the wait
    /// would mean waiting for a task that cannot finish.
    pub async fn close_session(&self, id: SessionId) {
        let Some(handle) = self.sessions.remove(id) else {
            return;
        };

        self.hub.prompts().deny_session(id);
        handle.close().await;
        // The queue keeps this session's jobs, paused. Closing a tab is not a decision
        // to abandon its transfers, and reopening the server picks them back up.
        self.queue.session_closed(id).await;
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
    ///
    /// `draft` carries credentials typed into the form but not yet saved, so testing a
    /// password does not require committing it to the keychain first. They live for the
    /// duration of this call and are never written anywhere.
    pub async fn test_connection(
        &self,
        cfg: ServerConfig,
        draft: Credentials,
    ) -> Result<ServerInfo> {
        let id = Uuid::new_v4();
        let mut backend = self.factory.build(id, &cfg)?;
        let interact = Arc::clone(self.hub.prompts()) as Arc<_>;
        let secrets = Overlay::new(draft, Arc::clone(&self.secrets));
        let result = backend.connect(&cfg, &secrets, interact).await;
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

    /// Queue one gesture's worth of transfers.
    ///
    /// `batch` identifies the gesture, not the engine's own bookkeeping: it comes from
    /// the caller so that a command re-sent after an uncertain delivery is recognised
    /// as the same request and returns the jobs it already made. A user who clicks
    /// Download once must not watch the folder arrive twice.
    ///
    /// The conflict default is read now rather than when a transfer starts. Someone
    /// changing the setting mid-queue should not retroactively change a decision that
    /// was already made on their behalf.
    pub async fn enqueue(&self, batch: BatchId, items: Vec<TransferItem>) -> Result<Vec<JobId>> {
        // Checked before anything is queued. A job aimed at a session that does not
        // exist could never run, and leaving it in the queue would show the user a row
        // that is waiting for nothing.
        for item in &items {
            self.session(item.session)?;
        }
        let specs: Vec<JobSpec> = items
            .iter()
            .map(|item| JobSpec {
                session: item.session,
                server_id: item.server_id,
                kind: if item.is_dir {
                    JobKind::Folder
                } else {
                    JobKind::File
                },
                direction: item.direction,
                // Stable within the batch, and the natural name for the thing being
                // moved: asking for the same file in the same gesture is one job.
                item: format!("{:?}:{}", item.direction, item.remote_path),
                remote_path: item.remote_path.clone(),
                local_path: item.local_path.clone(),
                size: None,
                parent: None,
            })
            .collect();

        let ids = self.queue.enqueue(batch, specs).await?;

        // A folder is queued before it is walked, so the drawer shows a row
        // immediately and the walk has a parent to hang its children on.
        for (item, id) in items.iter().zip(&ids) {
            if !item.is_dir {
                continue;
            }
            let Ok(session) = self.session(item.session) else {
                continue;
            };
            let cancel = session.child_token();
            self.rt.spawn(crate::walk::run(crate::walk::Walk {
                parent: *id,
                batch,
                session,
                session_id: item.session,
                server_id: item.server_id,
                queue: self.queue.clone(),
                direction: item.direction,
                remote_root: item.remote_path.clone(),
                local_root: item.local_path.clone(),
                cancel,
            }));
        }
        Ok(ids)
    }

    /// Pause, resume, retry, cancel, reorder, clear. Every one of them is the
    /// scheduler's to answer now; the engine only carries the message.
    pub async fn queue_control(&self, op: QueueOp) -> Result<()> {
        self.queue.control(op).await
    }

    /// Stop every session, then the pump. Called on app exit.
    pub async fn shutdown(&self) {
        for id in self.sessions.ids() {
            self.close_session(id).await;
        }
        // After the sessions, so anything they reported on the way out is applied and
        // any unflushed progress reaches the disk.
        self.queue.shutdown().await;
        self.hub.shutdown().await;
    }

    fn session(&self, id: SessionId) -> Result<Arc<SessionHandle>> {
        self.sessions
            .get(id)
            .ok_or_else(|| EngineError::protocol(format!("no such session {id}")))
    }
}
