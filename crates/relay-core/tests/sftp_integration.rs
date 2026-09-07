//! Phase 1 §1.7: `SftpBackend` against a real OpenSSH server.
//!
//! Phase 0's `sftp_prototype.rs` tested `russh`. This tests *Relay's backend* — the
//! auth ladder, the trust store deciding inside the handshake, the mapping onto
//! `RemoteEntry`, the transfer lanes, and the finalisation rules. The prototypes stay
//! where they are: they answer "can the library do this", and this answers "does our
//! code do it".
//!
//! Run them:
//!
//! ```sh
//! ./scripts/sftp-fixture.sh up > target/sftp-fixture/env.sh
//! set -a; . target/sftp-fixture/env.sh; set +a
//! cargo test -p relay-core --features integration --test sftp_integration -- --test-threads=1
//! ```
#![cfg(feature = "integration")]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use relay_core::error::EngineError;
use relay_core::interact::{Interact, Prompt, PromptReply};
use relay_core::model::{AuthMethod, FileKind, ServerConfig, SessionId};
use relay_core::model::{Direction, FileFacts};
use relay_core::protocol::{CheckpointSink, ProgressSink, Protocol, TransferReq};
use relay_core::queue::ResumeRecord;
use relay_core::secrets::{MemorySecrets, SecretKind};
use relay_core::sftp::SftpBackend;
use relay_core::trust::{TrustDecision, TrustStore};
use relay_core::wire::Bytes;
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

fn remote(path: &str) -> String {
    format!("/upload/{path}")
}

fn host_file(path: &str) -> PathBuf {
    PathBuf::from(env("RELAY_SFTP_DATA")).join(path)
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn config(auth: AuthMethod) -> ServerConfig {
    ServerConfig {
        id: uuid::Uuid::new_v4(),
        name: "fixture".into(),
        host: env("RELAY_SFTP_HOST"),
        port: env("RELAY_SFTP_PORT").parse().expect("port"),
        proto: relay_core::model::Proto::Sftp,
        username: env("RELAY_SFTP_USER"),
        auth,
        color: None,
        group: None,
        bookmarks: Vec::new(),
        initial_remote_path: None,
    }
}

/// Answers host-key prompts according to a fixed policy and counts them, so a test can
/// assert that a *pinned* key produced no question at all.
struct Answering {
    accept: bool,
    remember: bool,
    asked: Arc<AtomicU64>,
}

impl Answering {
    fn accepting() -> (Arc<Self>, Arc<AtomicU64>) {
        let asked = Arc::new(AtomicU64::new(0));
        (
            Arc::new(Self {
                accept: true,
                remember: true,
                asked: Arc::clone(&asked),
            }),
            asked,
        )
    }
}

#[async_trait]
impl Interact for Answering {
    async fn ask(&self, _session: SessionId, prompt: Prompt) -> PromptReply {
        self.asked.fetch_add(1, Ordering::SeqCst);
        match prompt {
            Prompt::HostKey { .. } | Prompt::TlsCert { .. } if self.accept => PromptReply::Accept {
                remember: self.remember,
            },
            _ => PromptReply::Deny,
        }
    }
}

struct Connected {
    backend: SftpBackend,
    asked: Arc<AtomicU64>,
}

/// Connect with the given auth method, accepting the host key.
async fn connect(auth: AuthMethod) -> Connected {
    connect_with(auth, Arc::new(TrustStore::ephemeral())).await
}

async fn connect_with(auth: AuthMethod, trust: Arc<TrustStore>) -> Connected {
    let cfg = config(auth);
    let secrets = MemorySecrets::new();
    secrets.set(cfg.id, SecretKind::Password, env("RELAY_SFTP_PASSWORD"));
    secrets.set(
        cfg.id,
        SecretKind::Passphrase,
        env("RELAY_SFTP_KEY_PASSPHRASE"),
    );

    let (interact, asked) = Answering::accepting();
    let mut backend = SftpBackend::new(uuid::Uuid::new_v4(), Arc::clone(&trust));
    backend
        .connect(&cfg, &secrets, interact as Arc<dyn Interact>)
        .await
        .expect("connecting to the fixture");
    Connected { backend, asked }
}

fn req(
    remote_path: &str,
    local: &std::path::Path,
    cancel: CancellationToken,
) -> (TransferReq, Arc<Progress>) {
    let progress = Arc::new(Progress::default());
    let sink = {
        let progress = Arc::clone(&progress);
        ProgressSink::new(move |bytes| progress.observe(bytes))
    };
    (
        TransferReq {
            job: uuid::Uuid::new_v4(),
            remote_path: remote_path.to_string(),
            local_path: local.to_path_buf(),
            offset: 0,
            prefix: None,
            progress: sink,
            checkpoint: CheckpointSink::noop(),
            cancel,
            keep_partial: Default::default(),
        },
        progress,
    )
}

/// Records that progress only ever moves forward. A bar that goes backwards is a bug
/// the UI cannot paper over.
#[derive(Default)]
struct Progress {
    last: AtomicU64,
    ticks: AtomicU64,
    regressions: AtomicU64,
}

impl Progress {
    fn observe(&self, bytes: u64) {
        let previous = self.last.swap(bytes, Ordering::SeqCst);
        if bytes < previous {
            self.regressions.fetch_add(1, Ordering::SeqCst);
        }
        self.ticks.fetch_add(1, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------- auth

#[tokio::test]
async fn password_authentication_connects_and_reports_the_peer() {
    let mut c = connect(AuthMethod::Password).await;
    let entries = c.backend.list(&remote("assets")).await.expect("listing");
    assert!(!entries.is_empty(), "the fixture's assets are listed");
    c.backend.disconnect().await;
}

#[tokio::test]
async fn key_file_authentication_connects() {
    let mut c = connect(AuthMethod::KeyFile {
        path: PathBuf::from(env("RELAY_SFTP_KEY")),
    })
    .await;
    assert!(c.backend.list(&remote("assets")).await.is_ok());
    c.backend.disconnect().await;
}

#[tokio::test]
async fn an_encrypted_key_uses_the_stored_passphrase_without_prompting() {
    let c = connect(AuthMethod::KeyFile {
        path: PathBuf::from(env("RELAY_SFTP_KEY_LOCKED")),
    })
    .await;
    // One prompt, and it was the host key. The passphrase came from the secret source,
    // which is the whole point of storing it.
    assert_eq!(
        c.asked.load(Ordering::SeqCst),
        1,
        "a stored passphrase must not produce a second sheet"
    );
}

#[tokio::test]
async fn a_wrong_password_is_an_auth_error_and_never_echoes_the_credential() {
    let cfg = config(AuthMethod::Password);
    let secrets = MemorySecrets::new();
    secrets.set(cfg.id, SecretKind::Password, "definitely-not-the-password");

    let (interact, _) = Answering::accepting();
    let mut backend = SftpBackend::new(uuid::Uuid::new_v4(), Arc::new(TrustStore::ephemeral()));
    let err = backend
        .connect(&cfg, &secrets, interact as Arc<dyn Interact>)
        .await
        .expect_err("a wrong password must fail");

    assert!(matches!(err, EngineError::Auth { .. }), "got {err:?}");
    assert!(
        !err.to_string().contains("definitely-not-the-password"),
        "an error must never carry the credential: {err}"
    );
}

// ---------------------------------------------------------------- trust

#[tokio::test]
async fn declining_the_host_key_reports_trust_rejected_not_a_network_fault() {
    let cfg = config(AuthMethod::Password);
    let secrets = MemorySecrets::new();
    secrets.set(cfg.id, SecretKind::Password, env("RELAY_SFTP_PASSWORD"));

    let asked = Arc::new(AtomicU64::new(0));
    let interact = Arc::new(Answering {
        accept: false,
        remember: false,
        asked: Arc::clone(&asked),
    });

    let mut backend = SftpBackend::new(uuid::Uuid::new_v4(), Arc::new(TrustStore::ephemeral()));
    let err = backend
        .connect(&cfg, &secrets, interact as Arc<dyn Interact>)
        .await
        .expect_err("declining must fail the connection");

    assert_eq!(
        asked.load(Ordering::SeqCst),
        1,
        "we were asked exactly once"
    );
    assert!(
        matches!(err, EngineError::TrustRejected { .. }),
        "a refused key is a decision, not a network error: {err:?}"
    );
}

#[tokio::test]
async fn a_pinned_key_connects_without_asking_again() {
    let trust = Arc::new(TrustStore::ephemeral());

    // First contact pins it, because the stub answers `remember: true`.
    let first = connect_with(AuthMethod::Password, Arc::clone(&trust)).await;
    assert_eq!(first.asked.load(Ordering::SeqCst), 1);
    let endpoint = format!("{}:{}", env("RELAY_SFTP_HOST"), env("RELAY_SFTP_PORT"));
    let pinned = trust.get(&endpoint).expect("the key was pinned");
    assert!(pinned.sha256.starts_with("SHA256:"), "{}", pinned.sha256);

    // Second connection: the store answers, so nobody is asked.
    let second = connect_with(AuthMethod::Password, Arc::clone(&trust)).await;
    assert_eq!(
        second.asked.load(Ordering::SeqCst),
        0,
        "a pinned key must not re-open the trust sheet"
    );
    assert_eq!(trust.check(&endpoint, &pinned.sha256), TrustDecision::Known);
}

// ---------------------------------------------------------------- browsing

#[tokio::test]
async fn a_listing_carries_the_metadata_the_pane_renders() {
    let mut c = connect(AuthMethod::Password).await;
    let entries = c.backend.list(&remote("assets")).await.expect("listing");

    let file = entries
        .iter()
        .find(|e| e.name == "one-mib.bin")
        .expect("the fixture's one-mib file");
    assert_eq!(file.kind, FileKind::File);
    assert_eq!(file.size.get(), 1024 * 1024);
    assert!(file.modified.is_some(), "a real mtime came back");
    let perms = file.perms.as_deref().expect("permissions");
    assert_eq!(perms.len(), 9, "rendered as rwxrwxrwx: {perms}");
    assert!(perms.starts_with("rw"), "{perms}");

    // Directories sort before files, which is what makes the two panes comparable.
    let first_file = entries.iter().position(|e| !e.is_dir());
    let last_dir = entries.iter().rposition(|e| e.is_dir());
    if let (Some(first_file), Some(last_dir)) = (first_file, last_dir) {
        assert!(last_dir < first_file, "directories come first");
    }

    // `.` and `..` are the server's business, not the pane's.
    assert!(!entries.iter().any(|e| e.name == "." || e.name == ".."));
}

#[tokio::test]
async fn mkdir_rename_and_delete_round_trip() {
    let mut c = connect(AuthMethod::Password).await;
    let dir = remote("relay-test-dir");
    let renamed = remote("relay-test-dir-renamed");

    // Leave nothing behind from an earlier failed run.
    let _ = c.backend.remove_dir(&dir).await;
    let _ = c.backend.remove_dir(&renamed).await;

    c.backend.mkdir(&dir).await.expect("mkdir");
    assert!(c.backend.stat(&dir).await.expect("stat").is_some());

    c.backend.rename(&dir, &renamed).await.expect("rename");
    assert!(
        c.backend.stat(&dir).await.expect("stat").is_none(),
        "the old name is gone"
    );
    assert!(c.backend.stat(&renamed).await.expect("stat").is_some());

    c.backend.remove_dir(&renamed).await.expect("rmdir");
    assert!(c.backend.stat(&renamed).await.expect("stat").is_none());
}

#[tokio::test]
async fn a_missing_path_is_not_found_rather_than_a_protocol_error() {
    let mut c = connect(AuthMethod::Password).await;
    assert!(
        c.backend
            .stat(&remote("no-such-file"))
            .await
            .expect("stat answers")
            .is_none(),
        "stat reports absence as None, so a caller can act on it"
    );
    let err = c
        .backend
        .list(&remote("no-such-directory"))
        .await
        .expect_err("listing a missing directory fails");
    assert!(matches!(err, EngineError::NotFound { .. }), "got {err:?}");
}

#[tokio::test]
async fn read_file_refuses_to_pull_something_too_large_into_memory() {
    let mut c = connect(AuthMethod::Password).await;
    let small = c
        .backend
        .read_file(&remote("assets/one-mib.bin"), 2 * 1024 * 1024)
        .await
        .expect("a file inside the limit");
    assert_eq!(small.len(), 1024 * 1024);

    let err = c
        .backend
        .read_file(&remote("assets/one-mib.bin"), 64 * 1024)
        .await
        .expect_err("beyond the limit");
    assert!(matches!(err, EngineError::Protocol { .. }), "got {err:?}");
}

// ---------------------------------------------------------------- transfers

#[tokio::test]
async fn a_download_lands_byte_for_byte_with_monotonic_progress() {
    let mut c = connect(AuthMethod::Password).await;
    let mut lane = c.backend.open_lane().await.expect("lane");

    let dir = tempfile::tempdir().expect("tempdir");
    let local = dir.path().join("one-mib.bin");
    let (request, progress) = req(
        &remote("assets/one-mib.bin"),
        &local,
        CancellationToken::new(),
    );

    let outcome = lane.download(request).await.expect("download");
    lane.close().await;

    assert_eq!(outcome.final_size, 1024 * 1024);
    assert_eq!(
        sha256(&std::fs::read(&local).expect("read back")),
        sha256(&std::fs::read(host_file("assets/one-mib.bin")).expect("source")),
    );
    assert!(
        progress.ticks.load(Ordering::SeqCst) > 1,
        "progress reported"
    );
    assert_eq!(
        progress.regressions.load(Ordering::SeqCst),
        0,
        "a progress bar must never move backwards"
    );

    // The partial file is gone, and nothing else was left in the directory.
    let leftovers: Vec<String> = std::fs::read_dir(dir.path())
        .expect("dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.contains("relaypart"))
        .collect();
    assert!(leftovers.is_empty(), "partials remain: {leftovers:?}");
}

#[tokio::test]
async fn cancelling_a_download_is_bounded_and_removes_only_its_own_partial() {
    let mut c = connect(AuthMethod::Password).await;
    let mut lane = c.backend.open_lane().await.expect("lane");

    let dir = tempfile::tempdir().expect("tempdir");
    let local = dir.path().join("thirty-two-mib.bin");
    // A file that already exists at the destination, to prove it is left alone.
    std::fs::write(&local, b"not the server's copy").expect("seed");

    let cancel = CancellationToken::new();
    let (request, _) = req(&remote("assets/thirty-two-mib.bin"), &local, cancel.clone());

    let task = tokio::spawn(async move {
        let outcome = lane.download(request).await;
        lane.close().await;
        outcome
    });

    tokio::time::sleep(Duration::from_millis(300)).await;
    let cancelled_at = Instant::now();
    cancel.cancel();

    let outcome = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("cancellation must be observed, not waited out")
        .expect("no panic");
    let stopped_in = cancelled_at.elapsed();
    println!("cancellation observed in {stopped_in:?}");

    assert!(
        matches!(outcome, Err(EngineError::Cancelled)),
        "{outcome:?}"
    );
    assert!(stopped_in < Duration::from_secs(2), "took {stopped_in:?}");
    assert_eq!(
        std::fs::read(&local).expect("read back"),
        b"not the server's copy",
        "a cancelled download must not touch the destination"
    );
    let leftovers: Vec<String> = std::fs::read_dir(dir.path())
        .expect("dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.contains("relaypart"))
        .collect();
    assert!(leftovers.is_empty(), "partials remain: {leftovers:?}");

    // The session survives one lane being torn down.
    assert!(c.backend.list(&remote("assets")).await.is_ok());
}

#[tokio::test]
async fn an_upload_finalises_atomically_and_can_replace_an_existing_file() {
    let mut c = connect(AuthMethod::Password).await;
    assert!(
        c.backend.capabilities().atomic_rename,
        "OpenSSH offers posix-rename; without it the upload path below is expected to \
         refuse rather than to overwrite"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let local = dir.path().join("payload.bin");
    let payload: Vec<u8> = (0..512 * 1024).map(|i| (i % 251) as u8).collect();
    std::fs::write(&local, &payload).expect("write payload");

    let target = remote("relay-upload.bin");
    let landed = host_file("relay-upload.bin");
    let _ = std::fs::remove_file(&landed);

    // First upload: the destination does not exist.
    let mut lane = c.backend.open_lane().await.expect("lane");
    let (request, progress) = req(&target, &local, CancellationToken::new());
    let outcome = lane.upload(request).await.expect("upload");
    lane.close().await;

    assert_eq!(outcome.final_size, payload.len() as u64);
    assert_eq!(progress.regressions.load(Ordering::SeqCst), 0);
    assert_eq!(
        sha256(&std::fs::read(&landed).expect("landed")),
        sha256(&payload)
    );

    // Second upload: the destination exists, so finalisation needs posix-rename.
    let replacement: Vec<u8> = (0..256 * 1024).map(|i| (i % 97) as u8).collect();
    std::fs::write(&local, &replacement).expect("write replacement");

    let mut lane = c.backend.open_lane().await.expect("lane");
    let (request, _) = req(&target, &local, CancellationToken::new());
    lane.upload(request).await.expect("replacing upload");
    lane.close().await;

    assert_eq!(
        sha256(&std::fs::read(&landed).expect("landed")),
        sha256(&replacement),
        "the destination was replaced with the new content"
    );

    // No temporary file survived either upload.
    let strays: Vec<String> = std::fs::read_dir(host_file(""))
        .expect("data dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.contains("relaypart"))
        .collect();
    assert!(strays.is_empty(), "remote partials remain: {strays:?}");

    let _ = std::fs::remove_file(&landed);
}

#[tokio::test]
async fn two_lanes_transfer_while_the_browse_channel_keeps_listing() {
    let mut c = connect(AuthMethod::Password).await;

    let mut lanes = Vec::new();
    let dir = tempfile::tempdir().expect("tempdir");
    for name in ["a.bin", "b.bin"] {
        let mut lane = c.backend.open_lane().await.expect("lane");
        let local = dir.path().join(name);
        let (request, _) = req(
            &remote("assets/thirty-two-mib.bin"),
            &local,
            CancellationToken::new(),
        );
        lanes.push(tokio::spawn(async move {
            let outcome = lane.download(request).await;
            lane.close().await;
            outcome
        }));
    }

    // The claim the lane design exists to support: the browse channel is not blocked
    // by transfers on the same connection.
    let mut worst = Duration::ZERO;
    for _ in 0..10 {
        let started = Instant::now();
        c.backend.list(&remote("assets")).await.expect("listing");
        worst = worst.max(started.elapsed());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    println!("worst listing while two lanes ran: {worst:?}");
    assert!(worst < Duration::from_secs(5), "a listing took {worst:?}");

    for lane in lanes {
        let outcome = lane.await.expect("no panic").expect("download");
        assert_eq!(outcome.final_size, 32 * 1024 * 1024);
    }
}

/// Phase 2 §2.4, against a real server: stop a transfer, verify what it left, continue
/// from there, and get the file the server has rather than something the right length.
#[tokio::test]
async fn a_paused_download_resumes_from_a_verified_checkpoint() {
    let mut c = connect(AuthMethod::Password).await;
    let source = remote("assets/thirty-two-mib.bin");

    let dir = tempfile::tempdir().expect("tempdir");
    let local = dir.path().join("thirty-two-mib.bin");
    let job = uuid::Uuid::new_v4();

    // ---- the interrupted attempt -------------------------------------------
    let checkpoints: Arc<std::sync::Mutex<Vec<(u64, String)>>> = Arc::default();
    let cancel = CancellationToken::new();
    let request = TransferReq {
        job,
        remote_path: source.clone(),
        local_path: local.clone(),
        offset: 0,
        prefix: None,
        // The pause is triggered by a byte count, not by a clock. A sleep long enough
        // to be sure of passing the first checkpoint on a slow link is long enough to
        // finish the whole file on a fast one — which is what CI's local docker did,
        // leaving the test to "pause" a transfer that had already completed.
        progress: {
            let cancel = cancel.clone();
            ProgressSink::new(move |bytes| {
                if bytes >= 12 * 1024 * 1024 {
                    cancel.cancel();
                }
            })
        },
        checkpoint: {
            let seen = Arc::clone(&checkpoints);
            CheckpointSink::new(move |at, digest| seen.lock().unwrap().push((at, digest)))
        },
        cancel: cancel.clone(),
        // A pause, not a cancellation: the bytes are what the resume will continue.
        keep_partial: Arc::new(std::sync::atomic::AtomicBool::new(true)),
    };

    let mut lane = c.backend.open_lane().await.expect("lane");
    let outcome = tokio::time::timeout(Duration::from_secs(60), lane.download(request))
        .await
        .expect("the pause is observed");
    lane.close().await;
    assert!(
        matches!(outcome, Err(EngineError::Cancelled)),
        "{outcome:?}"
    );

    let recorded = checkpoints.lock().unwrap().clone();
    assert!(
        !recorded.is_empty(),
        "twelve megabytes moved without a checkpoint; there is nothing here to resume \
         from and this test would prove nothing"
    );
    let (at, digest) = recorded.last().cloned().expect("a checkpoint");

    // The partial survived the pause, and is at least as long as the checkpoint.
    let partial = relay_core::protocol::local_partial(&local, job);
    let held = std::fs::metadata(&partial)
        .expect("the partial is still there")
        .len();
    assert!(
        held >= at,
        "the partial holds {held}, the checkpoint claims {at}"
    );

    // ---- verification -------------------------------------------------------
    // The facts as the engine would have recorded them, and as they still are.
    let facts = FileFacts {
        path: source.clone(),
        size: Bytes(32 * 1024 * 1024),
        modified: None,
        digest: None,
    };
    let record = ResumeRecord {
        checkpoint: Bytes(at),
        prefix_sha256: digest.clone(),
        ..ResumeRecord::new(facts.clone(), partial.display().to_string())
    };
    let mut lane = c.backend.open_lane().await.expect("lane");
    let prefix = relay_core::resume::check(
        relay_core::resume::Verify {
            record: &record,
            direction: Direction::Down,
            source_now: Some(&facts),
            local_path: &local,
            remote_path: &source,
        },
        lane.as_mut(),
    )
    .await
    .expect("the partial this engine wrote verifies against the source it came from");

    // ---- the resumed attempt ------------------------------------------------
    let request = TransferReq {
        job,
        remote_path: source.clone(),
        local_path: local.clone(),
        offset: at,
        prefix: Some(prefix),
        progress: ProgressSink::noop(),
        checkpoint: CheckpointSink::noop(),
        cancel: CancellationToken::new(),
        keep_partial: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let outcome = lane.download(request).await.expect("the resume completes");
    lane.close().await;

    assert_eq!(outcome.final_size, 32 * 1024 * 1024);
    assert_eq!(
        sha256(&std::fs::read(&local).expect("read back")),
        sha256(&std::fs::read(host_file("assets/thirty-two-mib.bin")).expect("source")),
        "the resumed file is the server's file, not a splice that is the right length"
    );
    let leftovers: Vec<String> = std::fs::read_dir(dir.path())
        .expect("dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.contains("relaypart"))
        .collect();
    assert!(leftovers.is_empty(), "partials remain: {leftovers:?}");
}

/// The other half of the same rule: a partial whose bytes do not match what the source
/// starts with must be refused, however plausible its length is.
#[tokio::test]
async fn a_partial_that_does_not_match_the_source_is_refused() {
    let mut c = connect(AuthMethod::Password).await;
    let source = remote("assets/one-mib.bin");
    let dir = tempfile::tempdir().expect("tempdir");
    let job = uuid::Uuid::new_v4();
    let local = dir.path().join("one-mib.bin");

    // A partial of exactly the right length, holding entirely the wrong bytes — the
    // case a size comparison cannot see.
    let partial = relay_core::protocol::local_partial(&local, job);
    let bogus = vec![b'x'; 4096];
    std::fs::write(&partial, &bogus).expect("seed the partial");

    // Everything about the source is unchanged and everything about the partial is
    // self-consistent. Only the bytes disagree, which is the whole point.
    let facts = FileFacts {
        path: source.clone(),
        size: Bytes(1024 * 1024),
        modified: None,
        digest: None,
    };
    let record = ResumeRecord {
        checkpoint: Bytes(4096),
        // The digest the engine *would* have recorded had it written those bytes.
        prefix_sha256: sha256(&bogus),
        ..ResumeRecord::new(facts.clone(), partial.display().to_string())
    };

    let mut lane = c.backend.open_lane().await.expect("lane");
    let verdict = relay_core::resume::check(
        relay_core::resume::Verify {
            record: &record,
            direction: Direction::Down,
            source_now: Some(&facts),
            local_path: &local,
            remote_path: &source,
        },
        lane.as_mut(),
    )
    .await;
    lane.close().await;

    let reason = verdict.expect_err("the source does not start with those bytes");
    assert!(
        reason.contains("no longer starts with"),
        "the refusal should name the side that disagreed: {reason}"
    );
}
