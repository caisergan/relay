//! An in-memory backend. Not a toy: it is the fixture the frontend develops against,
//! the subject of the phase 0 round-trip tests, and the thing that proves the IPC
//! contract end to end before a single packet touches the network.
//!
//! It deliberately reproduces the parts of real-backend behaviour that shape the
//! interface: transfers run on independently owned lanes, downloads land through an
//! owned temporary file and a rename, cancellation is checked mid-transfer, and a
//! resume offset is refused unless the partial file actually matches it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::error::{EngineError, Result};
use crate::interact::{Interact, Prompt, PromptReply};
use crate::model::{FileFacts, FileKind, RemoteEntry, ServerConfig, ServerInfo, SessionId};
use crate::protocol::{
    BackendCapabilities, Protocol, SecretSource, TransferLane, TransferOutcome, TransferReq,
    Transferred,
};
use crate::wire::Bytes;

#[derive(Debug, Clone)]
struct FileNode {
    data: Vec<u8>,
    modified: DateTime<Utc>,
    mode: u32,
}

#[derive(Debug, Clone)]
enum Node {
    Dir(BTreeMap<String, Node>),
    File(FileNode),
}

/// A shareable in-memory filesystem. Cloning shares the tree, which is how two lanes
/// transfer concurrently against the same "server".
#[derive(Debug, Clone)]
pub struct MockFs(Arc<Mutex<BTreeMap<String, Node>>>);

impl Default for MockFs {
    fn default() -> Self {
        Self::empty()
    }
}

impl MockFs {
    pub fn empty() -> Self {
        Self(Arc::new(Mutex::new(BTreeMap::new())))
    }

    /// A small tree with the shapes the panes need to render: nested directories,
    /// a dotfile, a large file, an empty file.
    pub fn seeded() -> Self {
        let fs = Self::empty();
        fs.write_file(
            "/var/www/index.html",
            b"<!doctype html><title>relay</title>".to_vec(),
        );
        fs.write_file("/var/www/assets/app.js", vec![b'x'; 256 * 1024]);
        fs.write_file("/var/www/assets/logo.svg", b"<svg/>".to_vec());
        fs.write_file("/var/log/nginx/access.log", vec![b'.'; 3 * 1024 * 1024]);
        fs.write_file("/home/deploy/.bashrc", b"export PATH=$PATH\n".to_vec());
        fs.write_file("/home/deploy/notes.md", b"# notes\n".to_vec());
        fs.write_file("/home/deploy/empty", Vec::new());
        fs.mkdir_all("/home/deploy/releases");
        fs
    }

    pub fn write_file(&self, path: &str, data: Vec<u8>) {
        let (parent, name) = split_parent(path);
        self.mkdir_all(&parent);
        let mut root = self.lock();
        let dir = dir_mut(&mut root, &parent).expect("mkdir_all just created it");
        dir.insert(
            name,
            Node::File(FileNode {
                data,
                modified: Utc::now(),
                mode: 0o644,
            }),
        );
    }

    pub fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        let root = self.lock();
        match node(&root, path) {
            Some(Node::File(f)) => Some(f.data.clone()),
            _ => None,
        }
    }

    pub fn mkdir_all(&self, path: &str) {
        let mut root = self.lock();
        let mut cursor = &mut *root;
        for segment in segments(path) {
            let entry = cursor
                .entry(segment.to_string())
                .or_insert_with(|| Node::Dir(BTreeMap::new()));
            match entry {
                Node::Dir(children) => cursor = children,
                Node::File(_) => return,
            }
        }
    }

    /// Delete a file if it is there. For the lane's own temporaries, where a missing
    /// one means the work was already done.
    pub fn remove_file(&self, path: &str) {
        let (parent, name) = split_parent(path);
        let mut root = self.lock();
        if let Some(dir) = dir_mut(&mut root, &parent) {
            dir.remove(&name);
        }
    }

    pub fn exists(&self, path: &str) -> bool {
        is_root(path) || node(&self.lock(), path).is_some()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Node>> {
        self.0.lock().expect("mock filesystem poisoned")
    }

    fn list(&self, path: &str) -> Result<Vec<RemoteEntry>> {
        let root = self.lock();
        if is_root(path) {
            return Ok(root
                .iter()
                .map(|(name, node)| entry_for(name, node))
                .collect());
        }
        let dir = match node(&root, path) {
            Some(Node::Dir(children)) => children,
            Some(Node::File(_)) => {
                return Err(EngineError::protocol(format!("{path} is not a directory")));
            }
            None => {
                return Err(EngineError::NotFound {
                    path: path.to_string(),
                });
            }
        };
        Ok(dir
            .iter()
            .map(|(name, node)| entry_for(name, node))
            .collect())
    }

    fn stat(&self, path: &str) -> Option<RemoteEntry> {
        let root = self.lock();
        if is_root(path) {
            return Some(entry_for("/", &Node::Dir(BTreeMap::new())));
        }
        let (_, name) = split_parent(path);
        node(&root, path).map(|n| entry_for(&name, n))
    }
}

fn entry_for(name: &str, node: &Node) -> RemoteEntry {
    match node {
        Node::Dir(_) => RemoteEntry {
            name: name.to_string(),
            kind: FileKind::Dir,
            target_kind: None,
            size: Bytes::ZERO,
            modified: None,
            perms: Some("rwxr-xr-x".into()),
            mode: Some(0o755),
            owner: Some("deploy".into()),
            group: Some("deploy".into()),
        },
        Node::File(f) => RemoteEntry {
            name: name.to_string(),
            kind: FileKind::File,
            target_kind: None,
            size: Bytes(f.data.len() as u64),
            modified: Some(f.modified),
            perms: Some("rw-r--r--".into()),
            mode: Some(f.mode),
            owner: Some("deploy".into()),
            group: Some("deploy".into()),
        },
    }
}

fn segments(path: &str) -> impl Iterator<Item = &str> {
    path.split('/').filter(|s| !s.is_empty() && *s != ".")
}

fn split_parent(path: &str) -> (String, String) {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => ("/".to_string(), trimmed.trim_start_matches('/').to_string()),
        Some(idx) => (trimmed[..idx].to_string(), trimmed[idx + 1..].to_string()),
    }
}

/// Resolve a path to its node. Returns `None` for the root, which has no owning
/// node — callers that accept the root handle it explicitly.
fn node<'a>(root: &'a BTreeMap<String, Node>, path: &str) -> Option<&'a Node> {
    let mut cursor = root;
    let mut found: Option<&Node> = None;
    for segment in segments(path) {
        // A previous segment resolved to a file, so this path cannot exist.
        if matches!(found, Some(Node::File(_))) {
            return None;
        }
        let next = cursor.get(segment)?;
        if let Node::Dir(children) = next {
            cursor = children;
        }
        found = Some(next);
    }
    found
}

fn is_root(path: &str) -> bool {
    segments(path).next().is_none()
}

fn dir_mut<'a>(
    root: &'a mut BTreeMap<String, Node>,
    path: &str,
) -> Option<&'a mut BTreeMap<String, Node>> {
    let mut cursor = root;
    for segment in segments(path) {
        match cursor.get_mut(segment)? {
            Node::Dir(children) => cursor = children,
            Node::File(_) => return None,
        }
    }
    Some(cursor)
}

/// Injectable behaviour, so tests and the UI fixture can ask for slowness, latency,
/// or a specific failure without a real server.
#[derive(Debug, Clone)]
pub struct MockOptions {
    pub op_latency: Duration,
    pub chunk: usize,
    pub chunk_delay: Duration,
    pub max_lanes: u8,
    /// Ask for host-key trust on connect, exercising the prompt path.
    pub prompt_host_key: bool,
    /// Fail the next transfer with this error.
    pub fail_transfer: Option<EngineError>,
    /// A switch a test can throw to make the connection go away.
    ///
    /// Shared rather than a count, because "the network dropped" is something that
    /// happens *to* a session at a moment of the test's choosing, and a counter would
    /// make the test depend on how many operations the engine happens to issue.
    pub severed: Arc<AtomicBool>,
}

impl Default for MockOptions {
    fn default() -> Self {
        Self {
            op_latency: Duration::ZERO,
            chunk: 64 * 1024,
            chunk_delay: Duration::ZERO,
            max_lanes: 4,
            prompt_host_key: false,
            fail_transfer: None,
            severed: Arc::new(AtomicBool::new(false)),
        }
    }
}

pub struct MockBackend {
    fs: MockFs,
    opts: MockOptions,
    session: SessionId,
    connected: bool,
}

impl MockBackend {
    pub fn new(fs: MockFs, session: SessionId) -> Self {
        Self {
            fs,
            opts: MockOptions::default(),
            session,
            connected: false,
        }
    }

    pub fn with_options(fs: MockFs, session: SessionId, opts: MockOptions) -> Self {
        Self {
            fs,
            opts,
            session,
            connected: false,
        }
    }

    pub fn fs(&self) -> MockFs {
        self.fs.clone()
    }

    async fn latency(&self) {
        if !self.opts.op_latency.is_zero() {
            tokio::time::sleep(self.opts.op_latency).await;
        }
    }
}

impl MockBackend {
    /// `Err` once a test has thrown the switch, as a real dropped connection reports.
    fn severed(&self) -> Result<()> {
        if self.opts.severed.load(Ordering::SeqCst) {
            return Err(EngineError::network("the connection was severed"));
        }
        Ok(())
    }
}

#[async_trait]
impl Protocol for MockBackend {
    async fn connect(
        &mut self,
        cfg: &ServerConfig,
        secrets: &dyn SecretSource,
        interact: std::sync::Arc<dyn Interact>,
    ) -> Result<ServerInfo> {
        self.latency().await;

        if self.opts.prompt_host_key {
            let reply = interact
                .ask(
                    self.session,
                    Prompt::HostKey {
                        host: cfg.endpoint(),
                        algo: "ssh-ed25519".into(),
                        sha256: "SHA256:mockmockmockmockmockmockmockmockmockmockmoc".into(),
                        changed: false,
                    },
                )
                .await;
            if !matches!(reply, PromptReply::Accept { .. }) {
                return Err(EngineError::TrustRejected {
                    endpoint: cfg.endpoint(),
                });
            }
        }

        // Mirrors the real ladder: stored secret first, prompt only if absent.
        if matches!(cfg.auth, crate::model::AuthMethod::Password)
            && secrets.password(cfg).await.is_none()
        {
            let reply = interact
                .ask(
                    self.session,
                    Prompt::Password {
                        hint: cfg.username.clone(),
                    },
                )
                .await;
            if !matches!(reply, PromptReply::Password { .. }) {
                return Err(EngineError::Auth {
                    message: "no password supplied".into(),
                });
            }
        }

        self.connected = true;
        Ok(ServerInfo {
            banner: Some("mock server".into()),
            software: Some("SSH-2.0-MockSSH_1.0".into()),
            kex: Some("curve25519-sha256".into()),
            cipher: Some("chacha20-poly1305@openssh.com".into()),
            mac: Some("hmac-sha2-256-etm@openssh.com".into()),
            host_key_algo: Some("ssh-ed25519".into()),
            host_key_sha256: Some("SHA256:mockmockmockmockmockmockmockmockmockmockmoc".into()),
            home_path: cfg
                .initial_remote_path
                .clone()
                .unwrap_or_else(|| "/home/deploy".into()),
        })
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            random_access: true,
            atomic_rename: true,
            reliable_mtime: true,
            max_lanes: self.opts.max_lanes,
        }
    }

    async fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>> {
        self.latency().await;
        self.severed()?;
        self.fs.list(path)
    }

    async fn stat(&mut self, path: &str) -> Result<Option<RemoteEntry>> {
        self.latency().await;
        Ok(self.fs.stat(path))
    }

    async fn mkdir(&mut self, path: &str) -> Result<()> {
        self.latency().await;
        if self.fs.exists(path) {
            return Err(EngineError::protocol(format!("{path} already exists")));
        }
        self.fs.mkdir_all(path);
        Ok(())
    }

    async fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        self.latency().await;
        let mut root = self.fs.lock();
        let (from_parent, from_name) = split_parent(from);
        let taken = dir_mut(&mut root, &from_parent)
            .and_then(|dir| dir.remove(&from_name))
            .ok_or_else(|| EngineError::NotFound {
                path: from.to_string(),
            })?;
        let (to_parent, to_name) = split_parent(to);
        match dir_mut(&mut root, &to_parent) {
            Some(dir) => {
                dir.insert(to_name, taken);
                Ok(())
            }
            None => {
                // Put it back rather than losing the node.
                if let Some(dir) = dir_mut(&mut root, &from_parent) {
                    dir.insert(from_name, taken);
                }
                Err(EngineError::NotFound { path: to_parent })
            }
        }
    }

    async fn remove_file(&mut self, path: &str) -> Result<()> {
        self.latency().await;
        let mut root = self.fs.lock();
        let (parent, name) = split_parent(path);
        let dir = dir_mut(&mut root, &parent).ok_or_else(|| EngineError::NotFound {
            path: path.to_string(),
        })?;
        match dir.get(&name) {
            Some(Node::File(_)) => {
                dir.remove(&name);
                Ok(())
            }
            Some(Node::Dir(_)) => Err(EngineError::protocol(format!("{path} is a directory"))),
            None => Err(EngineError::NotFound {
                path: path.to_string(),
            }),
        }
    }

    async fn remove_dir(&mut self, path: &str) -> Result<()> {
        self.latency().await;
        let mut root = self.fs.lock();
        let (parent, name) = split_parent(path);
        let dir = dir_mut(&mut root, &parent).ok_or_else(|| EngineError::NotFound {
            path: path.to_string(),
        })?;
        match dir.get(&name) {
            Some(Node::Dir(children)) if !children.is_empty() => {
                Err(EngineError::protocol(format!("{path} is not empty")))
            }
            Some(Node::Dir(_)) => {
                dir.remove(&name);
                Ok(())
            }
            Some(Node::File(_)) => Err(EngineError::protocol(format!("{path} is not a directory"))),
            None => Err(EngineError::NotFound {
                path: path.to_string(),
            }),
        }
    }

    async fn read_file(&mut self, path: &str, max: u64) -> Result<Vec<u8>> {
        self.latency().await;
        let data = self
            .fs
            .read_file(path)
            .ok_or_else(|| EngineError::NotFound {
                path: path.to_string(),
            })?;
        if data.len() as u64 > max {
            return Err(EngineError::protocol("too large"));
        }
        Ok(data)
    }

    async fn noop(&mut self) -> Result<()> {
        self.latency().await;
        // The keepalive is the liveness probe, so this is where a severed connection
        // is noticed even when nobody is browsing.
        self.severed()
    }

    async fn open_lane(&mut self) -> Result<Box<dyn TransferLane>> {
        // The lane borrows nothing from the backend: that is the whole point.
        Ok(Box::new(MockLane {
            fs: self.fs.clone(),
            opts: self.opts.clone(),
        }))
    }

    async fn disconnect(&mut self) {
        self.connected = false;
    }
}

pub struct MockLane {
    fs: MockFs,
    opts: MockOptions,
}

impl MockLane {
    /// A lane over an in-memory filesystem with no session behind it, for testing the
    /// parts of the engine that only need something implementing the trait.
    pub fn over(fs: MockFs) -> Self {
        Self {
            fs,
            opts: MockOptions::default(),
        }
    }

    fn partial_path(local: &Path, job: uuid::Uuid) -> PathBuf {
        crate::protocol::local_partial(local, job)
    }
}

/// A local file's facts, for the resume check to compare against.
async fn local_facts(path: &Path) -> Option<FileFacts> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    Some(FileFacts {
        path: path.display().to_string(),
        size: Bytes(meta.len()),
        modified: meta.modified().ok().map(chrono::DateTime::<Utc>::from),
        digest: None,
    })
}

/// Stop a download: keep the partial for a pause, remove it for a cancel.
async fn stop(mut file: tokio::fs::File, partial: &Path, keep: bool) -> Result<Transferred> {
    if keep {
        let _ = file.flush().await;
        let _ = file.sync_all().await;
        drop(file);
    } else {
        drop(file);
        // Only ever the partial this job owns.
        let _ = tokio::fs::remove_file(partial).await;
    }
    Err(EngineError::Cancelled)
}

#[async_trait]
impl TransferLane for MockLane {
    async fn download(&mut self, req: &TransferReq) -> Result<Transferred> {
        if let Some(err) = self.opts.fail_transfer.clone() {
            return Err(err);
        }
        let data = self
            .fs
            .read_file(&req.remote_path)
            .ok_or_else(|| EngineError::NotFound {
                path: req.remote_path.clone(),
            })?;

        if req.offset > data.len() as u64 {
            return Err(EngineError::ResumeUnverifiable {
                reason: "recorded offset is past the end of the source".into(),
            });
        }

        let partial = Self::partial_path(&req.local_path, req.job);
        let mut file = if req.offset > 0 {
            let f = tokio::fs::OpenOptions::new()
                .write(true)
                .open(&partial)
                .await
                .map_err(|e| EngineError::from_io(&partial, &e))?;
            let len = f
                .metadata()
                .await
                .map_err(|e| EngineError::from_io(&partial, &e))?
                .len();
            // A resume offset is a claim about *this* partial file. Verify it.
            if len < req.offset {
                return Err(EngineError::ResumeUnverifiable {
                    reason: format!(
                        "partial file holds {len} bytes, offset claims {}",
                        req.offset
                    ),
                });
            }
            // Bytes past the checkpoint were never covered by the digest that
            // authorised this resume.
            f.set_len(req.offset)
                .await
                .map_err(|e| EngineError::from_io(&partial, &e))?;
            let mut f = f;
            f.seek(std::io::SeekFrom::Start(req.offset))
                .await
                .map_err(|e| EngineError::from_io(&partial, &e))?;
            f
        } else {
            tokio::fs::File::create(&partial)
                .await
                .map_err(|e| EngineError::from_io(&partial, &e))?
        };

        let mut written = 0u64;
        let mut rolling = req.prefix.clone().unwrap_or_default();
        for chunk in data[req.offset as usize..].chunks(self.opts.chunk.max(1)) {
            if req.cancel.is_cancelled() {
                return stop(file, &partial, req.keeping_partial()).await;
            }
            file.write_all(chunk)
                .await
                .map_err(|e| EngineError::from_io(&partial, &e))?;
            rolling.update(chunk);
            written += chunk.len() as u64;
            req.progress.report(req.offset + written);
            // Every chunk, unlike the real backend's eight megabytes: a test needs a
            // checkpoint it can reach without moving eight megabytes to get there.
            file.flush()
                .await
                .map_err(|e| EngineError::from_io(&partial, &e))?;
            file.sync_all()
                .await
                .map_err(|e| EngineError::from_io(&partial, &e))?;
            req.checkpoint
                .report(req.offset + written, rolling.snapshot());
            if !self.opts.chunk_delay.is_zero() {
                tokio::select! {
                    _ = tokio::time::sleep(self.opts.chunk_delay) => {}
                    _ = req.cancel.cancelled() => {
                        return stop(file, &partial, req.keeping_partial()).await;
                    }
                }
            }
        }

        file.flush()
            .await
            .map_err(|e| EngineError::from_io(&partial, &e))?;
        file.sync_all()
            .await
            .map_err(|e| EngineError::from_io(&partial, &e))?;
        drop(file);

        Ok(Transferred {
            temporary_path: partial.display().to_string(),
            bytes: written,
            final_size: data.len() as u64,
            digest: rolling.snapshot(),
            source_now: self.fs.stat(&req.remote_path).map(|entry| FileFacts {
                path: req.remote_path.clone(),
                size: entry.size,
                modified: entry.modified,
                digest: None,
            }),
        })
    }

    async fn upload(&mut self, req: &TransferReq) -> Result<Transferred> {
        if let Some(err) = self.opts.fail_transfer.clone() {
            return Err(err);
        }
        let mut file = tokio::fs::File::open(&req.local_path)
            .await
            .map_err(|e| EngineError::from_io(&req.local_path, &e))?;
        if req.offset > 0 {
            file.seek(std::io::SeekFrom::Start(req.offset))
                .await
                .map_err(|e| EngineError::from_io(&req.local_path, &e))?;
        }

        let mut existing = if req.offset > 0 {
            let mut data = self.fs.read_file(&req.remote_path).unwrap_or_default();
            if data.len() as u64 != req.offset {
                return Err(EngineError::ResumeUnverifiable {
                    reason: format!(
                        "remote holds {} bytes, offset claims {}",
                        data.len(),
                        req.offset
                    ),
                });
            }
            data.truncate(req.offset as usize);
            data
        } else {
            Vec::new()
        };

        let mut buf = vec![0u8; self.opts.chunk.max(1)];
        let mut written = 0u64;
        loop {
            if req.cancel.is_cancelled() {
                return Err(EngineError::Cancelled);
            }
            let read = file
                .read(&mut buf)
                .await
                .map_err(|e| EngineError::from_io(&req.local_path, &e))?;
            if read == 0 {
                break;
            }
            existing.extend_from_slice(&buf[..read]);
            written += read as u64;
            req.progress.report(req.offset + written);
            if !self.opts.chunk_delay.is_zero() {
                tokio::time::sleep(self.opts.chunk_delay).await;
            }
        }

        let final_size = existing.len() as u64;
        let digest = crate::digest::of(&existing);
        // Into the job's own temporary path, not the destination: publishing is
        // `finalise`'s job, and the gap between them is where verification happens.
        let (parent, name) = split_parent(&req.remote_path);
        let leaf = crate::protocol::partial_name(&name, req.job);
        let temporary_path = if parent == "/" {
            format!("/{leaf}")
        } else {
            format!("{parent}/{leaf}")
        };
        self.fs.write_file(&temporary_path, existing);

        Ok(Transferred {
            temporary_path,
            bytes: written,
            final_size,
            digest,
            source_now: local_facts(&req.local_path).await,
        })
    }

    async fn finalise(&mut self, req: &TransferReq, done: &Transferred) -> Result<TransferOutcome> {
        let outcome = TransferOutcome {
            bytes: done.bytes,
            final_size: done.final_size,
        };
        let local = Path::new(&done.temporary_path);
        if tokio::fs::try_exists(local).await.unwrap_or(false) {
            tokio::fs::rename(local, &req.local_path)
                .await
                .map_err(|e| EngineError::from_io(&req.local_path, &e))?;
            return Ok(outcome);
        }
        if let Some(data) = self.fs.read_file(&done.temporary_path) {
            self.fs.write_file(&req.remote_path, data);
            self.fs.remove_file(&done.temporary_path);
        }
        Ok(outcome)
    }

    async fn discard(&mut self, temporary_path: &str) {
        let local = Path::new(temporary_path);
        if tokio::fs::try_exists(local).await.unwrap_or(false) {
            let _ = tokio::fs::remove_file(local).await;
            return;
        }
        self.fs.remove_file(temporary_path);
    }

    async fn prefix_digest(&mut self, path: &str, len: u64) -> Result<String> {
        let data = self
            .fs
            .read_file(path)
            .ok_or_else(|| EngineError::NotFound { path: path.into() })?;
        if (data.len() as u64) < len {
            return Err(EngineError::ResumeUnverifiable {
                reason: format!(
                    "{path} holds {} bytes, fewer than the {len} the checkpoint claims",
                    data.len()
                ),
            });
        }
        Ok(crate::digest::of(&data[..len as usize]))
    }

    async fn size_of(&mut self, path: &str) -> Result<Option<u64>> {
        Ok(self.fs.read_file(path).map(|data| data.len() as u64))
    }

    async fn close(self: Box<Self>) {}
}

/// Secrets that are simply absent, so the prompt path is the one under test.
pub struct NoSecrets;

#[async_trait]
impl SecretSource for NoSecrets {
    async fn password(&self, _server: &ServerConfig) -> Option<String> {
        None
    }
    async fn passphrase(&self, _server: &ServerConfig) -> Option<String> {
        None
    }
}

/// Hands the engine a [`MockBackend`] wherever it would build a real one.
///
/// This is the seam that lets `tests/engine_session.rs` drive the same code path the
/// shell calls — commands, actor, coordinator, prompts, events — with no network. A
/// test that stopped short of the engine would be testing everything except the part
/// that ships.
pub struct MockFactory {
    fs: MockFs,
    opts: MockOptions,
}

impl MockFactory {
    pub fn new(fs: MockFs, opts: MockOptions) -> Self {
        Self { fs, opts }
    }
}

impl crate::engine::BackendFactory for MockFactory {
    fn build(&self, session: SessionId, _cfg: &ServerConfig) -> Result<Box<dyn Protocol>> {
        Ok(Box::new(MockBackend::with_options(
            self.fs.clone(),
            session,
            self.opts.clone(),
        )))
    }
}
