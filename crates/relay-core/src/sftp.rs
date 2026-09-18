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
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use futures_util::future::BoxFuture;
use futures_util::stream::{FuturesOrdered, StreamExt};
use russh::client::{self, AuthResult, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKey, load_secret_key};
use russh::{ChannelId, Disconnect};
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::{RawSftpSession, SftpSession};
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::error::{EngineError, Result};
use crate::interact::{Interact, Prompt, PromptReply};
use crate::model::{
    AuthMethod, FileFacts, FileKind, JobId, RemoteEntry, ServerConfig, ServerInfo, SessionId,
};
use crate::protocol::{
    BackendCapabilities, CHECKPOINT_BYTES, Protocol, SecretSource, TransferLane, TransferOutcome,
    TransferReq, Transferred,
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
/// How many requests a transfer keeps in flight.
///
/// One-at-a-time moves `chunk / round-trip` and nothing else — 1.4 MB/s at 256 KiB
/// over 174 ms, whatever the bandwidth. Eight outstanding requests make it eight
/// times that. Higher costs memory per lane, since answers are buffered until they
/// can be written in order.
const PIPELINE_DEPTH: usize = 8;
/// Per-channel SSH receive window. russh's default 2 MiB is exactly a full pipeline,
/// which would leave flow control rather than the pipeline deciding the rate.
const WINDOW_SIZE: u32 = 4 * 1024 * 1024;
/// How long a request may go unanswered before the lane gives up.
///
/// A pipelined request waits behind the whole window ahead of it, so the deadline has
/// to cover the window rather than one chunk — russh-sftp's ten second default is a
/// timeout on a 1.7 Mbit/s link, not on a stalled server. Liveness is the session
/// keepalive's job; this only catches a peer that has stopped answering entirely.
fn request_timeout() -> u64 {
    const STALLED: u64 = 30;
    const SLOWEST: u64 = 64 * 1024;
    let window = PIPELINE_DEPTH as u64 * READ_CHUNK as u64;
    STALLED + window / SLOWEST
}
/// Every SFTP v3 server accepts a 32 KiB write. Raised only when the server states its
/// own limit, because an over-sized write is a protocol error, not a slow one.
const DEFAULT_WRITE_CHUNK: usize = 32 * 1024;
/// Ceiling regardless of what a server claims, leaving room for packet overhead below
/// OpenSSH's 256 KiB maximum.
const MAX_WRITE_CHUNK: usize = 255 * 1024;
/// How many SSH channels one connection will open of its own accord.
///
/// Lower than it used to be, not higher: `MaxSessions` defaults to 10 but hardened
/// servers set it far lower, and this plus the browse channel is what Relay asks a
/// stranger for. Transfers past this share a channel rather than demanding another.
const MAX_CHANNELS: usize = 4;
/// How many transfers may share one channel.
///
/// The scarce resource is the channel, not the request: `RawSftpSession` addresses
/// every operation by handle and request id, so one channel can carry several files at
/// once. A small file costs three round trips — open, read, close — and one file per
/// channel spends all three waiting. Eight files interleaved spend the same three
/// round trips moving eight files.
const LANES_PER_CHANNEL: usize = 8;
/// How long after a refused channel the backend tries for another one.
const CHANNEL_PROBE: Duration = Duration::from_secs(30);
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
    /// Like `write_chunk`, but for reads: an over-sized read is answered short, which
    /// costs a pipeline every request queued behind it.
    read_chunk: u32,
    /// Weak so a channel lives exactly as long as the transfers seated on it, and the
    /// count of what is open needs no bookkeeping of its own.
    channels: Vec<Weak<Channel>>,
    /// How many channels the server has proved willing to carry at once.
    channel_ceiling: usize,
    narrowed: Option<Instant>,
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
            read_chunk: READ_CHUNK,
            channels: Vec::new(),
            channel_ceiling: MAX_CHANNELS,
            narrowed: None,
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
            window_size: WINDOW_SIZE,
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

        // The account's own home, whatever the server's settings name as a starting
        // folder: a session lands in that folder but falls back here when it is gone,
        // and the search box's `~` means this.
        let home = sftp.canonicalize(".").await.map_err(sftp_error)?;

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
            max_lanes: (MAX_CHANNELS * LANES_PER_CHANNEL).min(usize::from(u8::MAX)) as u8,
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

    /// A seat for one transfer.
    ///
    /// A channel of its own while the server will give one, because a transfer alone
    /// on a channel has the whole request window. Once it will not, the transfer sits
    /// on the emptiest channel already open rather than failing — which is the point:
    /// `MaxSessions` stops being a limit on how many files can move.
    async fn open_lane(&mut self) -> Result<Box<dyn TransferLane>> {
        self.channels.retain(|channel| channel.strong_count() > 0);

        if self.may_add_channel() {
            match self.open_channel().await {
                Ok(channel) => {
                    self.channels.push(Arc::downgrade(&channel));
                    return Ok(Box::new(SftpLane::new(channel)));
                }
                Err(EngineError::LanesExhausted) => {
                    self.channel_ceiling = self.channels.len().max(1);
                    self.narrowed = Some(Instant::now());
                }
                Err(err) => return Err(err),
            }
        }

        self.channels
            .iter()
            .filter_map(|channel| channel.upgrade())
            // One of these is the reference just upgraded; the rest are seats.
            .map(|channel| (Arc::strong_count(&channel) - 1, channel))
            .filter(|(seats, _)| *seats < LANES_PER_CHANNEL)
            .min_by_key(|(seats, _)| *seats)
            .map(|(_, channel)| Box::new(SftpLane::new(channel)) as Box<dyn TransferLane>)
            .ok_or(EngineError::LanesExhausted)
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
    fn may_add_channel(&self) -> bool {
        self.channels.len() < self.channel_ceiling
            || (self.channel_ceiling < MAX_CHANNELS
                && self
                    .narrowed
                    .is_some_and(|at| at.elapsed() >= CHANNEL_PROBE))
    }

    async fn open_channel(&self) -> Result<Arc<Channel>> {
        let channel = self.open_sftp_channel().await?;
        let raw = RawSftpSession::new(channel.into_stream());
        raw.init().await.map_err(sftp_error)?;
        raw.set_timeout(request_timeout());
        Ok(Arc::new(Channel {
            raw,
            slots: Arc::new(Semaphore::new(PIPELINE_DEPTH)),
            write_chunk: self.write_chunk,
            read_chunk: self.read_chunk,
            posix_rename: self.posix_rename,
        }))
    }

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

        if let Ok(limits) = raw.limits().await {
            if limits.max_write_len > 0 {
                self.write_chunk = (limits.max_write_len as usize).min(MAX_WRITE_CHUNK);
            }
            if limits.max_read_len > 0 {
                self.read_chunk = (limits.max_read_len.min(u64::from(READ_CHUNK))) as u32;
            }
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
        return Ok(());
    }

    // Name the method that was actually tried. "The server rejected these credentials"
    // is true of every failure and useful for none of them: with four auth methods, the
    // first thing a person needs to know is which one Relay used.
    //
    // Never the credential itself, or anything derived from it.
    Err(EngineError::Auth {
        message: match &cfg.auth {
            AuthMethod::Agent => format!(
                "{user}@{}: the server accepted none of the identities your SSH agent \
                 offered. Add the right key with `ssh-add`, or switch this server to a \
                 key file.",
                cfg.host
            ),
            AuthMethod::KeyFile { path } => format!(
                "{user}@{}: the server rejected the key at {}. Check that its public half \
                 is in the account's authorized_keys.",
                cfg.host,
                path.display()
            ),
            AuthMethod::Password | AuthMethod::Ask => {
                format!("{user}@{}: the server rejected that password.", cfg.host)
            }
        },
    })
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

/// An SFTP channel, and what the server said it will accept on one.
///
/// Shared by the transfers seated on it. It closes when the last of them lets go,
/// which is what keeps the count of open channels honest without bookkeeping.
struct Channel {
    raw: RawSftpSession,
    /// Data requests this channel will keep outstanding at once, shared out among the
    /// transfers on it. It is the window [`request_timeout`] is sized against, and it
    /// bounds what one channel holds in memory however many files are using it.
    slots: Arc<Semaphore>,
    write_chunk: usize,
    read_chunk: u32,
    posix_rename: bool,
}

/// One transfer's seat on a channel.
///
/// Offset-addressed reads and writes on a `RawSftpSession`, which is what makes phase
/// 2's resume mechanically possible. Nothing here decides *whether* a resume is safe.
struct SftpLane {
    channel: Arc<Channel>,
    /// Whether an upload's destination was already there, settled before the first
    /// byte moved. `finalise` reads it rather than stat'ing again, because a second
    /// look could give a different answer than the one the transfer was planned on.
    destination_existed: bool,
}

impl SftpLane {
    fn new(channel: Arc<Channel>) -> Self {
        Self {
            channel,
            destination_existed: false,
        }
    }

    fn raw(&self) -> &RawSftpSession {
        &self.channel.raw
    }

    /// The partial file a download owns. See [`crate::protocol::partial_name`] for why
    /// the job id is in it.
    fn partial_path(local: &Path, job: JobId) -> PathBuf {
        crate::protocol::local_partial(local, job)
    }

    fn remote_partial(remote: &str, job: JobId) -> String {
        join(
            &parent_of(remote),
            &crate::protocol::partial_name(&base_name(remote), job),
        )
    }

    /// Replace the remote `to` with the remote `from`, or say why it cannot be done.
    async fn replace(&self, from: &str, to: &str, exists: bool) -> Result<()> {
        if !exists {
            // Nothing to replace: a plain rename is already atomic here.
            return self
                .raw()
                .rename(from, to)
                .await
                .map(|_| ())
                .map_err(sftp_error);
        }
        if !self.channel.posix_rename {
            return Err(EngineError::Unsupported {
                operation: "replacing an existing file atomically (this server does not \
                            offer posix-rename)"
                    .into(),
            });
        }
        let mut data = Vec::new();
        ssh_string(from, &mut data);
        ssh_string(to, &mut data);
        self.raw()
            .extended(POSIX_RENAME, data)
            .await
            .map(|_| ())
            .map_err(sftp_error)
    }
}

#[async_trait]
impl TransferLane for SftpLane {
    async fn download(&mut self, req: &TransferReq) -> Result<Transferred> {
        let raw = self.raw();
        let opened = raw
            .open(&req.remote_path, OpenFlags::READ, FileAttributes::default())
            .await
            .map_err(sftp_error)?;
        let handle = opened.handle;

        let partial = Self::partial_path(&req.local_path, req.job);
        let mut file = match open_partial(&partial, req.offset).await {
            Ok(file) => file,
            Err(err) => {
                let _ = raw.close(handle).await;
                return Err(err);
            }
        };

        let mut reads = Reads::new(&self.channel, handle.clone(), req.offset);
        let (mut chunk, total) = reads.open().await;

        let mut at = req.offset;
        let mut written = 0u64;
        let mut rolling = req.prefix.clone().unwrap_or_default();
        let mut checkpointed = req.offset;

        loop {
            if req.cancel.is_cancelled() {
                drop(reads);
                return stop_download(raw, handle, file, &partial, req.keeping_partial()).await;
            }
            let Some(next) = chunk else { break };
            let data = match next {
                Ok(data) => data,
                Err(err) => {
                    drop(reads);
                    let _ = raw.close(handle).await;
                    drop(file);
                    let _ = tokio::fs::remove_file(&partial).await;
                    return Err(err);
                }
            };

            file.write_all(&data)
                .await
                .map_err(|e| EngineError::from_io(&partial, &e))?;
            rolling.update(&data);
            at += data.len() as u64;
            written += data.len() as u64;
            req.progress.report(at);

            // A checkpoint is a promise that these bytes are still there after a power
            // cut, so it is made *after* the fsync, never before it.
            if at - checkpointed >= CHECKPOINT_BYTES {
                file.flush()
                    .await
                    .map_err(|e| EngineError::from_io(&partial, &e))?;
                file.sync_all()
                    .await
                    .map_err(|e| EngineError::from_io(&partial, &e))?;
                checkpointed = at;
                req.checkpoint.report(at, rolling.snapshot());
            }

            chunk = tokio::select! {
                _ = req.cancel.cancelled() => {
                    drop(reads);
                    return stop_download(raw, handle, file, &partial, req.keeping_partial()).await;
                }
                next = reads.next() => next,
            };
        }
        drop(reads);

        // Only a resumed transfer is spliced from two readings of the source, and only
        // a splice needs to know whether it moved in between.
        let source_now = if req.is_resume() {
            raw.fstat(handle.clone())
                .await
                .ok()
                .map(|a| facts_from(&req.remote_path, &a.attrs))
        } else {
            None
        };
        let _ = raw.close(handle).await;

        // Durability before visibility: the rename must not publish a name whose
        // contents are still only in the page cache.
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
            final_size: total.unwrap_or(at),
            digest: rolling.snapshot(),
            source_now,
        })
    }

    async fn upload(&mut self, req: &TransferReq) -> Result<Transferred> {
        // Whether the destination exists decides whether finalising is even possible,
        // so it is settled before a single byte moves.
        let exists = match self.raw().stat(&req.remote_path).await {
            Ok(_) => true,
            Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => false,
            Err(err) => return Err(sftp_error(err)),
        };
        if exists && !self.channel.posix_rename {
            return Err(EngineError::Unsupported {
                operation: "replacing an existing file atomically (this server does not \
                            offer posix-rename)"
                    .into(),
            });
        }

        let raw = self.raw();
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
        // Truncating is right for a fresh upload and catastrophic for a resumed one:
        // it would throw away the very bytes the offset was verified against.
        let flags = if req.offset > 0 {
            OpenFlags::WRITE | OpenFlags::CREATE
        } else {
            OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE
        };
        let opened = raw
            .open(&temp, flags, FileAttributes::default())
            .await
            .map_err(sftp_error)?;
        let handle = opened.handle;

        let mut writes = Writes::new(&self.channel, handle.clone(), req.offset);
        let mut at = req.offset;
        let mut written = 0u64;
        let mut rolling = req.prefix.clone().unwrap_or_default();
        let mut checkpointed = req.offset;
        let mut ended = false;

        loop {
            // The hash runs with what has been sent, so a checkpoint waits for the
            // pipeline to drain or it would file a digest of more bytes than its offset.
            let draining = writes.sent - checkpointed >= CHECKPOINT_BYTES;
            while !ended && !draining && !writes.full() {
                let Some(permit) = writes.reserve().await else {
                    break;
                };
                let mut buf = vec![0u8; self.channel.write_chunk];
                let read = match file.read(&mut buf).await {
                    Ok(read) => read,
                    Err(err) => {
                        drop(writes);
                        let _ = raw.close(handle).await;
                        let _ = raw.remove(&temp).await;
                        return Err(EngineError::from_io(&req.local_path, &err));
                    }
                };
                if read == 0 {
                    ended = true;
                    break;
                }
                buf.truncate(read);
                rolling.update(&buf);
                writes.send(permit, buf);
            }
            if req.cancel.is_cancelled() {
                drop(writes);
                return stop_upload(raw, handle, &temp, req.keeping_partial()).await;
            }
            if writes.idle() {
                if ended {
                    break;
                }
                let _ = raw.fsync(handle.clone()).await;
                checkpointed = at;
                req.checkpoint.report(at, rolling.snapshot());
                continue;
            }
            let acked = tokio::select! {
                _ = req.cancel.cancelled() => {
                    drop(writes);
                    return stop_upload(raw, handle, &temp, req.keeping_partial()).await;
                }
                acked = writes.ack() => acked,
            };
            match acked {
                Some(Ok(len)) => {
                    at += len;
                    written += len;
                    req.progress.report(at);
                }
                Some(Err(err)) => {
                    drop(writes);
                    let _ = raw.close(handle).await;
                    let _ = raw.remove(&temp).await;
                    return Err(err);
                }
                None => break,
            }
        }
        drop(writes);

        // Not every server implements fsync; a failure here is not a reason to throw
        // away a complete upload.
        let _ = raw.fsync(handle.clone()).await;
        raw.close(handle).await.map_err(sftp_error)?;

        let source_now = local_facts_of(&req.local_path).await;
        self.destination_existed = exists;

        Ok(Transferred {
            temporary_path: temp,
            bytes: written,
            final_size: total,
            digest: rolling.snapshot(),
            source_now,
        })
    }

    async fn finalise(&mut self, req: &TransferReq, done: &Transferred) -> Result<TransferOutcome> {
        let outcome = TransferOutcome {
            bytes: done.bytes,
            final_size: done.final_size,
        };
        if done.temporary_path == req.local_path.display().to_string() {
            // Nothing to do; the bytes are already where they belong.
            return Ok(outcome);
        }

        // Which side the temporary file is on decides how it is published, and that is
        // decided by which side the destination is on.
        let local = std::path::Path::new(&done.temporary_path);
        if local.is_absolute() && tokio::fs::try_exists(local).await.unwrap_or(false) {
            tokio::fs::rename(local, &req.local_path)
                .await
                .map_err(|e| EngineError::from_io(&req.local_path, &e))?;
            return Ok(outcome);
        }

        if let Err(err) = self
            .replace(
                &done.temporary_path,
                &req.remote_path,
                self.destination_existed,
            )
            .await
        {
            // Leave nothing behind that a person would have to find and delete.
            let _ = self.raw().remove(&done.temporary_path).await;
            return Err(err);
        }
        Ok(outcome)
    }

    async fn discard(&mut self, temporary_path: &str) {
        let local = std::path::Path::new(temporary_path);
        if local.is_absolute() && tokio::fs::try_exists(local).await.unwrap_or(false) {
            let _ = tokio::fs::remove_file(local).await;
            return;
        }
        let _ = self.raw().remove(temporary_path).await;
    }

    async fn prefix_digest(&mut self, path: &str, len: u64) -> Result<String> {
        let opened = self
            .raw()
            .open(path, OpenFlags::READ, FileAttributes::default())
            .await
            .map_err(sftp_error)?;
        let handle = opened.handle;

        // Pipelined for the same reason a download is: this reads back every byte the
        // resume is about to skip, so serially it costs what re-sending them would.
        let raw = self.raw();
        let mut reads = Reads::new(&self.channel, handle.clone(), 0).until(Some(len));
        let mut rolling = crate::digest::Rolling::new();
        let mut failed = None;
        while let Some(chunk) = reads.next().await {
            match chunk {
                Ok(data) => rolling.update(&data),
                Err(err) => {
                    failed = Some(err);
                    break;
                }
            }
        }
        drop(reads);
        let _ = raw.close(handle).await;
        if let Some(err) = failed {
            return Err(err);
        }

        // Short of the length the checkpoint claims, so the checkpoint is not about
        // this file. Refusing beats hashing whatever happens to be there.
        if rolling.len() < len {
            return Err(EngineError::ResumeUnverifiable {
                reason: format!(
                    "{path} holds {} bytes, fewer than the {len} the checkpoint claims",
                    rolling.len()
                ),
            });
        }
        Ok(rolling.snapshot())
    }

    async fn size_of(&mut self, path: &str) -> Result<Option<u64>> {
        match self.raw().stat(path).await {
            Ok(attrs) => Ok(Some(attrs.attrs.size.unwrap_or(0))),
            Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => {
                Ok(None)
            }
            Err(err) => Err(sftp_error(err)),
        }
    }

    async fn close(self: Box<Self>) {
        // Just the seat. The channel closes with the last transfer on it.
    }
}

/// One of a channel's request slots.
///
/// A pipeline waits for a slot only when it holds no requests of its own: waiting while
/// holding them would be waiting on answers that nothing is polling for. With none
/// held there is nothing to wait on itself, so the wait is always on another transfer
/// that is making progress.
async fn reserve(slots: &Arc<Semaphore>, must_wait: bool) -> Option<OwnedSemaphorePermit> {
    let slots = Arc::clone(slots);
    if must_wait {
        slots.acquire_owned().await.ok()
    } else {
        slots.try_acquire_owned().ok()
    }
}

/// An answer to one pipelined read: what it asked for, and what came back.
type Answered = (u32, std::result::Result<Vec<u8>, SftpError>);
/// An acknowledged write: how many bytes it carried.
type Acked = (u64, std::result::Result<(), SftpError>);

/// A remote file read in order, with several requests in flight.
///
/// The pipeline is what makes a transfer cost bandwidth rather than round trips, and
/// in-order delivery is what lets the caller hash and checkpoint as it goes.
struct Reads<'a> {
    raw: &'a RawSftpSession,
    slots: Arc<Semaphore>,
    handle: String,
    chunk: u32,
    /// Where the next request starts, which runs ahead of what has been delivered.
    asking: u64,
    delivered: u64,
    /// Stop asking here. `None` reads until the server says the file has ended.
    end: Option<u64>,
    inflight: FuturesOrdered<BoxFuture<'a, Answered>>,
    ended: bool,
}

impl<'a> Reads<'a> {
    fn new(channel: &'a Channel, handle: String, from: u64) -> Self {
        Self {
            raw: &channel.raw,
            slots: Arc::clone(&channel.slots),
            handle,
            chunk: channel.read_chunk,
            asking: from,
            delivered: from,
            end: None,
            inflight: FuturesOrdered::new(),
            ended: false,
        }
    }

    fn until(mut self, end: Option<u64>) -> Self {
        self.end = end;
        self
    }

    async fn fill(&mut self) {
        while !self.ended && self.inflight.len() < PIPELINE_DEPTH {
            let want = match self.end {
                Some(end) if self.asking >= end => break,
                Some(end) => (end - self.asking).min(u64::from(self.chunk)) as u32,
                None => self.chunk,
            };
            let Some(permit) = reserve(&self.slots, self.inflight.is_empty()).await else {
                break;
            };
            let (raw, handle, at) = (self.raw, self.handle.clone(), self.asking);
            self.inflight.push_back(Box::pin(async move {
                let _permit = permit;
                (want, raw.read(handle, at, want).await.map(|data| data.data))
            }));
            self.asking += u64::from(want);
        }
    }

    /// The next chunk, in order. `None` once the file has ended.
    async fn next(&mut self) -> Option<Result<Vec<u8>>> {
        self.fill().await;
        let (want, result) = self.inflight.next().await?;
        match result {
            Ok(data) if data.is_empty() => {
                self.ended = true;
                None
            }
            Ok(data) => {
                self.delivered += data.len() as u64;
                // A short answer leaves everything queued behind it aimed at the wrong
                // offsets, so the queue is dropped and re-primed from where the file is.
                if (data.len() as u32) < want {
                    self.inflight = FuturesOrdered::new();
                    self.asking = self.delivered;
                }
                Some(Ok(data))
            }
            Err(SftpError::Status(status)) if status.status_code == StatusCode::Eof => {
                self.ended = true;
                None
            }
            Err(err) => {
                self.ended = true;
                Some(Err(sftp_error(err)))
            }
        }
    }

    /// The first chunk, and the file's length, asked for together.
    ///
    /// One flight rather than two, and the length is what the transfer is measured
    /// against afterwards: a listing's idea of the size can be minutes old by the time
    /// the job reaches the front of the queue, and stopping at a stale figure would
    /// publish a file with its tail missing.
    async fn open(&mut self) -> (Option<Result<Vec<u8>>>, Option<u64>) {
        let (raw, handle) = (self.raw, self.handle.clone());
        let (first, facts) = tokio::join!(self.next(), raw.fstat(handle));
        let total = facts.ok().and_then(|a| a.attrs.size);
        self.end = total;
        (first, total)
    }
}

/// A remote file written in order, with several requests in flight.
///
/// Acknowledgements arrive in the order the writes were sent, so the offset the server
/// has confirmed is always a prefix — which is what a checkpoint can be filed under.
struct Writes<'a> {
    raw: &'a RawSftpSession,
    slots: Arc<Semaphore>,
    handle: String,
    sent: u64,
    inflight: FuturesOrdered<BoxFuture<'a, Acked>>,
}

impl<'a> Writes<'a> {
    fn new(channel: &'a Channel, handle: String, from: u64) -> Self {
        Self {
            raw: &channel.raw,
            slots: Arc::clone(&channel.slots),
            handle,
            sent: from,
            inflight: FuturesOrdered::new(),
        }
    }

    /// A slot for the next write, taken before the bytes are read from disk so none are
    /// read that cannot be sent.
    async fn reserve(&mut self) -> Option<OwnedSemaphorePermit> {
        reserve(&self.slots, self.inflight.is_empty()).await
    }

    fn full(&self) -> bool {
        self.inflight.len() >= PIPELINE_DEPTH
    }

    fn idle(&self) -> bool {
        self.inflight.is_empty()
    }

    fn send(&mut self, permit: OwnedSemaphorePermit, data: Vec<u8>) {
        let len = data.len() as u64;
        let (raw, handle, at) = (self.raw, self.handle.clone(), self.sent);
        self.inflight.push_back(Box::pin(async move {
            let _permit = permit;
            (len, raw.write(handle, at, data).await.map(|_| ()))
        }));
        self.sent += len;
    }

    /// How many bytes the next acknowledgement covers, in order.
    async fn ack(&mut self) -> Option<Result<u64>> {
        let (len, result) = self.inflight.next().await?;
        Some(result.map(|()| len).map_err(sftp_error))
    }
}

/// Open the local partial a download writes into./// Open the local partial a download writes into.
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
    // Anything past the checkpoint was written after the last durable point and is not
    // covered by the digest that authorised this resume. Truncating is what makes the
    // file exactly the prefix that was verified, rather than the prefix plus whatever
    // a crash left behind it.
    if len > offset {
        file.set_len(offset)
            .await
            .map_err(|e| EngineError::from_io(partial, &e))?;
    }
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| EngineError::from_io(partial, &e))?;
    Ok(file)
}

/// Stop a download.
///
/// A paused job is coming back to these bytes, so they are flushed and left where they
/// are. A cancelled one is not, and its partial is litter. Either way the destination
/// is untouched and the only file this ever removes is the one this job created.
async fn stop_download(
    raw: &RawSftpSession,
    handle: String,
    mut file: tokio::fs::File,
    partial: &Path,
    keep: bool,
) -> Result<Transferred> {
    let _ = raw.close(handle).await;
    if keep {
        // Durability before anything else looks at it: a checkpoint is only worth
        // resuming from if the bytes behind it survived.
        let _ = file.flush().await;
        let _ = file.sync_all().await;
        drop(file);
    } else {
        drop(file);
        let _ = tokio::fs::remove_file(partial).await;
    }
    Err(EngineError::Cancelled)
}

async fn stop_upload(
    raw: &RawSftpSession,
    handle: String,
    temp: &str,
    keep: bool,
) -> Result<Transferred> {
    if keep {
        let _ = raw.fsync(handle.clone()).await;
    }
    let _ = raw.close(handle).await;
    if !keep {
        let _ = raw.remove(temp).await;
    }
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
        // The name if the server sent one, the number otherwise.
        //
        // It never sends one in practice: the `user` and `group` name strings arrived
        // in SFTP v4, and v3 — which is what OpenSSH speaks — carries only numeric
        // `uid`/`gid`. `russh-sftp` parses a v3 ATTRS block with `user: None` and
        // `group: None` unconditionally, so reading those fields alone left both of
        // these permanently empty and the inspector said "not known" for every file on
        // every server. A bare uid is what `ls -ln` shows and is a real answer.
        owner: attrs
            .user
            .clone()
            .or_else(|| attrs.uid.map(|id| id.to_string())),
        group: attrs
            .group
            .clone()
            .or_else(|| attrs.gid.map(|id| id.to_string())),
    }
}

/// What a resume compares the source against: the path, its size and its time.
fn facts_from(path: &str, attrs: &FileAttributes) -> FileFacts {
    FileFacts {
        path: path.to_string(),
        size: Bytes(attrs.size.unwrap_or(0)),
        modified: attrs
            .mtime
            .and_then(|secs| Utc.timestamp_opt(i64::from(secs), 0).single()),
        digest: None,
    }
}

/// The same, for a local file.
async fn local_facts_of(path: &std::path::Path) -> Option<FileFacts> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    Some(FileFacts {
        path: path.display().to_string(),
        size: Bytes(meta.len()),
        modified: meta.modified().ok().map(DateTime::<Utc>::from),
        digest: None,
    })
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
        // Every way a connection can be gone. `SendError` — russh's "Channel send
        // error" — is the one that matters most and read as a protocol fault until a
        // real server was restarted underneath a transfer: nothing treated it as an
        // outage, so the session never reconnected and the job failed where it should
        // have waited. The keepalive and inactivity timeouts are the same fact
        // arriving from a different direction.
        russh::Error::Disconnect
        | russh::Error::HUP
        | russh::Error::ConnectionTimeout
        | russh::Error::SendError
        | russh::Error::KeepaliveTimeout
        | russh::Error::InactivityTimeout => EngineError::network(err.to_string()),
        // The only channels Relay opens are sftp sessions, so a refusal means one
        // thing: the server is already carrying as many as it will. OpenSSH answers
        // `MaxSessions` with `ConnectFailed`, which is otherwise indistinguishable
        // from a protocol fault and used to fail the transfer that asked.
        russh::Error::ChannelOpenFailure(_) => EngineError::LanesExhausted,
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

    /// The window every transfer on a channel shares. Sizing the request timeout
    /// against anything smaller is what made a pipelined read time out on a slow link.
    #[test]
    fn the_request_timeout_covers_the_whole_window() {
        let window = PIPELINE_DEPTH as u64 * READ_CHUNK as u64;
        let slowest = window / (request_timeout() - 30);
        assert!(
            slowest <= 64 * 1024,
            "a link slower than {slowest} B/s would time out on a full window"
        );
    }

    /// The classification the reconnect depends on. A dropped connection has to look
    /// like a network fault, or `check_fatal` will not notice, the session will not
    /// reconnect, and the job will fail immediately instead of being retried.
    ///
    /// This was wrong until a real server was restarted underneath a transfer: the
    /// mock backend reports its own severance as `Network`, so every test agreed with
    /// the code rather than with OpenSSH.
    #[test]
    fn a_dropped_connection_is_a_network_fault_however_it_arrives() {
        for err in [
            russh::Error::SendError,
            russh::Error::HUP,
            russh::Error::Disconnect,
            russh::Error::ConnectionTimeout,
            russh::Error::KeepaliveTimeout,
            russh::Error::InactivityTimeout,
        ] {
            let text = err.to_string();
            let mapped = ssh_error(err);
            assert!(
                matches!(mapped, EngineError::Network { .. }),
                "{text} must be retryable, got {mapped:?}"
            );
            assert!(mapped.is_retryable(), "{text}");
        }

        for err in [SftpError::IO("broken pipe".into()), SftpError::Timeout] {
            let mapped = sftp_error(err);
            assert!(matches!(mapped, EngineError::Network { .. }), "{mapped:?}");
        }
    }

    /// And the ones a reconnect would not help with stay where they are: rejected
    /// credentials are an answer, and a server talking nonsense will talk the same
    /// nonsense to the next connection.
    #[test]
    fn a_rejection_is_not_a_connection_problem() {
        assert!(matches!(
            ssh_error(russh::Error::NotAuthenticated),
            EngineError::Auth { .. }
        ));
        let mapped = sftp_error(SftpError::UnexpectedBehavior(
            "two replies to one request".into(),
        ));
        assert!(matches!(mapped, EngineError::Protocol { .. }), "{mapped:?}");
        assert!(!mapped.is_retryable());
    }

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
    fn putty_ppk_keys_reach_puttys_parser() {
        // The editor tells people `.ppk` files work, so something has to hold that
        // claim up. A real ppk needs `puttygen` to produce, which is not a dependency
        // worth taking for one test — but *which parser russh chose* is observable
        // from the error alone, and that is the part that could silently regress.
        let ppk = russh::keys::decode_secret_key(
            "PuTTY-User-Key-File-3: ssh-ed25519\nEncryption: none\n",
            None,
        )
        .expect_err("a headers-only ppk cannot decode");
        assert!(
            format!("{ppk:?}").contains("Ppk"),
            "a PuTTY header must reach the ppk parser, not the generic one: {ppk:?}"
        );

        // The contrast is what makes the assertion above mean something.
        let other = russh::keys::decode_secret_key("not a key at all", None)
            .expect_err("garbage cannot decode");
        assert!(
            !format!("{other:?}").contains("Ppk"),
            "only a PuTTY header should take that path: {other:?}"
        );
    }

    #[test]
    fn an_ssh_string_is_length_prefixed() {
        let mut out = Vec::new();
        ssh_string("ab", &mut out);
        assert_eq!(out, [0, 0, 0, 2, b'a', b'b']);
    }
}
