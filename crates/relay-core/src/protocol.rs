//! The backend contract: one implementation per protocol family.
//!
//! **Deviation from the phase 0 sketch, deliberately.** The sketch put `download`
//! and `upload` on `Protocol` behind `&mut self`, and phase 0 §0.2 asks us to prove
//! that concurrent transfers are not forced through that single borrow. They cannot
//! be: a `&mut Protocol` held by a running transfer blocks browsing on the same
//! session, which is exactly what phase 1 §1.4 forbids.
//!
//! So transfers live on [`TransferLane`], an independently owned handle that
//! [`Protocol::open_lane`] hands out. The borrow ends when `open_lane` returns, the
//! lane moves into its own task, and the session actor keeps serving listings. For
//! SFTP a lane is a second `sftp` subsystem channel on the same SSH connection; for
//! FTP (phase 3) it is an additional control connection, which is why the per-server
//! connection cap lives on [`BackendCapabilities`].

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::interact::Interact;
use crate::model::{JobId, RemoteEntry, ServerConfig, ServerInfo};

/// Where a backend gets credentials. Implemented over the OS keychain in phase 1;
/// the trait keeps `relay-core` testable without touching a real keychain.
#[async_trait]
pub trait SecretSource: Send + Sync {
    async fn password(&self, server: &ServerConfig) -> Option<String>;
    async fn passphrase(&self, server: &ServerConfig) -> Option<String>;
}

/// Bytes-so-far callback. Owned rather than borrowed so a transfer can move into a
/// spawned task; throttling happens on the far side of this sink, not inside backends.
#[derive(Clone)]
pub struct ProgressSink(Arc<dyn Fn(u64) + Send + Sync>);

impl ProgressSink {
    pub fn new(f: impl Fn(u64) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    /// A sink that discards updates — for tests and for `Test connection`.
    pub fn noop() -> Self {
        Self::new(|_| {})
    }

    pub fn report(&self, bytes_so_far: u64) {
        (self.0)(bytes_so_far)
    }
}

impl std::fmt::Debug for ProgressSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProgressSink")
    }
}

#[derive(Debug, Clone)]
pub struct TransferReq {
    pub job: JobId,
    pub remote_path: String,
    pub local_path: PathBuf,
    /// Byte offset to start from. Non-zero **only** after phase 2 has verified the
    /// source facts and the partial file's content prefix. A backend must never
    /// infer resumability from a size match on its own.
    pub offset: u64,
    pub progress: ProgressSink,
    pub cancel: CancellationToken,
}

impl TransferReq {
    pub fn is_resume(&self) -> bool {
        self.offset > 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferOutcome {
    /// Total bytes written by this attempt, excluding any resumed prefix.
    pub bytes: u64,
    /// Final size of the destination file once finalised.
    pub final_size: u64,
}

/// What a backend can promise. The queue reads these instead of assuming SFTP
/// semantics apply to every protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendCapabilities {
    /// Reads and writes at an explicit offset — the mechanism resume needs.
    pub random_access: bool,
    /// Rename-over-existing, i.e. a destination is never left half-written.
    pub atomic_rename: bool,
    /// Server-reported modification times precise enough to compare.
    pub reliable_mtime: bool,
    /// Extra concurrent lanes allowed per session. SFTP multiplexes channels on one
    /// connection; FTP needs a whole control connection per lane.
    pub max_lanes: u8,
}

#[async_trait]
pub trait Protocol: Send {
    /// `interact` is an `Arc` and `secrets` a borrow, deliberately. Secrets are read
    /// within this call; the prompt handle is not. SFTP's host-key check happens inside
    /// russh's handshake handler, which russh moves into its own task, so the backend
    /// needs shared ownership of the thing that can ask a question.
    async fn connect(
        &mut self,
        cfg: &ServerConfig,
        secrets: &dyn SecretSource,
        interact: Arc<dyn Interact>,
    ) -> Result<ServerInfo>;

    fn capabilities(&self) -> BackendCapabilities;

    async fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>>;
    async fn stat(&mut self, path: &str) -> Result<Option<RemoteEntry>>;
    async fn mkdir(&mut self, path: &str) -> Result<()>;
    async fn rename(&mut self, from: &str, to: &str) -> Result<()>;
    async fn remove_file(&mut self, path: &str) -> Result<()>;
    async fn remove_dir(&mut self, path: &str) -> Result<()>;

    /// Bounded read for Quick Look and the built-in editor. Refuses beyond `max`
    /// instead of streaming a large file into memory.
    async fn read_file(&mut self, path: &str, max: u64) -> Result<Vec<u8>>;

    /// Keepalive; doubles as the latency probe in the session actor's select loop.
    async fn noop(&mut self) -> Result<()>;

    /// Hand out an independently owned transfer handle. See the module comment.
    async fn open_lane(&mut self) -> Result<Box<dyn TransferLane>>;

    async fn disconnect(&mut self);
}

/// One in-flight transfer's connection resources, owned by the transfer task.
#[async_trait]
pub trait TransferLane: Send {
    async fn download(&mut self, req: TransferReq) -> Result<TransferOutcome>;
    async fn upload(&mut self, req: TransferReq) -> Result<TransferOutcome>;
    /// Release the lane. Called even after a cancelled transfer, where protocol state
    /// may be uncertain and the lane must be discarded rather than reused.
    async fn close(self: Box<Self>);
}
