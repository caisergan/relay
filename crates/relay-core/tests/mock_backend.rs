//! Phase 0 exit criterion 2: round-trip the engine contract against the in-memory
//! backend, including the two behaviours the interface depends on — transfers that do
//! not block browsing, and cancellation that leaves nothing behind.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use relay_core::error::EngineError;
use relay_core::interact::{Interact, Prompt, PromptReply};
use relay_core::mock::{MockBackend, MockFs, MockOptions, NoSecrets};
use relay_core::model::{AuthMethod, Direction, Proto, ServerConfig, SessionId};
use relay_core::protocol::{CheckpointSink, ProgressSink, Protocol, TransferReq};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn server() -> ServerConfig {
    ServerConfig {
        id: Uuid::new_v4(),
        name: "fixture".into(),
        host: "example.test".into(),
        port: 22,
        proto: Proto::Sftp,
        username: "deploy".into(),
        auth: AuthMethod::Agent,
        color: None,
        group: None,
        bookmarks: Vec::new(),
        initial_remote_path: None,
    }
}

/// Answers every prompt affirmatively, standing in for a user at the sheet.
struct AlwaysAccept;

#[async_trait::async_trait]
impl Interact for AlwaysAccept {
    async fn ask(&self, _session: SessionId, prompt: Prompt) -> PromptReply {
        match prompt {
            Prompt::Password { .. } => PromptReply::Password {
                value: "hunter2".into(),
            },
            _ => PromptReply::Accept { remember: true },
        }
    }
}

/// Transfer and publish in one go.
///
/// The lane splits the two so the engine can verify in between; a test that is not
/// about that step still wants the file where it belongs at the end.
async fn transfer(
    lane: &mut Box<dyn relay_core::protocol::TransferLane>,
    req: &TransferReq,
    direction: Direction,
) -> Result<relay_core::protocol::TransferOutcome, EngineError> {
    let moved = match direction {
        Direction::Down => lane.download(req).await?,
        Direction::Up => lane.upload(req).await?,
    };
    lane.finalise(req, &moved).await
}

fn req(job: Uuid, remote: &str, local: PathBuf, cancel: CancellationToken) -> TransferReq {
    TransferReq {
        job,
        remote_path: remote.into(),
        local_path: local,
        offset: 0,
        prefix: None,
        progress: ProgressSink::noop(),
        checkpoint: CheckpointSink::noop(),
        cancel,
        keep_partial: Default::default(),
    }
}

async fn connected(fs: MockFs, opts: MockOptions) -> MockBackend {
    let mut backend = MockBackend::with_options(fs, Uuid::new_v4(), opts);
    backend
        .connect(&server(), &NoSecrets, Arc::new(AlwaysAccept))
        .await
        .expect("mock connect");
    backend
}

#[tokio::test]
async fn lists_a_directory_with_dirs_and_files() {
    let mut backend = connected(MockFs::seeded(), MockOptions::default()).await;

    let entries = backend.list("/home/deploy").await.expect("listing");
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"notes.md"), "got {names:?}");
    assert!(names.contains(&"releases"));

    let dotfile = entries
        .iter()
        .find(|e| e.name == ".bashrc")
        .expect("dotfile listed");
    assert!(
        dotfile.is_hidden(),
        "the pane dims these rather than hiding them"
    );

    let root = backend.list("/").await.expect("root listing");
    assert!(root.iter().any(|e| e.name == "home" && e.is_dir()));
}

#[tokio::test]
async fn missing_paths_report_not_found_rather_than_a_generic_failure() {
    let mut backend = connected(MockFs::seeded(), MockOptions::default()).await;
    let err = backend.list("/nope").await.unwrap_err();
    assert!(matches!(err, EngineError::NotFound { .. }), "got {err:?}");
}

#[tokio::test]
async fn downloads_and_uploads_round_trip_byte_for_byte() {
    let fs = MockFs::seeded();
    let dir = tempfile::tempdir().expect("tempdir");
    let mut backend = connected(fs.clone(), MockOptions::default()).await;

    let local = dir.path().join("app.js");
    let mut lane = backend.open_lane().await.expect("lane");
    let outcome = transfer(
        &mut lane,
        &req(
            Uuid::new_v4(),
            "/var/www/assets/app.js",
            local.clone(),
            CancellationToken::new(),
        ),
        Direction::Down,
    )
    .await
    .expect("download");

    assert_eq!(outcome.bytes, 256 * 1024);
    assert_eq!(
        std::fs::read(&local).unwrap(),
        fs.read_file("/var/www/assets/app.js").unwrap()
    );
    assert!(
        std::fs::read_dir(dir.path()).unwrap().count() == 1,
        "the temporary partial must be gone after finalisation"
    );

    transfer(
        &mut lane,
        &req(
            Uuid::new_v4(),
            "/var/www/assets/copy.js",
            local.clone(),
            CancellationToken::new(),
        ),
        Direction::Up,
    )
    .await
    .expect("upload");
    assert_eq!(
        fs.read_file("/var/www/assets/copy.js").unwrap(),
        std::fs::read(&local).unwrap()
    );
}

#[tokio::test]
async fn progress_is_monotonic_and_ends_at_the_full_size() {
    let fs = MockFs::seeded();
    let dir = tempfile::tempdir().expect("tempdir");
    let mut backend = connected(
        fs,
        MockOptions {
            chunk: 4096,
            ..MockOptions::default()
        },
    )
    .await;

    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
    let sink = {
        let seen = std::sync::Arc::clone(&seen);
        ProgressSink::new(move |bytes| seen.lock().unwrap().push(bytes))
    };

    let mut lane = backend.open_lane().await.expect("lane");
    lane.download(&TransferReq {
        job: Uuid::new_v4(),
        remote_path: "/var/log/nginx/access.log".into(),
        local_path: dir.path().join("access.log"),
        offset: 0,
        prefix: None,
        progress: sink,
        checkpoint: CheckpointSink::noop(),
        cancel: CancellationToken::new(),
        keep_partial: Default::default(),
    })
    .await
    .expect("download");

    let seen = seen.lock().unwrap().clone();
    assert!(
        seen.windows(2).all(|w| w[1] > w[0]),
        "progress went backwards"
    );
    assert_eq!(seen.last().copied(), Some(3 * 1024 * 1024));
}

/// The reason `open_lane` exists: a running transfer must not hold the backend.
#[tokio::test]
async fn browsing_continues_while_two_lanes_transfer() {
    let fs = MockFs::seeded();
    let dir = tempfile::tempdir().expect("tempdir");
    let opts = MockOptions {
        chunk: 16 * 1024,
        chunk_delay: Duration::from_millis(2),
        ..MockOptions::default()
    };
    let mut backend = connected(fs, opts).await;

    let mut first = backend.open_lane().await.expect("lane 1");
    let mut second = backend.open_lane().await.expect("lane 2");

    let a = dir.path().join("a.log");
    let b = dir.path().join("b.js");
    let t1 = tokio::spawn(async move {
        let request = req(
            Uuid::new_v4(),
            "/var/log/nginx/access.log",
            a,
            CancellationToken::new(),
        );
        transfer(&mut first, &request, Direction::Down).await
    });
    let t2 = tokio::spawn(async move {
        let request = req(
            Uuid::new_v4(),
            "/var/www/assets/app.js",
            b,
            CancellationToken::new(),
        );
        transfer(&mut second, &request, Direction::Down).await
    });

    // The backend is still ours to browse with while both transfers run.
    for _ in 0..5 {
        backend
            .list("/home/deploy")
            .await
            .expect("listing during transfers");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    t1.await.unwrap().expect("transfer 1");
    t2.await.unwrap().expect("transfer 2");
}

#[tokio::test]
async fn cancellation_removes_the_partial_and_never_creates_the_destination() {
    let fs = MockFs::seeded();
    let dir = tempfile::tempdir().expect("tempdir");
    let opts = MockOptions {
        chunk: 8 * 1024,
        chunk_delay: Duration::from_millis(5),
        ..MockOptions::default()
    };
    let mut backend = connected(fs, opts).await;
    let mut lane = backend.open_lane().await.expect("lane");

    let cancel = CancellationToken::new();
    let local = dir.path().join("access.log");
    let handle = {
        let cancel = cancel.clone();
        let local = local.clone();
        tokio::spawn(async move {
            let request = req(Uuid::new_v4(), "/var/log/nginx/access.log", local, cancel);
            transfer(&mut lane, &request, Direction::Down).await
        })
    };

    tokio::time::sleep(Duration::from_millis(20)).await;
    cancel.cancel();

    let err = handle.await.unwrap().unwrap_err();
    assert!(matches!(err, EngineError::Cancelled), "got {err:?}");
    assert!(
        !local.exists(),
        "a cancelled download must not create the destination"
    );
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "the owned partial file must be cleaned up"
    );
}

#[tokio::test]
async fn a_resume_offset_is_refused_when_the_partial_does_not_back_it_up() {
    let fs = MockFs::seeded();
    let dir = tempfile::tempdir().expect("tempdir");
    let mut backend = connected(fs, MockOptions::default()).await;
    let mut lane = backend.open_lane().await.expect("lane");

    let job = Uuid::new_v4();
    let local = dir.path().join("app.js");
    // No partial file exists at all, yet the record claims 100 KiB were transferred.
    let err = lane
        .download(&TransferReq {
            job,
            remote_path: "/var/www/assets/app.js".into(),
            local_path: local.clone(),
            offset: 100 * 1024,
            prefix: None,
            progress: ProgressSink::noop(),
            checkpoint: CheckpointSink::noop(),
            cancel: CancellationToken::new(),
            keep_partial: Default::default(),
        })
        .await
        .unwrap_err();

    assert!(
        matches!(
            err,
            EngineError::NotFound { .. } | EngineError::ResumeUnverifiable { .. }
        ),
        "an unbacked offset must refuse to resume, got {err:?}"
    );
    assert!(!local.exists());
}

#[tokio::test]
async fn declining_the_host_key_fails_the_connect_with_trust_rejected() {
    struct AlwaysDeny;
    #[async_trait::async_trait]
    impl Interact for AlwaysDeny {
        async fn ask(&self, _session: SessionId, _prompt: Prompt) -> PromptReply {
            PromptReply::Deny
        }
    }

    let opts = MockOptions {
        prompt_host_key: true,
        ..MockOptions::default()
    };
    let mut backend = MockBackend::with_options(MockFs::seeded(), Uuid::new_v4(), opts);
    let err = backend
        .connect(&server(), &NoSecrets, Arc::new(AlwaysDeny))
        .await
        .unwrap_err();

    assert!(
        matches!(err, EngineError::TrustRejected { .. }),
        "got {err:?}"
    );
}
