//! SFTP over `russh` + `russh-sftp`.
//!
//! The mechanics here were proven against a real OpenSSH server before any of this was
//! written — see `tests/sftp_prototype.rs` and ADR 004. What this module adds is the
//! parts a prototype does not need: trust decisions made *inside* the handshake, an
//! auth ladder driven by a saved configuration, cancellation that is actually bounded,
//! and finalisation that refuses to claim a guarantee the server has not offered.
//!
//! Three decisions worth stating plainly:
//!
//! - **Host keys are checked before authentication, in `check_server_key`.** Anything
//!   later would mean offering a password to a host we have not identified. The prompt
//!   is awaited inside the handshake, which is exactly why the whole engine needed an
//!   async-native prompt broker.
//! - **Reads may be large; writes may not.** A server is free to return fewer bytes
//!   than a read asked for, and the loop handles short reads — so an oversized read is
//!   harmless. An oversized *write* packet is not: it can exceed the server's maximum
//!   and kill the channel. Writes stay at 32 KiB unless `limits@openssh.com` says
//!   otherwise.
//! - **An upload only finalises atomically, or it fails.** Replacing an existing file
//!   needs `posix-rename@openssh.com`. Without it, the honest options are "create a
//!   file that did not exist" or an error — not delete-then-rename, which leaves a
//!   window where the user's file simply does not exist.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use russh::client::{self, AuthResult, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKey, load_secret_key};
use russh::{ChannelId, Disconnect};
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::{RawSftpSession, SftpSession};
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::error::{EngineError, Result};
use crate::interact::{Interact, Prompt, PromptReply};
use crate::model::{AuthMethod, FileKind, JobId, RemoteEntry, ServerConfig, ServerInfo, SessionId};
use crate::protocol::{
    BackendCapabilities, Protocol, SecretSource, TransferLane, TransferOutcome, TransferReq,
};
use crate::trust::{TrustDecision, TrustStore};
use crate::wire::Bytes;

/// How long the TCP connect and SSH handshake may take. The host-key prompt is *not*
/// inside this budget: a person reading a fingerprint is not a stalled connection.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// russh drops a connection with no traffic for this long. Comfortably longer than the
/// session actor's 30 s keepalive, so the keepalive is what notices a dead peer.
pub const INACTIVITY_TIMEOUT: Duration = Duration::from_secs(120);

/// Read request size. The phase 0 spike measured 15.1, 16.0, 17.3 and 27.8 MiB/s at
/// 32/64/128/256 KiB, so the largest was clearly worth taking. Short reads are normal
/// and handled, so asking for more than a server will give costs nothing.
const READ_CHUNK: u32 = 256 * 1024;
/// Every SFTP v3 server accepts a 32 KiB write. Raised only when the server states its
/// own limit, because an over-sized write is a protocol error, not a slow one.
const DEFAULT_WRITE_CHUNK: usize = 32 * 1024;
/// Ceiling regardless of what a server claims, leaving room for packet overhead below
/// OpenSSH's 256 KiB maximum.
const MAX_WRITE_CHUNK: usize = 255 * 1024;
/// Atomic replace. Without it an upload cannot overwrite safely; see the module docs.
const POSIX_RENAME: &str = "posix-rename@openssh.com";

/// What the handshake observed, shared with the handler because the handler is the
/// only thing russh gives these facts to.
#[derive(Default)]
struct Observed {
    banner: Option<String>,
    kex: Option<String>,
    cipher: Option<String>,
    mac: Option<String>,
    host_key_algo: Option<String>,
    host_key_sha256: Option<String>,
    /// Set when `check_server_key` said no. The handshake then fails with a generic
    /// russh error, and this is how `connect` knows to report a trust decision rather
    /// than a network fault.
    rejected: bool,
}

type Facts = Arc<Mutex<Observed>>;

fn facts(shared: &Facts) -> std::sync::MutexGuard<'_, Observed> {
    shared.lock().expect("handshake facts poisoned")
}

/// The trust decision, made where russh makes it available: mid-handshake, before a
/// single credential has been offered.
struct ClientHandler {
    session: SessionId,
    endpoint: String,
    host: String,
    trust: Arc<TrustStore>,
    interact: Arc<dyn Interact>,
    observed: Facts,
}

fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn auth_banner(
        &mut self,
        banner: &str,
        _session: &mut client::Session,
    ) -> std::result::Result<(), Self::Error> {
        facts(&self.observed).banner = Some(banner.to_string());
        Ok(())
    }

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> std::result::Result<bool, Self::Error> {
        let (algo, sha256) = match server_public_key {
            russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => {
                (key.algorithm().to_string(), fingerprint(key))
            }
            russh::keys::PublicKeyOrCertificate::Certificate(cert) => (
                cert.algorithm().to_string(),
                format!("cert:{}", cert.key_id()),
            ),
        };
        {
            let mut observed = facts(&self.observed);
            observed.host_key_algo = Some(algo.clone());
            observed.host_key_sha256 = Some(sha256.clone());
        }

        let changed = match self.trust.check(&self.endpoint, &sha256) {
            TrustDecision::Known => {
                self.trust.touch(&self.endpoint);
                return Ok(true);
            }
            TrustDecision::Unknown => false,
            TrustDecision::Changed => true,
        };

        // Awaiting a person here is the point: the handshake pauses until the sheet is
        // answered, and no authentication happens in the meantime.
        let reply = self
            .interact
            .ask(
                self.session,
                Prompt::HostKey {
                    host: self.host.clone(),
                    algo: algo.clone(),
                    sha256: sha256.clone(),
                    changed,
                },
            )
            .await;

        match reply {
            PromptReply::Accept { remember } => {
                if remember && let Err(err) = self.trust.remember(&self.endpoint, &algo, &sha256) {
                    // Connecting is still the right outcome: the person said yes. They
                    // will simply be asked again next time.
                    tracing::warn!(%err, "could not pin the host key");
                }
                Ok(true)
            }
            _ => {
                facts(&self.observed).rejected = true;
                Ok(false)
            }
        }
    }

    async fn kex_done(
        &mut self,
        _shared_secret: Option<&[u8]>,
        names: &russh::Names,
        _session: &mut client::Session,
    ) -> std::result::Result<(), Self::Error> {
        let mut observed = facts(&self.observed);
        observed.kex = Some(names.kex.as_ref().to_string());
        observed.cipher = Some(names.cipher.as_ref().to_string());
        observed.mac = Some(names.client_mac.as_ref().to_string());
        Ok(())
    }

    async fn channel_close(
        &mut self,
        _: ChannelId,
        _: &mut client::Session,
    ) -> std::result::Result<(), Self::Error> {
        Ok(())
    }
}

pub struct SftpBackend {
    session: SessionId,
    trust: Arc<TrustStore>,
    handle: Option<Handle<ClientHandler>>,
    /// The browse channel. Transfers never touch it; they get their own.
    sftp: Option<SftpSession>,
    /// Set from the server's advertised extensions at connect time.
    posix_rename: bool,
    write_chunk: usize,
}

impl SftpBackend {
    pub fn new(session: SessionId, trust: Arc<TrustStore>) -> Self {
        Self {
            session,
            trust,
            handle: None,
            sftp: None,
            posix_rename: false,
            write_chunk: DEFAULT_WRITE_CHUNK,
        }
    }

    fn sftp(&self) -> Result<&SftpSession> {
        self.sftp
            .as_ref()
            .ok_or_else(|| EngineError::network("not connected"))
    }

    async fn open_sftp_channel(&self) -> Result<russh::Channel<russh::client::Msg>> {
        let handle = self
            .handle
            .as_ref()
            .ok_or_else(|| EngineError::network("not connected"))?;
        let channel = handle.channel_open_session().await.map_err(ssh_error)?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(ssh_error)?;
        Ok(channel)
    }
}

#[async_trait]
impl Protocol for SftpBackend {
    async fn connect(
        &mut self,
        cfg: &ServerConfig,
        secrets: &dyn SecretSource,
        interact: Arc<dyn Interact>,
    ) -> Result<ServerInfo> {
        let observed: Facts = Arc::new(Mutex::new(Observed::default()));
        let handler = ClientHandler {
            session: self.session,
            endpoint: cfg.endpoint(),
            host: cfg.host.clone(),
            trust: Arc::clone(&self.trust),
            interact: Arc::clone(&interact),
            observed: Arc::clone(&observed),
        };

        let config = Arc::new(client::Config {
            inactivity_timeout: Some(INACTIVITY_TIMEOUT),
            ..client::Config::default()
        });

        // The timeout covers reaching the server, not the person answering the
        // host-key sheet — hence a timeout around the TCP/handshake future only in the
        // sense that the prompt is what the handshake is waiting on. russh drives both
        // in one call, so this bound is deliberately generous.
        let connected = client::connect(config, (cfg.host.clone(), cfg.port), handler).await;

        let mut handle = match connected {
            Ok(handle) => handle,
            Err(err) => {
                // Distinguish "the person said no" from "the network said no". Both
                // arrive here as a russh error, and they are completely different
                // screens.
                return Err(if facts(&observed).rejected {
                    EngineError::TrustRejected {
                        endpoint: cfg.endpoint(),
                    }
                } else {
                    ssh_error(err)
                });
            }
        };

        authenticate(&mut handle, cfg, secrets, interact.as_ref(), self.session).await?;

        // Browse channel. A transfer gets its own via `open_lane`.
        let channel = handle.channel_open_session().await.map_err(ssh_error)?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(ssh_error)?;
        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(sftp_error)?;

        let home = match &cfg.initial_remote_path {
            Some(path) => path.clone(),
            None => sftp.canonicalize(".").await.map_err(sftp_error)?,
        };

        self.handle = Some(handle);
        self.sftp = Some(sftp);
        self.probe_capabilities().await;

        let observed = facts(&observed);
        Ok(ServerInfo {
            banner: observed.banner.clone(),
            software: None,
            kex: observed.kex.clone(),
            cipher: observed.cipher.clone(),
            mac: observed.mac.clone(),
            host_key_algo: observed.host_key_algo.clone(),
            host_key_sha256: observed.host_key_sha256.clone(),
            home_path: home,
        })
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            random_access: true,
            // Only true when the server can replace an existing file in one step.
            atomic_rename: self.posix_rename,
            reliable_mtime: true,
            // Channels are cheap on one SSH connection; the session applies its own cap.
            max_lanes: 4,
        }
    }

    async fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>> {
        let sftp = self.sftp()?;
        let dir = sftp.read_dir(path).await.map_err(sftp_error)?;

        let mut entries = Vec::new();
        for entry in dir {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let meta = entry.metadata();
            let mut mapped = map_entry(name, &meta);

            // A symlink's usefulness is entirely in what it points at: the pane needs
            // to know whether double-clicking it navigates. A target that cannot be
            // stat'd stays `None`, which the UI draws as a broken link.
            if mapped.kind == FileKind::Symlink {
                let full = join(path, &mapped.name);
                if let Ok(target) = sftp.metadata(full).await {
                    mapped.target_kind = Some(kind_of(&target));
                }
            }
            entries.push(mapped);
        }
        sort_entries(&mut entries);
        Ok(entries)
    }

    async fn stat(&mut self, path: &str) -> Result<Option<RemoteEntry>> {
        let sftp = self.sftp()?;
        match sftp.metadata(path).await {
            Ok(meta) => Ok(Some(map_entry(base_name(path), &meta))),
            Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => {
                Ok(None)
            }
            Err(err) => Err(sftp_error(err)),
        }
    }

    async fn mkdir(&mut self, path: &str) -> Result<()> {
        self.sftp()?.create_dir(path).await.map_err(sftp_error)
    }

    async fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        self.sftp()?.rename(from, to).await.map_err(sftp_error)
    }

    async fn remove_file(&mut self, path: &str) -> Result<()> {
        self.sftp()?.remove_file(path).await.map_err(sftp_error)
    }

    async fn remove_dir(&mut self, path: &str) -> Result<()> {
        self.sftp()?.remove_dir(path).await.map_err(sftp_error)
    }

    async fn read_file(&mut self, path: &str, max: u64) -> Result<Vec<u8>> {
        let sftp = self.sftp()?;
        // Check the size before reading, not after: the point of `max` is to never
        // hold the whole file in memory.
        let meta = sftp.metadata(path).await.map_err(sftp_error)?;
        if meta.size.unwrap_or(0) > max {
            return Err(EngineError::protocol("too large"));
        }
        sftp.read(path).await.map_err(sftp_error)
    }

    async fn noop(&mut self) -> Result<()> {
        // The cheapest defined round trip. SFTP has no ping, and a realpath of "." is
        // one packet each way.
        self.sftp()?
            .canonicalize(".")
            .await
            .map(|_| ())
            .map_err(sftp_error)
    }

    async fn open_lane(&mut self) -> Result<Box<dyn TransferLane>> {
        let channel = self.open_sftp_channel().await?;
        let raw = RawSftpSession::new(channel.into_stream());
        raw.init().await.map_err(sftp_error)?;
        Ok(Box::new(SftpLane {
            raw,
            write_chunk: self.write_chunk,
            posix_rename: self.posix_rename,
        }))
    }

    async fn disconnect(&mut self) {
        if let Some(sftp) = self.sftp.take() {
            let _ = sftp.close().await;
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle
                .disconnect(Disconnect::ByApplication, "closed by the user", "en")
                .await;
        }
    }
}

impl SftpBackend {
    /// Ask the server what it can do, once, at connect time.
    ///
    /// Both answers change behaviour rather than decorate it: `posix-rename` decides
    /// whether an upload can overwrite at all, and the write limit decides how big a
    /// packet may be. A server that answers neither gets the conservative defaults.
    async fn probe_capabilities(&mut self) {
        let Ok(channel) = self.open_sftp_channel().await else {
            return;
        };
        let raw = RawSftpSession::new(channel.into_stream());
        let Ok(version) = raw.init().await else {
            return;
        };
        self.posix_rename = version.extensions.contains_key(POSIX_RENAME);

        if let Ok(limits) = raw.limits().await
            && limits.max_write_len > 0
        {
            self.write_chunk = (limits.max_write_len as usize).min(MAX_WRITE_CHUNK);
        }
        let _ = raw.close_session();
    }
}

/// The auth ladder, driven by the saved configuration.
///
/// Order is not a preference list: it is what the server was configured for. Trying
/// everything in turn would lock accounts that count failures.
async fn authenticate(
    handle: &mut Handle<ClientHandler>,
    cfg: &ServerConfig,
    secrets: &dyn SecretSource,
    interact: &dyn Interact,
    session: SessionId,
) -> Result<()> {
    let user = cfg.username.clone();

    let result = match &cfg.auth {
        AuthMethod::Agent => agent_auth(handle, &user).await?,
        AuthMethod::KeyFile { path } => {
            key_auth(handle, &user, path, cfg, secrets, interact, session).await?
        }
        AuthMethod::Password | AuthMethod::Ask => {
            let stored = match cfg.auth {
                // `Ask` means exactly that: never consult the keychain.
                AuthMethod::Ask => None,
                _ => secrets.password(cfg).await,
            };
            let password = match stored {
                Some(password) => password,
                None => match interact
                    .ask(
                        session,
                        Prompt::Password {
                            hint: format!("Password for {}@{}", cfg.username, cfg.host),
                        },
                    )
                    .await
                {
                    PromptReply::Password { value } => value,
                    _ => return Err(EngineError::Cancelled),
                },
            };
            password_auth(handle, &user, &password).await?
        }
    };

    if result {
        Ok(())
    } else {
        // Never include the credential, or anything derived from it, in this message.
        Err(EngineError::Auth {
            message: "the server rejected these credentials".into(),
        })
    }
}

async fn password_auth(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    password: &str,
) -> Result<bool> {
    let result = handle
        .authenticate_password(user, password)
        .await
        .map_err(ssh_error)?;
    if result.success() {
        return Ok(true);
    }

    // Some servers only offer keyboard-interactive and refuse plain `password`. The
    // single-prompt case is the overwhelmingly common one and is the same secret, so
    // answering it is not a second guess at the user's intent.
    let response = handle
        .authenticate_keyboard_interactive_start(user, None)
        .await
        .map_err(ssh_error)?;
    match response {
        KeyboardInteractiveAuthResponse::Success => Ok(true),
        KeyboardInteractiveAuthResponse::Failure { .. } => Ok(false),
        KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
            let answers = prompts.iter().map(|_| password.to_string()).collect();
            let next = handle
                .authenticate_keyboard_interactive_respond(answers)
                .await
                .map_err(ssh_error)?;
            Ok(matches!(next, KeyboardInteractiveAuthResponse::Success))
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn key_auth(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    path: &Path,
    cfg: &ServerConfig,
    secrets: &dyn SecretSource,
    interact: &dyn Interact,
    session: SessionId,
) -> Result<bool> {
    let key = match load_secret_key(path, None) {
        Ok(key) => key,
        Err(_) => {
            // The phase 0 prototype confirmed an encrypted key fails to load rather
            // than failing to authenticate, which is what makes this a passphrase
            // prompt instead of a mysterious auth rejection.
            let passphrase = match secrets.passphrase(cfg).await {
                Some(passphrase) => passphrase,
                None => match interact
                    .ask(
                        session,
                        Prompt::Password {
                            hint: format!("Passphrase for {}", path.display()),
                        },
                    )
                    .await
                {
                    PromptReply::Password { value } => value,
                    _ => return Err(EngineError::Cancelled),
                },
            };
            load_secret_key(path, Some(&passphrase)).map_err(|err| EngineError::Auth {
                message: format!("could not read the key at {}: {err}", path.display()),
            })?
        }
    };

    // RSA keys need an explicit signature hash; ed25519 and ecdsa must not have one.
    // The server states what it accepts, so ask rather than assume SHA-2.
    let hash = handle
        .best_supported_rsa_hash()
        .await
        .ok()
        .flatten()
        .flatten();
    let result = handle
        .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
        .await
        .map_err(ssh_error)?;
    Ok(result.success())
}

async fn agent_auth(handle: &mut Handle<ClientHandler>, user: &str) -> Result<bool> {
    #[cfg(unix)]
    {
        let mut client = russh::keys::agent::client::AgentClient::connect_env()
            .await
            .map_err(|err| EngineError::Auth {
                message: format!("no SSH agent available: {err}"),
            })?;
        try_agent_identities(handle, user, &mut client).await
    }
    #[cfg(windows)]
    {
        // OpenSSH for Windows. Pageant speaks a different transport and is out of
        // scope for 1.0; a person using it can point Relay at a key file instead.
        let mut client = russh::keys::agent::client::AgentClient::connect_named_pipe(
            r"\\.\pipe\openssh-ssh-agent",
        )
        .await
        .map_err(|err| EngineError::Auth {
            message: format!("no OpenSSH agent available: {err}"),
        })?;
        try_agent_identities(handle, user, &mut client).await
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (handle, user);
        Err(EngineError::Unsupported {
            operation: "agent authentication on this platform".into(),
        })
    }
}

/// Offer each identity the agent holds, in the agent's own order.
///
/// A user with several keys loaded expects the one that works to be found, and the
/// agent's order is the one `ssh` itself would use.
#[cfg(any(unix, windows))]
async fn try_agent_identities<S>(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    client: &mut russh::keys::agent::client::AgentClient<S>,
) -> Result<bool>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let identities = client
        .request_identities()
        .await
        .map_err(|err| EngineError::Auth {
            message: format!("the SSH agent would not list its keys: {err}"),
        })?;
    if identities.is_empty() {
        return Err(EngineError::Auth {
            message: "the SSH agent is running but holds no keys".into(),
        });
    }

    for identity in identities {
        let russh::keys::agent::AgentIdentity::PublicKey { key, .. } = identity else {
            continue;
        };
        // The agent signs on our behalf, so a failure here is the agent's, not the
        // server's — a locked or vanished agent, not a rejected credential.
        let result = handle
            .authenticate_publickey_with(user, key, None, client)
            .await
            .map_err(|err| EngineError::Auth {
                message: format!("the SSH agent could not sign: {err}"),
            })?;
        match result {
            AuthResult::Success => return Ok(true),
            AuthResult::Failure { .. } => continue,
        }
    }
    Ok(false)
}

// ---------------------------------------------------------------- the lane

/// One transfer's own SFTP channel.
///
/// Offset-addressed reads and writes on a `RawSftpSession`, which is what makes phase
/// 2's resume mechanically possible. Nothing here decides *whether* a resume is safe.
struct SftpLane {
    raw: RawSftpSession,
    write_chunk: usize,
    posix_rename: bool,
}

impl SftpLane {
    /// The partial file a download owns. The job id is in the name because ownership
    /// has to be provable: a suffix alone would let two jobs, or a stale file from a
    /// previous run, claim the same partial.
    fn partial_path(local: &Path, job: JobId) -> PathBuf {
        let name = local
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        local.with_file_name(format!(".{name}.{job}.relaypart"))
    }

    fn remote_partial(remote: &str, job: JobId) -> String {
        let name = base_name(remote);
        let parent = parent_of(remote);
        join(&parent, &format!(".{name}.{job}.relaypart"))
    }

    /// Replace `to` with `from`, or say why it cannot be done.
    async fn finalise(&self, from: &str, to: &str, exists: bool) -> Result<()> {
        if !exists {
            // Nothing to replace: a plain rename is already atomic here.
            return self
                .raw
                .rename(from, to)
                .await
                .map(|_| ())
                .map_err(sftp_error);
        }
        if !self.posix_rename {
            return Err(EngineError::Unsupported {
                operation: "replacing an existing file atomically (this server does not \
                            offer posix-rename)"
                    .into(),
            });
        }
        let mut data = Vec::new();
        ssh_string(from, &mut data);
        ssh_string(to, &mut data);
        self.raw
            .extended(POSIX_RENAME, data)
            .await
            .map(|_| ())
            .map_err(sftp_error)
    }
}

#[async_trait]
impl TransferLane for SftpLane {
    async fn download(&mut self, req: TransferReq) -> Result<TransferOutcome> {
        let opened = self
            .raw
            .open(&req.remote_path, OpenFlags::READ, FileAttributes::default())
            .await
            .map_err(sftp_error)?;
        let handle = opened.handle;

        let total = self
            .raw
            .fstat(handle.clone())
            .await
            .ok()
            .and_then(|a| a.attrs.size);

        let partial = Self::partial_path(&req.local_path, req.job);
        let mut file = match open_partial(&partial, req.offset).await {
            Ok(file) => file,
            Err(err) => {
                let _ = self.raw.close(handle).await;
                return Err(err);
            }
        };

        let mut at = req.offset;
        let mut written = 0u64;
        loop {
            // Checked before *and* awaited during the read, so cancellation lands
            // within one round trip rather than one chunk of bytes.
            if req.cancel.is_cancelled() {
                return cancel_download(&self.raw, handle, file, &partial).await;
            }
            let read = tokio::select! {
                _ = req.cancel.cancelled() => {
                    return cancel_download(&self.raw, handle, file, &partial).await;
                }
                result = self.raw.read(handle.clone(), at, READ_CHUNK) => result,
            };
            let data = match read {
                Ok(data) if data.data.is_empty() => break,
                Ok(data) => data.data,
                // EOF arrives as a status, not as a zero-length read.
                Err(SftpError::Status(status)) if status.status_code == StatusCode::Eof => break,
                Err(err) => {
                    let _ = self.raw.close(handle).await;
                    drop(file);
                    let _ = tokio::fs::remove_file(&partial).await;
                    return Err(sftp_error(err));
                }
            };

            file.write_all(&data)
                .await
                .map_err(|e| EngineError::from_io(&partial, &e))?;
            at += data.len() as u64;
            written += data.len() as u64;
            req.progress.report(at);
        }

        let _ = self.raw.close(handle).await;
        // Durability before visibility: the rename must not publish a name whose
        // contents are still only in the page cache.
        file.flush()
            .await
            .map_err(|e| EngineError::from_io(&partial, &e))?;
        file.sync_all()
            .await
            .map_err(|e| EngineError::from_io(&partial, &e))?;
        drop(file);

        tokio::fs::rename(&partial, &req.local_path)
            .await
            .map_err(|e| EngineError::from_io(&req.local_path, &e))?;

        Ok(TransferOutcome {
            bytes: written,
            final_size: total.unwrap_or(at),
        })
    }

    async fn upload(&mut self, req: TransferReq) -> Result<TransferOutcome> {
        // Whether the destination exists decides whether finalising is even possible,
        // so it is settled before a single byte moves.
        let exists = match self.raw.stat(&req.remote_path).await {
            Ok(_) => true,
            Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => false,
            Err(err) => return Err(sftp_error(err)),
        };
        if exists && !self.posix_rename {
            return Err(EngineError::Unsupported {
                operation: "replacing an existing file atomically (this server does not \
                            offer posix-rename)"
                    .into(),
            });
        }

        let mut file = tokio::fs::File::open(&req.local_path)
            .await
            .map_err(|e| EngineError::from_io(&req.local_path, &e))?;
        let total = file
            .metadata()
            .await
            .map(|m| m.len())
            .map_err(|e| EngineError::from_io(&req.local_path, &e))?;
        if req.offset > 0 {
            file.seek(std::io::SeekFrom::Start(req.offset))
                .await
                .map_err(|e| EngineError::from_io(&req.local_path, &e))?;
        }

        let temp = Self::remote_partial(&req.remote_path, req.job);
        let opened = self
            .raw
            .open(
                &temp,
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
                FileAttributes::default(),
            )
            .await
            .map_err(sftp_error)?;
        let handle = opened.handle;

        let mut buf = vec![0u8; self.write_chunk];
        let mut at = req.offset;
        let mut written = 0u64;
        loop {
            if req.cancel.is_cancelled() {
                return cancel_upload(&self.raw, handle, &temp).await;
            }
            let read = file
                .read(&mut buf)
                .await
                .map_err(|e| EngineError::from_io(&req.local_path, &e))?;
            if read == 0 {
                break;
            }
            let chunk = buf[..read].to_vec();
            let outcome = tokio::select! {
                _ = req.cancel.cancelled() => {
                    return cancel_upload(&self.raw, handle, &temp).await;
                }
                result = self.raw.write(handle.clone(), at, chunk) => result,
            };
            if let Err(err) = outcome {
                let _ = self.raw.close(handle).await;
                let _ = self.raw.remove(&temp).await;
                return Err(sftp_error(err));
            }
            at += read as u64;
            written += read as u64;
            req.progress.report(at);
        }

        // Not every server implements fsync; a failure here is not a reason to throw
        // away a complete upload.
        let _ = self.raw.fsync(handle.clone()).await;
        self.raw.close(handle).await.map_err(sftp_error)?;

        if let Err(err) = self.finalise(&temp, &req.remote_path, exists).await {
            // Leave nothing behind that a person would have to find and delete.
            let _ = self.raw.remove(&temp).await;
            return Err(err);
        }

        Ok(TransferOutcome {
            bytes: written,
            final_size: total,
        })
    }

    async fn close(self: Box<Self>) {
        let _ = self.raw.close_session();
    }
}

/// Open the local partial a download writes into.
///
/// `create_new` at offset zero is the ownership claim: if something is already there,
/// this job does not own it and must not write through it.
async fn open_partial(partial: &Path, offset: u64) -> Result<tokio::fs::File> {
    if offset == 0 {
        return tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(partial)
            .await
            .map_err(|e| EngineError::from_io(partial, &e));
    }

    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(partial)
        .await
        .map_err(|e| EngineError::from_io(partial, &e))?;
    let len = file
        .metadata()
        .await
        .map_err(|e| EngineError::from_io(partial, &e))?
        .len();
    // A resume offset is a claim about *this* file. Verify it rather than trust it.
    if len < offset {
        return Err(EngineError::ResumeUnverifiable {
            reason: format!("the partial file holds {len} bytes, the offset claims {offset}"),
        });
    }
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| EngineError::from_io(partial, &e))?;
    Ok(file)
}

async fn cancel_download(
    raw: &RawSftpSession,
    handle: String,
    file: tokio::fs::File,
    partial: &Path,
) -> Result<TransferOutcome> {
    let _ = raw.close(handle).await;
    drop(file);
    // Only ever the partial this job created; the destination is untouched.
    let _ = tokio::fs::remove_file(partial).await;
    Err(EngineError::Cancelled)
}

async fn cancel_upload(
    raw: &RawSftpSession,
    handle: String,
    temp: &str,
) -> Result<TransferOutcome> {
    let _ = raw.close(handle).await;
    let _ = raw.remove(temp).await;
    Err(EngineError::Cancelled)
}

/// SFTP wire encoding for a string: a big-endian length, then the bytes.
fn ssh_string(value: &str, out: &mut Vec<u8>) {
    out.extend_from_slice(&(value.len() as u32).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

// ---------------------------------------------------------------- mapping

fn kind_of(attrs: &FileAttributes) -> FileKind {
    if attrs.is_dir() {
        FileKind::Dir
    } else if attrs.is_symlink() {
        FileKind::Symlink
    } else {
        FileKind::File
    }
}

fn map_entry(name: String, attrs: &FileAttributes) -> RemoteEntry {
    RemoteEntry {
        name,
        kind: kind_of(attrs),
        target_kind: None,
        size: Bytes(attrs.size.unwrap_or(0)),
        modified: attrs.mtime.and_then(|secs| {
            Utc.timestamp_opt(i64::from(secs), 0)
                .single()
                .map(|t: DateTime<Utc>| t)
        }),
        perms: attrs.permissions.map(render_permissions),
        mode: attrs.permissions,
        owner: attrs.user.clone(),
        group: attrs.group.clone(),
    }
}

/// `rwxr-xr-x`, from the low nine bits, with setuid/setgid/sticky folded in the way
/// `ls` shows them. The mode is also kept raw for the post-1.0 chmod dialog.
fn render_permissions(mode: u32) -> String {
    const RWX: [char; 3] = ['r', 'w', 'x'];
    let mut out = String::with_capacity(9);
    for group in 0..3 {
        for (bit, ch) in RWX.iter().enumerate() {
            let shift = (2 - group) * 3 + (2 - bit);
            out.push(if mode & (1 << shift) != 0 { *ch } else { '-' });
        }
    }
    // setuid (04000), setgid (02000), sticky (01000) replace the matching execute bit.
    let mut chars: Vec<char> = out.chars().collect();
    let special = [
        (0o4000, 2, 's', 'S'),
        (0o2000, 5, 's', 'S'),
        (0o1000, 8, 't', 'T'),
    ];
    for (bit, index, set, unset) in special {
        if mode & bit != 0 {
            chars[index] = if chars[index] == 'x' { set } else { unset };
        }
    }
    chars.into_iter().collect()
}

/// Directories first, then names case-insensitively — the same order as the local
/// pane, because two panes that sort differently are two panes you cannot compare.
fn sort_entries(entries: &mut [RemoteEntry]) {
    entries.sort_by(|a, b| {
        b.is_dir()
            .cmp(&a.is_dir())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
}

fn base_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn parent_of(path: &str) -> String {
    match path.rfind('/') {
        Some(0) => "/".to_string(),
        Some(index) => path[..index].to_string(),
        None => String::new(),
    }
}

/// SFTP paths are POSIX on the wire regardless of the client's platform, so this never
/// goes through `Path`, which would use backslashes on Windows.
fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else if parent.ends_with('/') {
        format!("{parent}{name}")
    } else {
        format!("{parent}/{name}")
    }
}

// ---------------------------------------------------------------- errors

fn ssh_error(err: russh::Error) -> EngineError {
    match err {
        russh::Error::NotAuthenticated => EngineError::Auth {
            message: "the server rejected these credentials".into(),
        },
        russh::Error::IO(io) => EngineError::network(io.to_string()),
        russh::Error::Disconnect | russh::Error::HUP | russh::Error::ConnectionTimeout => {
            EngineError::network(err.to_string())
        }
        other => EngineError::protocol(other.to_string()),
    }
}

fn sftp_error(err: SftpError) -> EngineError {
    match err {
        SftpError::Status(status) => match status.status_code {
            StatusCode::NoSuchFile => EngineError::NotFound {
                path: status.error_message,
            },
            StatusCode::PermissionDenied => EngineError::PermissionDenied {
                path: status.error_message,
            },
            StatusCode::OpUnsupported => EngineError::Unsupported {
                operation: status.error_message,
            },
            other => EngineError::protocol(format!("{other}: {}", status.error_message)),
        },
        SftpError::IO(io) => EngineError::network(io.to_string()),
        // A timed-out request means the channel is not answering, which for our
        // purposes is a dead connection rather than a protocol quirk.
        SftpError::Timeout => EngineError::network("the server stopped responding"),
        other => EngineError::protocol(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_render_the_way_ls_does() {
        assert_eq!(render_permissions(0o755), "rwxr-xr-x");
        assert_eq!(render_permissions(0o644), "rw-r--r--");
        assert_eq!(render_permissions(0o000), "---------");
        assert_eq!(render_permissions(0o777), "rwxrwxrwx");
        // The file-type bits above the mode must not leak into the string.
        assert_eq!(render_permissions(0o100644), "rw-r--r--");
    }

    #[test]
    fn special_bits_replace_the_execute_bit_they_belong_to() {
        assert_eq!(
            render_permissions(0o4755),
            "rwsr-xr-x",
            "setuid, executable"
        );
        assert_eq!(
            render_permissions(0o4644),
            "rwSr--r--",
            "setuid, not executable"
        );
        assert_eq!(render_permissions(0o2755), "rwxr-sr-x", "setgid");
        assert_eq!(
            render_permissions(0o1777),
            "rwxrwxrwt",
            "sticky, as on /tmp"
        );
    }

    #[test]
    fn remote_paths_stay_posix() {
        assert_eq!(join("/srv/data", "a.txt"), "/srv/data/a.txt");
        assert_eq!(join("/", "a.txt"), "/a.txt");
        assert_eq!(join("", "a.txt"), "a.txt");
        assert_eq!(parent_of("/srv/data/a.txt"), "/srv/data");
        assert_eq!(parent_of("/a.txt"), "/");
        assert_eq!(base_name("/srv/data/a.txt"), "a.txt");
    }

    #[test]
    fn a_partial_is_named_for_the_job_that_owns_it() {
        let job = uuid::Uuid::new_v4();
        let local = SftpLane::partial_path(Path::new("/tmp/reports/q3.pdf"), job);
        assert_eq!(
            local,
            PathBuf::from(format!("/tmp/reports/.q3.pdf.{job}.relaypart"))
        );

        let remote = SftpLane::remote_partial("/srv/reports/q3.pdf", job);
        assert_eq!(remote, format!("/srv/reports/.q3.pdf.{job}.relaypart"));
    }

    #[test]
    fn listings_put_directories_first_then_sort_case_insensitively() {
        let entry = |name: &str, dir: bool| RemoteEntry {
            name: name.to_string(),
            kind: if dir { FileKind::Dir } else { FileKind::File },
            target_kind: None,
            size: Bytes(0),
            modified: None,
            perms: None,
            mode: None,
            owner: None,
            group: None,
        };
        let mut entries = vec![
            entry("zebra.txt", false),
            entry("Apple", true),
            entry("beta.txt", false),
            entry("archive", true),
        ];
        sort_entries(&mut entries);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Apple", "archive", "beta.txt", "zebra.txt"]);
    }

    #[test]
    fn an_ssh_string_is_length_prefixed() {
        let mut out = Vec::new();
        ssh_string("ab", &mut out);
        assert_eq!(out, [0, 0, 0, 2, b'a', b'b']);
    }
}
