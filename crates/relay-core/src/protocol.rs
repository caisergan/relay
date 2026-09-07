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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::interact::Interact;
use crate::model::{FileFacts, JobId, RemoteEntry, ServerConfig, ServerInfo};

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

/// A durable point a resume may start from.
///
/// Called only after the bytes it describes are on the destination for good — flushed
/// and fsynced locally, acknowledged and re-readable remotely. `digest` is the SHA-256
/// of exactly the first `offset` bytes, which is what makes the checkpoint an identity
/// claim rather than a length.
///
/// Fire and forget, like [`ProgressSink`]. A checkpoint that does not reach the
/// database means a resume starts from an earlier point, which is the safe direction
/// to be wrong in; blocking the transfer until it commits would put an fsync in the
/// middle of the byte loop to prevent re-sending a few megabytes.
#[derive(Clone)]
pub struct CheckpointSink(Arc<dyn Fn(u64, String) + Send + Sync>);

impl CheckpointSink {
    pub fn new(f: impl Fn(u64, String) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    pub fn noop() -> Self {
        Self::new(|_, _| {})
    }

    pub fn report(&self, offset: u64, digest: String) {
        (self.0)(offset, digest)
    }
}

impl std::fmt::Debug for CheckpointSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CheckpointSink")
    }
}

#[derive(Debug, Clone)]
pub struct TransferReq {
    pub job: JobId,
    pub remote_path: String,
    pub local_path: PathBuf,
    /// Byte offset to start from. Non-zero **only** after the source facts and the
    /// partial file's content prefix have been verified. A backend must never infer
    /// resumability from a size match on its own.
    pub offset: u64,
    /// The running hash of the first `offset` bytes, from the verification that
    /// authorised this resume.
    ///
    /// Passed in rather than re-derived because the prefix was just read to check it,
    /// and reading it twice would double the cost of the one operation that is already
    /// the expensive part of resuming. `None` at offset zero, where there is nothing
    /// to carry.
    pub prefix: Option<crate::digest::Rolling>,
    pub progress: ProgressSink,
    pub checkpoint: CheckpointSink,
    pub cancel: CancellationToken,
    /// Whether the partial survives a cancellation.
    ///
    /// A pause and a cancel look identical from inside a transfer loop — the token is
    /// cancelled either way — and they mean opposite things for the bytes already
    /// written. A paused job is coming back to them; a cancelled one is not, and
    /// leaving its partial behind is litter a person has to find and delete.
    ///
    /// Set by whoever stops the transfer, read by the backend when it notices. Shared
    /// rather than passed because the decision is made after the request was built.
    pub keep_partial: Arc<AtomicBool>,
}

impl TransferReq {
    /// Whether the partial should be left where it is, now that the transfer is over.
    pub fn keeping_partial(&self) -> bool {
        self.keep_partial.load(Ordering::SeqCst)
    }
}

impl TransferReq {
    pub fn is_resume(&self) -> bool {
        self.offset > 0
    }
}

/// The partial file a job owns, on either side.
///
/// One function because three places have to agree on it exactly: the backend that
/// creates it, the backend that finalises it, and the resume record that claims
/// ownership of it. A name derived independently in any of them would produce a
/// record pointing at a file nothing wrote.
///
/// The job id is in the name because ownership has to be provable. A bare `.relaypart`
/// suffix would let a stale file from a previous run — or another job aimed at the
/// same destination — be mistaken for this job's work.
pub fn partial_name(file_name: &str, job: JobId) -> String {
    format!(".{file_name}.{job}.relaypart")
}

/// The local partial for a path.
pub fn local_partial(local: &Path, job: JobId) -> PathBuf {
    let name = local
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    local.with_file_name(partial_name(&name, job))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferOutcome {
    /// Total bytes written by this attempt, excluding any resumed prefix.
    pub bytes: u64,
    /// Final size of the destination file once finalised.
    pub final_size: u64,
}

/// Bytes that have arrived but have not been published at the destination.
///
/// The gap between transferring and finalising exists so something can happen in it.
/// A resumed transfer is a splice of two attempts, and the moment to notice that the
/// source moved underneath the second one is *before* the destination is replaced —
/// afterwards there is nothing left to protect. The caller checks, then calls
/// [`TransferLane::finalise`] or [`TransferLane::discard`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transferred {
    /// Where the bytes are: the job's own temporary file, on whichever side is the
    /// destination.
    pub temporary_path: String,
    /// Bytes written by this attempt, excluding any resumed prefix.
    pub bytes: u64,
    /// What the destination will be once published.
    pub final_size: u64,
    /// SHA-256 of the whole content, accumulated as it streamed. Free, because the
    /// bytes went past a hasher on their way to the disk.
    pub digest: String,
    /// The source as it was when the transfer finished, for comparing against the
    /// facts the resume record holds. `None` when it could not be read.
    pub source_now: Option<FileFacts>,
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
    /// Move the bytes into a temporary destination. Publishing them is a second step.
    async fn download(&mut self, req: &TransferReq) -> Result<Transferred>;
    async fn upload(&mut self, req: &TransferReq) -> Result<Transferred>;

    /// Publish what was transferred at the destination, replacing what is there.
    ///
    /// Separate from the transfer so a caller can verify in between, and so the
    /// intent can be recorded before the rename that a crash could land in the middle
    /// of. Idempotent as far as the protocol allows: a temporary file that is already
    /// gone means the rename happened.
    async fn finalise(&mut self, req: &TransferReq, done: &Transferred) -> Result<TransferOutcome>;

    /// Throw away a temporary destination this job owns, having decided not to publish
    /// it. Never touches the destination itself.
    async fn discard(&mut self, temporary_path: &str);

    /// SHA-256 of the first `len` bytes of a remote file, for verifying an upload's
    /// partial before continuing it.
    ///
    /// Reading a prefix back costs as much I/O as re-sending it would, and the queue
    /// says so out loud rather than pretending resume is free. It is still worth it on
    /// a large file, and it is the only thing that distinguishes "the same length" from
    /// "the same bytes".
    async fn prefix_digest(&mut self, path: &str, len: u64) -> Result<String>;

    /// The size of a remote path, or `None` when it is not there. Used to find a free
    /// name for keep-both, and to tell an owned partial from nothing at all.
    async fn size_of(&mut self, path: &str) -> Result<Option<u64>>;
    /// Release the lane. Called even after a cancelled transfer, where protocol state
    /// may be uncertain and the lane must be discarded rather than reused.
    async fn close(self: Box<Self>);
}
