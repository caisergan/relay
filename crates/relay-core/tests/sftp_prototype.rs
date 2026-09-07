//! Phase 0 §0.6 — the SFTP compatibility prototypes, against a real OpenSSH server.
//!
//! These do **not** test Relay's engine. They test `russh` and `russh-sftp`, before
//! phase 1 commits to them, and they answer questions the in-memory backend cannot:
//! does the auth ladder work against real sshd, can two SFTP channels transfer while
//! a third browses, is cancellation actually bounded, and does an offset-addressed
//! resume produce the same bytes the server has.
//!
//! Run them:
//!
//! ```sh
//! ./scripts/sftp-fixture.sh up > target/sftp-fixture/env.sh
//! set -a; . target/sftp-fixture/env.sh; set +a
//! cargo test -p relay-core --features integration --test sftp_prototype -- --test-threads=1
//! ```
//!
//! Results belong in `docs/adr/004-protocol-compatibility.md`.
#![cfg(feature = "integration")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use russh::client::{self, AuthResult, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKey, load_secret_key};
use russh::{ChannelId, Disconnect};
use russh_sftp::client::{RawSftpSession, SftpSession};
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------- fixture

fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| {
        panic!(
            "{key} is unset. Start the fixture first:\n  \
             ./scripts/sftp-fixture.sh up > target/sftp-fixture/env.sh\n  \
             set -a; . target/sftp-fixture/env.sh; set +a"
        )
    })
}

fn address() -> (String, u16) {
    (
        env("RELAY_SFTP_HOST"),
        env("RELAY_SFTP_PORT").parse().expect("port"),
    )
}

fn user() -> String {
    env("RELAY_SFTP_USER")
}

/// Paths inside the chroot. The fixture mounts the host directory at `upload`.
fn remote(path: &str) -> String {
    format!("/upload/{path}")
}

fn host_file(path: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env("RELAY_SFTP_DATA")).join(path)
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn fixture(command: &str) {
    let status = std::process::Command::new("./scripts/sftp-fixture.sh")
        .arg(command)
        .current_dir(env!("CARGO_MANIFEST_DIR").to_owned() + "/../..")
        .status()
        .expect("running the fixture script");
    assert!(status.success(), "fixture {command} failed");
}

// ---------------------------------------------------------------- handler

/// Records the host key it was shown and answers according to a policy, which is the
/// shape phase 1's trust store needs: the decision happens inside the handshake.
struct TrustHandler {
    accept: bool,
    /// When set, only this fingerprint is accepted — the pinned-key case.
    pinned: Option<String>,
    seen: Arc<std::sync::Mutex<Option<String>>>,
}

impl TrustHandler {
    fn accepting() -> (Self, Arc<std::sync::Mutex<Option<String>>>) {
        let seen = Arc::new(std::sync::Mutex::new(None));
        (
            Self {
                accept: true,
                pinned: None,
                seen: Arc::clone(&seen),
            },
            seen,
        )
    }
}

fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

impl client::Handler for TrustHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let fp = match server_public_key {
            russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => fingerprint(key),
            russh::keys::PublicKeyOrCertificate::Certificate(cert) => {
                format!("cert:{}", cert.key_id())
            }
        };
        *self.seen.lock().unwrap() = Some(fp.clone());
        Ok(match &self.pinned {
            Some(pinned) => self.accept && *pinned == fp,
            None => self.accept,
        })
    }

    async fn channel_close(
        &mut self,
        _: ChannelId,
        _: &mut client::Session,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

async fn connect(handler: TrustHandler) -> Result<Handle<TrustHandler>, russh::Error> {
    let (host, port) = address();
    let config = Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(30)),
        ..client::Config::default()
    });
    client::connect(config, (host, port), handler).await
}

async fn connect_accepting() -> (Handle<TrustHandler>, String) {
    let (handler, seen) = TrustHandler::accepting();
    let session = connect(handler).await.expect("tcp + handshake");
    let fp = seen
        .lock()
        .unwrap()
        .clone()
        .expect("a host key was presented");
    (session, fp)
}

async fn authed_with_key() -> Handle<TrustHandler> {
    let (mut session, _) = connect_accepting().await;
    let key = load_secret_key(env("RELAY_SFTP_KEY"), None).expect("loading the test key");
    let result = session
        .authenticate_publickey(user(), PrivateKeyWithHashAlg::new(Arc::new(key), None))
        .await
        .expect("auth round trip");
    assert!(result.success(), "key auth should succeed: {result:?}");
    session
}

async fn sftp(session: &Handle<TrustHandler>) -> SftpSession {
    let channel = session.channel_open_session().await.expect("channel");
    channel
        .request_subsystem(true, "sftp")
        .await
        .expect("sftp subsystem");
    SftpSession::new(channel.into_stream())
        .await
        .expect("sftp session")
}

async fn raw_sftp(session: &Handle<TrustHandler>) -> RawSftpSession {
    let channel = session.channel_open_session().await.expect("channel");
    channel
        .request_subsystem(true, "sftp")
        .await
        .expect("sftp subsystem");
    let raw = RawSftpSession::new(channel.into_stream());
    raw.init().await.expect("sftp init");
    raw
}

// ---------------------------------------------------------------- gate: authentication

#[tokio::test]
async fn password_auth_succeeds_and_the_wrong_password_fails() {
    let (mut session, _) = connect_accepting().await;
    let result = session
        .authenticate_password(user(), env("RELAY_SFTP_PASSWORD"))
        .await
        .expect("auth round trip");
    assert!(result.success(), "password auth should succeed: {result:?}");
    session
        .disconnect(Disconnect::ByApplication, "", "en")
        .await
        .ok();

    let (mut session, _) = connect_accepting().await;
    let result = session
        .authenticate_password(user(), "not-the-password")
        .await
        .expect("auth round trip");
    assert!(
        matches!(result, AuthResult::Failure { .. }),
        "a wrong password must fail, not succeed quietly: {result:?}"
    );
}

#[tokio::test]
async fn public_key_auth_works_for_ed25519_and_rsa() {
    for (label, path, hash) in [
        ("ed25519", env("RELAY_SFTP_KEY"), None),
        ("rsa", env("RELAY_SFTP_KEY_RSA"), Some(HashAlg::Sha256)),
    ] {
        let (mut session, _) = connect_accepting().await;
        let key = load_secret_key(&path, None).unwrap_or_else(|e| panic!("{label}: {e}"));
        let result = session
            .authenticate_publickey(user(), PrivateKeyWithHashAlg::new(Arc::new(key), hash))
            .await
            .expect("auth round trip");
        assert!(
            result.success(),
            "{label} key auth should succeed: {result:?}"
        );
    }
}

#[tokio::test]
async fn an_encrypted_key_needs_its_passphrase() {
    let path = env("RELAY_SFTP_KEY_LOCKED");

    // Without the passphrase the load fails — this is the error phase 1 turns into a
    // passphrase prompt, so it must be distinguishable rather than generic.
    let err = load_secret_key(&path, None).expect_err("an encrypted key must not load bare");
    println!("locked key without passphrase: {err:?}");

    let key = load_secret_key(&path, Some(&env("RELAY_SFTP_KEY_PASSPHRASE")))
        .expect("loading with the passphrase");
    let (mut session, _) = connect_accepting().await;
    let result = session
        .authenticate_publickey(user(), PrivateKeyWithHashAlg::new(Arc::new(key), None))
        .await
        .expect("auth round trip");
    assert!(
        result.success(),
        "encrypted key auth should succeed: {result:?}"
    );
}

/// The macOS path exactly: `SSH_AUTH_SOCK` pointing at a unix socket. The Windows
/// named-pipe agent cannot be exercised here and stays open in ADR 004.
#[tokio::test]
async fn agent_auth_over_ssh_auth_sock() {
    // A drop guard rather than a kill at the end: an assertion below would otherwise
    // leave an ssh-agent running after the test.
    struct Agent(std::process::Child);
    impl Drop for Agent {
        fn drop(&mut self) {
            self.0.kill().ok();
            self.0.wait().ok();
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("agent.sock");

    let _agent = Agent(
        std::process::Command::new("ssh-agent")
            .args(["-D", "-a"])
            .arg(&sock)
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("ssh-agent must be installed to run this prototype"),
    );

    // Wait for the socket rather than sleeping blindly.
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    assert!(sock.exists(), "ssh-agent did not create its socket");

    let added = std::process::Command::new("ssh-add")
        .env("SSH_AUTH_SOCK", &sock)
        .arg(env("RELAY_SFTP_KEY"))
        .status()
        .expect("ssh-add");
    assert!(added.success(), "ssh-add failed");

    let mut client = russh::keys::agent::client::AgentClient::connect_uds(&sock)
        .await
        .expect("connecting to the agent socket");
    let identities = client.request_identities().await.expect("agent identities");
    assert!(!identities.is_empty(), "the agent should hold one identity");

    let key = match &identities[0] {
        russh::keys::agent::AgentIdentity::PublicKey { key, .. } => key.clone(),
        other => panic!("unexpected identity {other:?}"),
    };

    let (mut session, _) = connect_accepting().await;
    let result = session
        .authenticate_publickey_with(user(), key, None, &mut client)
        .await
        .expect("agent auth round trip");
    assert!(result.success(), "agent auth should succeed: {result:?}");
}

#[tokio::test]
async fn keyboard_interactive_is_reported_honestly() {
    let (mut session, _) = connect_accepting().await;
    let response = session
        .authenticate_keyboard_interactive_start(user(), None)
        .await
        .expect("kbd-interactive round trip");

    // The fixture's sshd may not offer it. Phase 1 falls back to it after a password
    // failure, so what matters here is that the outcome is legible, not that it works.
    match response {
        KeyboardInteractiveAuthResponse::Success => {
            println!("kbd-interactive: accepted with no prompt")
        }
        KeyboardInteractiveAuthResponse::Failure {
            remaining_methods, ..
        } => {
            println!("kbd-interactive: not offered; server suggests {remaining_methods:?}");
        }
        KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
            let answers = prompts.iter().map(|_| env("RELAY_SFTP_PASSWORD")).collect();
            let next = session
                .authenticate_keyboard_interactive_respond(answers)
                .await
                .expect("kbd-interactive response");
            println!(
                "kbd-interactive after answering {} prompt(s): {next:?}",
                prompts.len()
            );
        }
    }
}

// ---------------------------------------------------------------- gate: host keys

#[tokio::test]
async fn a_rejected_host_key_stops_the_connection_before_authentication() {
    let seen = Arc::new(std::sync::Mutex::new(None));
    let handler = TrustHandler {
        accept: false,
        pinned: None,
        seen: Arc::clone(&seen),
    };

    let outcome = connect(handler).await;
    assert!(
        outcome.is_err(),
        "refusing the host key must fail the handshake, not continue to auth"
    );
    assert!(
        seen.lock().unwrap().is_some(),
        "the handler was shown a key first"
    );
}

#[tokio::test]
async fn a_changed_host_key_is_visible_to_the_pinning_check() {
    let expected = env("RELAY_SFTP_HOSTKEY_FP");
    let (_, actual) = connect_accepting().await;
    assert_eq!(
        actual, expected,
        "the fixture's advertised fingerprint must match"
    );

    // Present a different host key on the same host:port — the MITM shape.
    fixture("rotate");
    let rotated = env("RELAY_SFTP_HOSTKEY_FP_ROTATED");

    let seen = Arc::new(std::sync::Mutex::new(None));
    let pinned = TrustHandler {
        accept: true,
        pinned: Some(expected.clone()),
        seen: Arc::clone(&seen),
    };
    let outcome = connect(pinned).await;
    let observed = seen.lock().unwrap().clone();

    fixture("restore");

    assert_eq!(
        observed.as_deref(),
        Some(rotated.as_str()),
        "we saw the new key"
    );
    assert!(
        outcome.is_err(),
        "a pinned mismatch must refuse the connection"
    );
    assert_ne!(rotated, expected);
}

// ---------------------------------------------------------------- gate: concurrency

#[tokio::test]
async fn two_channels_transfer_while_a_third_browses() {
    let session = Arc::new(authed_with_key().await);
    let listings = Arc::new(AtomicUsize::new(0));

    let browse = {
        let session = Arc::clone(&session);
        let listings = Arc::clone(&listings);
        tokio::spawn(async move {
            let sftp = sftp(&session).await;
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut worst = Duration::ZERO;
            while Instant::now() < deadline && listings.load(Ordering::SeqCst) < 40 {
                let started = Instant::now();
                let entries = sftp.read_dir(remote("assets")).await.expect("listing");
                worst = worst.max(started.elapsed());
                assert!(entries.count() >= 4);
                listings.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            worst
        })
    };

    let mut transfers = Vec::new();
    for _ in 0..2 {
        let session = Arc::clone(&session);
        transfers.push(tokio::spawn(async move {
            let raw = raw_sftp(&session).await;
            let started = Instant::now();
            let bytes = read_whole(&raw, &remote("assets/thirty-two-mib.bin"), 128 * 1024, None)
                .await
                .expect("download");
            (bytes.len(), started.elapsed())
        }));
    }

    let worst_listing = browse.await.unwrap();
    let mut total = 0usize;
    for handle in transfers {
        let (len, elapsed) = handle.await.unwrap();
        total += len;
        let mib_s = len as f64 / 1_048_576.0 / elapsed.as_secs_f64();
        println!("lane: {len} bytes in {elapsed:?} ({mib_s:.1} MiB/s)");
    }

    println!(
        "browsing stayed responsive: {} listings, worst {worst_listing:?}",
        listings.load(Ordering::SeqCst)
    );
    assert_eq!(total, 2 * 32 * 1024 * 1024);
    assert!(
        listings.load(Ordering::SeqCst) >= 10,
        "browsing should not have been starved by the transfers"
    );
    // The number that matters for phase 1's actor: a listing must not queue behind a
    // whole transfer. Generous, because this is CI-shaped hardware, not a promise.
    assert!(
        worst_listing < Duration::from_secs(5),
        "a listing took {worst_listing:?}"
    );
}

#[tokio::test]
async fn cancellation_stops_a_transfer_within_a_bounded_time() {
    let session = Arc::new(authed_with_key().await);
    let raw = raw_sftp(&session).await;
    let cancel = CancellationToken::new();

    let task = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            read_whole(
                &raw,
                &remote("assets/thirty-two-mib.bin"),
                32 * 1024,
                Some(cancel),
            )
            .await
        })
    };

    tokio::time::sleep(Duration::from_millis(300)).await;
    let cancelled_at = Instant::now();
    cancel.cancel();

    let outcome = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the transfer must observe cancellation, not hang")
        .unwrap();
    let stopped_in = cancelled_at.elapsed();

    println!("cancellation observed in {stopped_in:?}");
    assert!(
        outcome.is_err(),
        "a cancelled read should report cancellation"
    );
    assert!(
        stopped_in < Duration::from_secs(2),
        "cancellation took {stopped_in:?}"
    );

    // The session itself must still be usable: cancelling one lane cannot poison the
    // connection, or phase 2's retry has nothing to retry on.
    let sftp = sftp(&session).await;
    assert!(
        sftp.read_dir(remote("assets")).await.is_ok(),
        "the session survived"
    );
}

// ---------------------------------------------------------------- gate: interrupted transfer

#[tokio::test]
async fn an_interrupted_download_resumes_to_the_same_bytes() {
    let path = remote("assets/one-mib.bin");
    let expected = std::fs::read(host_file("assets/one-mib.bin")).expect("host copy");

    let session = Arc::new(authed_with_key().await);
    let raw = raw_sftp(&session).await;

    // Interrupt at a deliberately unaligned offset: real interruptions do not respect
    // chunk boundaries.
    let cut = 393_216 + 111;
    let head = read_range(&raw, &path, 0, cut).await.expect("first half");
    assert_eq!(head.len(), cut);

    // A fresh channel, as a reconnect would give us.
    let raw = raw_sftp(&session).await;
    let tail = read_range(&raw, &path, cut as u64, expected.len() - cut)
        .await
        .expect("resumed half");

    let mut joined = head;
    joined.extend_from_slice(&tail);
    assert_eq!(joined.len(), expected.len());
    assert_eq!(
        sha256(&joined),
        sha256(&expected),
        "a resumed download must hash identically to the source"
    );
}

#[tokio::test]
async fn a_same_size_source_change_is_detectable_before_resuming() {
    let name = "assets/mutable.bin";
    let host = host_file(name);
    std::fs::write(&host, vec![b'a'; 262_144]).expect("seed");

    let session = Arc::new(authed_with_key().await);
    let raw = raw_sftp(&session).await;
    let path = remote(name);

    let before = raw.stat(&path).await.expect("stat").attrs;
    let prefix = read_range(&raw, &path, 0, 65_536).await.expect("prefix");
    let prefix_digest = sha256(&prefix);

    // Same length, different content, and — crucially — this can happen inside one
    // mtime granularity, which is exactly why size and mtime are not enough.
    std::fs::write(&host, vec![b'b'; 262_144]).expect("mutate");

    let after = raw.stat(&path).await.expect("stat").attrs;
    let prefix_now = read_range(&raw, &path, 0, 65_536)
        .await
        .expect("prefix again");

    println!(
        "size before/after: {:?}/{:?}, mtime before/after: {:?}/{:?}",
        before.size, after.size, before.mtime, after.mtime
    );
    assert_eq!(
        before.size, after.size,
        "the fixture kept the size identical"
    );
    assert_ne!(
        prefix_digest,
        sha256(&prefix_now),
        "only a content digest catches this — which is why ADR 005 requires one"
    );

    std::fs::remove_file(&host).ok();
}

#[tokio::test]
async fn chunk_size_changes_throughput_enough_to_measure() {
    let session = Arc::new(authed_with_key().await);
    let path = remote("assets/thirty-two-mib.bin");

    for chunk in [32 * 1024usize, 64 * 1024, 128 * 1024, 256 * 1024] {
        let raw = raw_sftp(&session).await;
        let started = Instant::now();
        let bytes = read_whole(&raw, &path, chunk, None)
            .await
            .expect("download");
        let elapsed = started.elapsed();
        println!(
            "chunk {:>4} KiB: {:.1} MiB/s ({elapsed:?})",
            chunk / 1024,
            bytes.len() as f64 / 1_048_576.0 / elapsed.as_secs_f64()
        );
        assert_eq!(bytes.len(), 32 * 1024 * 1024);
    }
}

#[tokio::test]
async fn an_upload_lands_byte_for_byte_and_can_be_finalised_by_rename() {
    let session = Arc::new(authed_with_key().await);
    let raw = raw_sftp(&session).await;

    let payload: Vec<u8> = (0..512 * 1024).map(|i| (i % 251) as u8).collect();
    let temp = remote("assets/.upload.relaypart");
    let final_path = remote("assets/uploaded.bin");

    let handle = raw
        .open(
            &temp,
            OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
            FileAttributes::default(),
        )
        .await
        .expect("open for write")
        .handle;

    for (index, chunk) in payload.chunks(128 * 1024).enumerate() {
        raw.write(handle.clone(), (index * 128 * 1024) as u64, chunk.to_vec())
            .await
            .expect("write chunk");
    }
    raw.fsync(handle.clone()).await.ok();
    raw.close(handle).await.expect("close");

    // Atomic finalisation: the destination never exists half-written.
    raw.rename(&temp, &final_path).await.expect("rename over");

    let landed = std::fs::read(host_file("assets/uploaded.bin")).expect("host copy");
    assert_eq!(sha256(&landed), sha256(&payload));

    std::fs::remove_file(host_file("assets/uploaded.bin")).ok();
}

// ---------------------------------------------------------------- helpers

async fn read_range(
    raw: &RawSftpSession,
    path: &str,
    offset: u64,
    len: usize,
) -> Result<Vec<u8>, russh_sftp::client::error::Error> {
    let handle = raw
        .open(path, OpenFlags::READ, FileAttributes::default())
        .await?
        .handle;
    let mut out = Vec::with_capacity(len);
    let mut at = offset;
    while out.len() < len {
        let want = (len - out.len()).min(32 * 1024) as u32;
        let data = raw.read(handle.clone(), at, want).await?;
        if data.data.is_empty() {
            break;
        }
        at += data.data.len() as u64;
        out.extend_from_slice(&data.data);
    }
    raw.close(handle).await?;
    Ok(out)
}

async fn read_whole(
    raw: &RawSftpSession,
    path: &str,
    chunk: usize,
    cancel: Option<CancellationToken>,
) -> Result<Vec<u8>, russh_sftp::client::error::Error> {
    let handle = raw
        .open(path, OpenFlags::READ, FileAttributes::default())
        .await?
        .handle;
    let mut out = Vec::new();
    let mut at = 0u64;
    loop {
        if let Some(token) = &cancel
            && token.is_cancelled()
        {
            raw.close(handle).await.ok();
            return Err(russh_sftp::client::error::Error::UnexpectedBehavior(
                "cancelled".into(),
            ));
        }
        match raw.read(handle.clone(), at, chunk as u32).await {
            Ok(data) if data.data.is_empty() => break,
            Ok(data) => {
                at += data.data.len() as u64;
                out.extend_from_slice(&data.data);
            }
            // EOF arrives as a status error rather than an empty read.
            Err(russh_sftp::client::error::Error::Status(status))
                if status.status_code == russh_sftp::protocol::StatusCode::Eof =>
            {
                break;
            }
            Err(err) => {
                raw.close(handle).await.ok();
                return Err(err);
            }
        }
    }
    raw.close(handle).await?;
    Ok(out)
}
