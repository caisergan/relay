//! Phase 1 §1.7: the engine, end to end, over the in-memory backend.
//!
//! Same path the shell calls — `Engine` command, session actor, transfer lane,
//! coordinator, prompt broker, ordered event stream. The only substitution is the
//! backend, which is the one thing that needs a network. Everything these tests assert
//! is behaviour the interface depends on and that no type signature can guarantee.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use relay_core::engine::{BackendFactory, Engine, TransferItem};
use relay_core::error::EngineError;
use relay_core::events::EngineEvent;
use relay_core::hub::EngineHub;
use relay_core::interact::{ConflictAction, PromptReply};
use relay_core::job::{JobState, QueueOp};
use relay_core::mock::{MockFactory, MockFs, MockOptions, NoSecrets};
use relay_core::model::{AuthMethod, Direction, Proto, ServerConfig, SessionId, SessionState};
use relay_core::servers::ServerStore;
use relay_core::settings::SettingsStore;
use relay_core::store::QueueStore;
use relay_core::{EngineSnapshot, Subscription};
use uuid::Uuid;

fn server() -> ServerConfig {
    ServerConfig {
        id: uuid::Uuid::new_v4(),
        name: "Fixture".into(),
        host: "mock.test".into(),
        port: 22,
        proto: Proto::Sftp,
        username: "ada".into(),
        // Agent auth, so connecting does not open a password sheet: these tests are
        // about the session, and the prompt path has its own tests.
        auth: AuthMethod::Agent,
        color: None,
        group: None,
        bookmarks: Vec::new(),
        initial_remote_path: None,
    }
}

struct Harness {
    engine: Arc<Engine>,
    hub: Arc<EngineHub>,
    fs: MockFs,
}

async fn harness(opts: MockOptions) -> Harness {
    let hub = EngineHub::start(&tokio::runtime::Handle::current());
    let fs = MockFs::seeded();
    let factory: Arc<dyn BackendFactory> = Arc::new(MockFactory::new(fs.clone(), opts));
    let engine = Arc::new(
        Engine::with_parts(
            Arc::clone(&hub),
            tokio::runtime::Handle::current(),
            factory,
            Arc::new(NoSecrets),
            Arc::new(ServerStore::ephemeral()),
            QueueStore::in_memory().await.expect("an in-memory queue"),
            Arc::new(SettingsStore::ephemeral()),
        )
        .await
        .expect("the engine starts"),
    );
    Harness { engine, hub, fs }
}

/// Queue one file, the way the shell does.
async fn enqueue_one(
    engine: &Engine,
    session: relay_core::model::SessionId,
    server_id: Uuid,
    direction: Direction,
    remote_path: &str,
    local_path: std::path::PathBuf,
) -> Uuid {
    engine
        .enqueue(
            Uuid::new_v4(),
            vec![TransferItem {
                session,
                server_id,
                direction,
                remote_path: remote_path.into(),
                local_path,
                is_dir: false,
            }],
        )
        .await
        .expect("accepted")[0]
}

/// Poll until a condition holds, rather than sleeping and hoping.
///
/// The engine is asynchronous by construction: a command returns before its events
/// arrive. A fixed sleep would be either flaky or slow, and usually both.
async fn until<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(value) = f() {
            return value;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn snapshot(hub: &EngineHub, sub: &Subscription) -> EngineSnapshot {
    hub.snapshot(sub.id).expect("the subscription is live")
}

#[tokio::test]
async fn a_session_connects_lists_and_transfers_through_the_real_command_surface() {
    let h = harness(MockOptions {
        prompt_host_key: true,
        ..MockOptions::default()
    })
    .await;
    let sub = h.hub.subscribe();
    let cfg = server();
    let server_id = cfg.id;

    let session = h.engine.open_session(cfg).expect("session opens");

    // The connect parks on a host-key sheet. The prompt lives in the engine, so it is
    // in the snapshot before anyone answers it.
    let prompt = until("the host-key prompt", || {
        snapshot(&h.hub, &sub).prompts.into_iter().next()
    })
    .await;
    h.hub
        .prompts()
        .resolve(prompt.id, PromptReply::Accept { remember: true })
        .expect("answering an open prompt");

    // Connecting emits the landing directory without anyone asking for it.
    let listing = until("the landing listing", || {
        snapshot(&h.hub, &sub).listings.into_iter().next()
    })
    .await;
    assert!(
        !listing.entries.is_empty(),
        "the landing directory rendered"
    );

    // A listing the user asked for, on top of the one connect produced.
    let entries = h
        .engine
        .list_dir(session, "/home/deploy")
        .await
        .expect("listing succeeds");
    assert!(entries.iter().any(|e| e.is_dir()), "directories are listed");

    let dir = tempfile::tempdir().expect("tempdir");
    let local = dir.path().join("notes.md");
    let job = enqueue_one(
        &h.engine,
        session,
        server_id,
        Direction::Down,
        "/home/deploy/notes.md",
        local.clone(),
    )
    .await;

    let state = until("the job to finish", || {
        snapshot(&h.hub, &sub)
            .jobs
            .into_iter()
            .find(|j| j.id == job && j.state.is_terminal())
            .map(|j| j.state)
    })
    .await;
    assert!(
        matches!(state, JobState::Done { .. }),
        "the download finished: {state:?}"
    );
    assert!(local.exists(), "the file is where it was asked to go");
    assert_eq!(
        std::fs::read(&local).expect("read back"),
        h.fs.read_file("/home/deploy/notes.md").expect("source"),
        "the bytes match the source"
    );

    // Nothing is left behind that a person would have to find and delete.
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .expect("dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".relaypart"))
        .collect();
    assert!(leftovers.is_empty(), "partial files remain: {leftovers:?}");

    h.engine.close_session(session).await;
}

#[tokio::test]
async fn declining_the_host_key_is_a_decision_not_a_fault() {
    let h = harness(MockOptions {
        prompt_host_key: true,
        ..MockOptions::default()
    })
    .await;
    let sub = h.hub.subscribe();
    let session = h.engine.open_session(server()).expect("session opens");

    let prompt = until("the host-key prompt", || {
        snapshot(&h.hub, &sub).prompts.into_iter().next()
    })
    .await;
    h.hub
        .prompts()
        .resolve(prompt.id, PromptReply::Deny)
        .expect("answering");

    let state = until("the session to give up", || {
        snapshot(&h.hub, &sub)
            .sessions
            .into_iter()
            .find(|s| s.id == session)
            .map(|s| s.state)
            .filter(|s| matches!(s, relay_core::model::SessionState::Disconnected { .. }))
    })
    .await;
    match state {
        relay_core::model::SessionState::Disconnected { unexpected, .. } => assert!(
            !unexpected,
            "a refused host key must not draw the connection-lost pane"
        ),
        other => panic!("unexpected state {other:?}"),
    }
}

#[tokio::test]
async fn a_transfer_can_be_cancelled_and_leaves_the_destination_alone() {
    let h = harness(MockOptions {
        // Slow enough that cancellation lands mid-transfer rather than after it.
        chunk: 4 * 1024,
        chunk_delay: Duration::from_millis(20),
        ..MockOptions::default()
    })
    .await;
    let sub = h.hub.subscribe();
    let cfg = server();
    let server_id = cfg.id;
    let session = h.engine.open_session(cfg).expect("session opens");

    let dir = tempfile::tempdir().expect("tempdir");
    let local = dir.path().join("big.bin");
    let job = enqueue_one(
        &h.engine,
        session,
        server_id,
        Direction::Down,
        "/var/log/nginx/access.log",
        local.clone(),
    )
    .await;

    until("the transfer to start moving", || {
        snapshot(&h.hub, &sub)
            .jobs
            .into_iter()
            .find(|j| j.id == job && j.transferred.get() > 0)
    })
    .await;

    h.engine
        .queue_control(QueueOp::Cancel { job })
        .await
        .expect("cancel is accepted");

    let state = until("the job to stop", || {
        snapshot(&h.hub, &sub)
            .jobs
            .into_iter()
            .find(|j| j.id == job && j.state.is_terminal())
            .map(|j| j.state)
    })
    .await;
    assert!(matches!(state, JobState::Cancelled { .. }));
    assert!(
        !local.exists(),
        "a cancelled download must not leave a half-written file at the destination"
    );

    // The session survives its lane being torn down; phase 2's retry depends on this.
    assert!(
        h.engine.list_dir(session, "/home/deploy").await.is_ok(),
        "the session is still usable"
    );
}

#[tokio::test]
async fn an_existing_destination_opens_the_conflict_sheet_and_skip_leaves_it_untouched() {
    let h = harness(MockOptions::default()).await;
    let sub = h.hub.subscribe();
    let cfg = server();
    let server_id = cfg.id;
    let session = h.engine.open_session(cfg).expect("session opens");

    let dir = tempfile::tempdir().expect("tempdir");
    let local = dir.path().join("notes.md");
    std::fs::write(&local, b"mine, not the server's").expect("seed the destination");

    let job = enqueue_one(
        &h.engine,
        session,
        server_id,
        Direction::Down,
        "/home/deploy/notes.md",
        local.clone(),
    )
    .await;

    let prompt = until("the conflict sheet", || {
        snapshot(&h.hub, &sub)
            .prompts
            .into_iter()
            .find(|p| matches!(p.prompt, relay_core::Prompt::Conflict { .. }))
    })
    .await;

    // The job says what it is waiting for, by id, before the answer exists.
    let awaiting = snapshot(&h.hub, &sub)
        .jobs
        .into_iter()
        .find(|j| j.id == job)
        .expect("the job is in the snapshot");
    assert_eq!(
        awaiting.state,
        JobState::AwaitingPrompt { prompt: prompt.id },
        "the drawer can name the sheet the job is blocked on"
    );

    match prompt.prompt {
        relay_core::Prompt::Conflict { resume_allowed, .. } => assert!(
            !resume_allowed,
            "phase 1 must not offer a resume it has not verified"
        ),
        other => panic!("unexpected prompt {other:?}"),
    }

    h.hub
        .prompts()
        .resolve(
            prompt.id,
            PromptReply::Conflict {
                action: ConflictAction::Skip,
                apply_to_remaining: false,
            },
        )
        .expect("answering");

    until("the job to settle", || {
        snapshot(&h.hub, &sub)
            .jobs
            .into_iter()
            .find(|j| j.id == job && j.state.is_terminal())
    })
    .await;
    assert_eq!(
        std::fs::read(&local).expect("read back"),
        b"mine, not the server's",
        "skip means the file on disk is exactly as it was"
    );
}

#[tokio::test]
async fn closing_a_session_releases_a_transfer_parked_on_an_unanswered_prompt() {
    let h = harness(MockOptions::default()).await;
    let sub = h.hub.subscribe();
    let cfg = server();
    let server_id = cfg.id;
    let session = h.engine.open_session(cfg).expect("session opens");

    let dir = tempfile::tempdir().expect("tempdir");
    let local = dir.path().join("notes.md");
    std::fs::write(&local, b"in the way").expect("seed");

    enqueue_one(
        &h.engine,
        session,
        server_id,
        Direction::Down,
        "/home/deploy/notes.md",
        local,
    )
    .await;

    until("the conflict sheet", || {
        snapshot(&h.hub, &sub)
            .prompts
            .into_iter()
            .find(|p| matches!(p.prompt, relay_core::Prompt::Conflict { .. }))
    })
    .await;

    // Nobody answers. Closing must still finish, and within the grace period — a
    // transfer waiting on a sheet is exactly the case that would otherwise hang exit.
    tokio::time::timeout(Duration::from_secs(8), h.engine.close_session(session))
        .await
        .expect("closing a session with an open prompt must not hang");

    assert!(
        h.hub.prompts().pending().is_empty(),
        "closing denies the prompts its session opened"
    );
}

#[tokio::test]
async fn a_stalled_listing_does_not_stop_the_session_from_closing() {
    let h = harness(MockOptions {
        // Far longer than the close grace period: the operation is still in flight
        // when the session is asked to go away.
        op_latency: Duration::from_secs(60),
        ..MockOptions::default()
    })
    .await;
    let cfg = server();
    let session = h.engine.open_session(cfg).expect("session opens");

    let engine = Arc::clone(&h.engine);
    let listing = tokio::spawn(async move { engine.list_dir(session, "/home/deploy").await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    tokio::time::timeout(Duration::from_secs(8), h.engine.close_session(session))
        .await
        .expect("a stalled operation must not hold shutdown open");

    // The caller is told, rather than left waiting on a session that no longer exists.
    let outcome = tokio::time::timeout(Duration::from_secs(2), listing)
        .await
        .expect("the pending call is released")
        .expect("no panic");
    assert!(
        outcome.is_err(),
        "a stalled call fails once its session closes"
    );
}

/// Phase 1 answered these with `Unsupported`, honestly, because there was no
/// scheduler behind them. There is one now, so every operation the drawer offers has
/// to reach it and come back.
#[tokio::test]
async fn every_queue_operation_reaches_the_scheduler() {
    let h = harness(MockOptions {
        chunk: 8 * 1024,
        chunk_delay: Duration::from_millis(50),
        ..MockOptions::default()
    })
    .await;
    let cfg = server();
    let server_id = cfg.id;
    let session = h.engine.open_session(cfg).expect("session opens");
    let dir = tempfile::tempdir().expect("tempdir");
    let job = enqueue_one(
        &h.engine,
        session,
        server_id,
        Direction::Down,
        "/var/www/assets/app.js",
        dir.path().join("app.js"),
    )
    .await;

    for op in [
        QueueOp::PauseAll,
        QueueOp::ResumeAll,
        QueueOp::Pause { job },
        QueueOp::Resume { job },
        QueueOp::Reorder { job, after: None },
        QueueOp::Cancel { job },
        QueueOp::Retry { job },
        QueueOp::ClearCompleted,
    ] {
        h.engine
            .queue_control(op)
            .await
            .unwrap_or_else(|err| panic!("{op:?} should be supported, got {err}"));
    }
}

/// A tab closing is not a decision to throw its transfers away. Reopening the server
/// should find them where they were left, paused.
#[tokio::test]
async fn closing_a_session_pauses_its_queue_rather_than_discarding_it() {
    let h = harness(MockOptions {
        chunk: 4 * 1024,
        chunk_delay: Duration::from_millis(50),
        ..MockOptions::default()
    })
    .await;
    let cfg = server();
    let server_id = cfg.id;
    let session = h.engine.open_session(cfg).expect("session opens");
    let dir = tempfile::tempdir().expect("tempdir");
    let job = enqueue_one(
        &h.engine,
        session,
        server_id,
        Direction::Down,
        "/var/www/assets/app.js",
        dir.path().join("app.js"),
    )
    .await;

    h.engine.close_session(session).await;

    // `until` polls a synchronous closure; the queue's snapshot is asynchronous, so
    // this one waits on its own.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let state = loop {
        let found = h
            .engine
            .queue()
            .snapshot()
            .await
            .into_iter()
            .find(|j| j.id == job)
            .map(|j| j.state);
        match found {
            Some(state) if !matches!(state, JobState::Preparing | JobState::Transferring) => {
                break state;
            }
            _ if tokio::time::Instant::now() > deadline => panic!("the job never stopped"),
            _ => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    };
    assert!(
        matches!(state, JobState::Paused { .. }),
        "the transfer is waiting for its server, not gone: {state:?}"
    );
}

#[tokio::test]
async fn a_command_for_a_session_that_does_not_exist_is_an_error_not_a_panic() {
    let h = harness(MockOptions::default()).await;
    let missing: SessionId = uuid::Uuid::new_v4();
    assert!(h.engine.list_dir(missing, "/").await.is_err());
    assert!(
        h.engine
            .enqueue(
                Uuid::new_v4(),
                vec![TransferItem {
                    session: missing,
                    server_id: Uuid::new_v4(),
                    direction: Direction::Down,
                    remote_path: "/home/deploy/notes.md".into(),
                    local_path: PathBuf::from("/tmp/x"),
                    is_dir: false,
                }],
            )
            .await
            .is_err(),
        "a job aimed at nothing would wait in the queue for ever"
    );
}

#[tokio::test]
async fn browsing_stays_responsive_while_two_transfers_run() {
    let h = harness(MockOptions {
        chunk: 8 * 1024,
        chunk_delay: Duration::from_millis(10),
        ..MockOptions::default()
    })
    .await;
    let sub = h.hub.subscribe();
    let cfg = server();
    let server_id = cfg.id;
    let session = h.engine.open_session(cfg).expect("session opens");

    let dir = tempfile::tempdir().expect("tempdir");
    let mut jobs = Vec::new();
    for name in ["one.bin", "two.bin"] {
        jobs.push(
            enqueue_one(
                &h.engine,
                session,
                server_id,
                Direction::Down,
                "/var/www/assets/app.js",
                dir.path().join(name),
            )
            .await,
        );
    }

    until("both transfers to be moving", || {
        let moving = snapshot(&h.hub, &sub)
            .jobs
            .into_iter()
            .filter(|j| jobs.contains(&j.id) && j.transferred.get() > 0)
            .count();
        (moving == 2).then_some(())
    })
    .await;

    // The claim the whole lane design exists to support: a listing does not queue
    // behind a transfer.
    let started = std::time::Instant::now();
    h.engine
        .list_dir(session, "/home/deploy")
        .await
        .expect("browsing works during transfers");
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "a listing waited {elapsed:?} behind running transfers"
    );

    for job in jobs {
        until("the transfers to finish", || {
            snapshot(&h.hub, &sub)
                .jobs
                .into_iter()
                .find(|j| j.id == job && j.state.is_terminal())
        })
        .await;
    }
}

#[tokio::test]
async fn events_carry_the_session_through_its_whole_life() {
    let h = harness(MockOptions::default()).await;
    let mut sub = h.hub.subscribe();
    let session = h.engine.open_session(server()).expect("session opens");

    let mut kinds = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        let Ok(Some(envelope)) =
            tokio::time::timeout(Duration::from_millis(200), sub.rx.recv()).await
        else {
            continue;
        };
        match envelope.update {
            EngineEvent::SessionOpened { .. } => kinds.push("opened"),
            EngineEvent::SessionState { .. } => kinds.push("state"),
            EngineEvent::Listing { .. } => kinds.push("listing"),
            EngineEvent::SessionClosed { .. } => kinds.push("closed"),
            _ => {}
        }
        if kinds.contains(&"listing") {
            h.engine.close_session(session).await;
        }
        if kinds.contains(&"closed") {
            break;
        }
    }

    assert_eq!(
        kinds.first(),
        Some(&"opened"),
        "a tab must exist before anything reports state for it"
    );
    assert!(
        kinds.contains(&"closed"),
        "the close is announced: {kinds:?}"
    );
}

/// A recursive download: the folder is a parent, the files inside it are children, and
/// the parent finishes only when they all do.
#[tokio::test]
async fn a_folder_download_queues_its_tree_and_finishes_with_it() {
    let h = harness(MockOptions::default()).await;
    let cfg = server();
    let server_id = cfg.id;
    let session = h.engine.open_session(cfg).expect("session opens");
    let dir = tempfile::tempdir().expect("tempdir");
    let destination = dir.path().join("www");
    let ids = h
        .engine
        .enqueue(
            Uuid::new_v4(),
            vec![TransferItem {
                session,
                server_id,
                direction: Direction::Down,
                remote_path: "/var/www".into(),
                local_path: destination.clone(),
                is_dir: true,
            }],
        )
        .await
        .expect("a folder is accepted");
    let parent = ids[0];

    let state = poll_job(&h, parent, |state| state.is_terminal()).await;
    assert!(
        matches!(state, JobState::Done { skipped: false, .. }),
        "the folder finished: {state:?}"
    );

    // Everything under the root arrived, at the right depth.
    assert_eq!(
        std::fs::read(destination.join("index.html")).expect("index.html"),
        h.fs.read_file("/var/www/index.html").expect("source"),
    );
    assert_eq!(
        std::fs::read(destination.join("assets/logo.svg")).expect("logo.svg"),
        h.fs.read_file("/var/www/assets/logo.svg").expect("source"),
    );
    assert!(destination.join("assets/app.js").exists());

    // The parent's progress is the sum of its children's, not a number of its own.
    let jobs = h.engine.queue().snapshot().await;
    let children: Vec<_> = jobs.iter().filter(|j| j.parent == Some(parent)).collect();
    assert_eq!(children.len(), 3, "three files under /var/www");
    let folder = jobs.iter().find(|j| j.id == parent).expect("the folder");
    assert_eq!(
        folder.transferred.get(),
        children.iter().map(|c| c.transferred.get()).sum::<u64>(),
    );
}

/// An empty directory is still part of the folder: it has to exist at the destination,
/// even though nothing is queued for it.
#[tokio::test]
async fn a_folder_download_creates_directories_that_hold_no_files() {
    let h = harness(MockOptions::default()).await;
    let cfg = server();
    let server_id = cfg.id;
    let session = h.engine.open_session(cfg).expect("session opens");

    let dir = tempfile::tempdir().expect("tempdir");
    let destination = dir.path().join("deploy");
    let parent = h
        .engine
        .enqueue(
            Uuid::new_v4(),
            vec![TransferItem {
                session,
                server_id,
                direction: Direction::Down,
                remote_path: "/home/deploy".into(),
                local_path: destination.clone(),
                is_dir: true,
            }],
        )
        .await
        .expect("accepted")[0];

    poll_job(&h, parent, |state| state.is_terminal()).await;
    assert!(
        destination.join("releases").is_dir(),
        "an empty remote directory is still transferred"
    );
}

/// Poll one job until a condition holds. The queue's snapshot is asynchronous, so this
/// cannot go through `until`.
async fn poll_job(h: &Harness, job: Uuid, done: impl Fn(&JobState) -> bool) -> JobState {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let found = h
            .engine
            .queue()
            .snapshot()
            .await
            .into_iter()
            .find(|j| j.id == job)
            .map(|j| j.state);
        match found {
            Some(state) if done(&state) => return state,
            _ if tokio::time::Instant::now() > deadline => {
                panic!("the job never reached the state the test was waiting for")
            }
            _ => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
}

/// The upload direction is a different walk: the tree is read from the local disk and
/// every directory has to be created on the server before its files arrive.
#[tokio::test]
async fn a_folder_upload_creates_the_remote_tree_before_filling_it() {
    let h = harness(MockOptions::default()).await;
    let cfg = server();
    let server_id = cfg.id;
    let session = h.engine.open_session(cfg).expect("session opens");

    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join("site");
    std::fs::create_dir_all(source.join("css")).expect("nested source");
    std::fs::create_dir_all(source.join("empty")).expect("empty source");
    std::fs::write(source.join("index.html"), b"<h1>hello</h1>").expect("seed");
    std::fs::write(source.join("css/site.css"), b"body{}").expect("seed");

    let parent = h
        .engine
        .enqueue(
            Uuid::new_v4(),
            vec![TransferItem {
                session,
                server_id,
                direction: Direction::Up,
                remote_path: "/var/www/site".into(),
                local_path: source.clone(),
                is_dir: true,
            }],
        )
        .await
        .expect("accepted")[0];

    let state = poll_job(&h, parent, |state| state.is_terminal()).await;
    assert!(
        matches!(state, JobState::Done { skipped: false, .. }),
        "the upload finished: {state:?}"
    );
    assert_eq!(
        h.fs.read_file("/var/www/site/index.html").as_deref(),
        Some(&b"<h1>hello</h1>"[..]),
    );
    assert_eq!(
        h.fs.read_file("/var/www/site/css/site.css").as_deref(),
        Some(&b"body{}"[..]),
        "a nested file needs its directory made first"
    );
    assert!(
        h.fs.exists("/var/www/site/empty"),
        "an empty directory is created even though nothing is queued for it"
    );
}

/// The exit criterion resume exists for: interrupt a transfer, continue it, and get
/// the file that was on the server rather than a plausible-looking splice.
#[tokio::test]
async fn a_paused_download_continues_from_its_checkpoint_and_the_bytes_are_right() {
    let h = harness(MockOptions {
        chunk: 64 * 1024,
        chunk_delay: Duration::from_millis(20),
        ..MockOptions::default()
    })
    .await;
    let cfg = server();
    let server_id = cfg.id;
    let session = h.engine.open_session(cfg).expect("session opens");

    let dir = tempfile::tempdir().expect("tempdir");
    let local = dir.path().join("access.log");
    let job = enqueue_one(
        &h.engine,
        session,
        server_id,
        Direction::Down,
        "/var/log/nginx/access.log",
        local.clone(),
    )
    .await;

    // Let it get far enough that there is something to resume from.
    poll_job(&h, job, |state| matches!(state, JobState::Transferring)).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let moved = h
            .engine
            .queue()
            .snapshot()
            .await
            .into_iter()
            .find(|j| j.id == job)
            .map(|j| j.transferred.get())
            .unwrap_or(0);
        if moved > 256 * 1024 {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "no progress at all");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    h.engine
        .queue_control(QueueOp::Pause { job })
        .await
        .expect("pause");
    poll_job(&h, job, |state| matches!(state, JobState::Paused { .. })).await;

    // A pause keeps the bytes it is coming back to. A cancellation would not.
    let partial = partial_in(dir.path());
    assert!(
        partial.is_some(),
        "the partial a paused job will continue must still be there"
    );
    let stopped_at = std::fs::metadata(partial.as_ref().unwrap())
        .expect("partial")
        .len();
    assert!(stopped_at > 0);

    h.engine
        .queue_control(QueueOp::Resume { job })
        .await
        .expect("resume");
    let state = poll_job(&h, job, |state| state.is_terminal()).await;
    assert!(
        matches!(state, JobState::Done { skipped: false, .. }),
        "the resumed transfer finished: {state:?}"
    );

    // The whole point: the file is the file, not a splice that happens to be the
    // right length.
    assert_eq!(
        std::fs::read(&local).expect("the finished download"),
        h.fs.read_file("/var/log/nginx/access.log").expect("source"),
    );
    assert_eq!(
        partial_in(dir.path()),
        None,
        "nothing is left behind for a person to find and delete"
    );
}

/// The failure resume verification exists to prevent. The source is replaced while the
/// job is paused; continuing would splice the tail of a new file onto the head of an
/// old one, and every length involved would still add up.
#[tokio::test]
async fn a_source_that_changed_while_paused_restarts_instead_of_splicing() {
    let h = harness(MockOptions {
        chunk: 64 * 1024,
        chunk_delay: Duration::from_millis(20),
        ..MockOptions::default()
    })
    .await;
    let cfg = server();
    let server_id = cfg.id;
    let session = h.engine.open_session(cfg).expect("session opens");

    let dir = tempfile::tempdir().expect("tempdir");
    let local = dir.path().join("access.log");
    let job = enqueue_one(
        &h.engine,
        session,
        server_id,
        Direction::Down,
        "/var/log/nginx/access.log",
        local.clone(),
    )
    .await;

    poll_job(&h, job, |state| matches!(state, JobState::Transferring)).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    h.engine
        .queue_control(QueueOp::Pause { job })
        .await
        .expect("pause");
    poll_job(&h, job, |state| matches!(state, JobState::Paused { .. })).await;

    // Same length, different bytes — the case a size comparison cannot see.
    let replacement = vec![b'!'; 3 * 1024 * 1024];
    h.fs.write_file("/var/log/nginx/access.log", replacement.clone());

    h.engine
        .queue_control(QueueOp::Resume { job })
        .await
        .expect("resume");
    let state = poll_job(&h, job, |state| state.is_terminal()).await;
    assert!(
        matches!(state, JobState::Done { skipped: false, .. }),
        "the transfer completed by restarting: {state:?}"
    );
    assert_eq!(
        std::fs::read(&local).expect("the finished download"),
        replacement,
        "the file is the replacement in full, not the old head with a new tail"
    );
}

/// Whatever `.relaypart` file is in a directory, if any.
fn partial_in(dir: &std::path::Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .expect("dir")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.to_string_lossy().ends_with(".relaypart"))
}

/// The design's amber banner, and what has to be true behind it: a dropped connection
/// pauses the queue rather than failing it, the session counts down and tries again,
/// and the work continues when it comes back.
#[tokio::test]
async fn a_dropped_connection_reconnects_and_the_queue_carries_on() {
    let severed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let h = harness(MockOptions {
        chunk: 32 * 1024,
        chunk_delay: Duration::from_millis(20),
        severed: std::sync::Arc::clone(&severed),
        ..MockOptions::default()
    })
    .await;
    let sub = h.hub.subscribe();
    let cfg = server();
    let server_id = cfg.id;
    let session = h.engine.open_session(cfg).expect("session opens");

    let dir = tempfile::tempdir().expect("tempdir");
    let local = dir.path().join("access.log");
    let job = enqueue_one(
        &h.engine,
        session,
        server_id,
        Direction::Down,
        "/var/log/nginx/access.log",
        local.clone(),
    )
    .await;
    poll_job(&h, job, |state| matches!(state, JobState::Transferring)).await;

    // Pull the cable.
    severed.store(true, std::sync::atomic::Ordering::SeqCst);
    h.engine
        .list_dir(session, "/home/deploy")
        .await
        .expect_err("a browse on a severed connection fails");

    // The banner: reconnecting, with an attempt number to show.
    until("the session to report it is reconnecting", || {
        snapshot(&h.hub, &sub)
            .sessions
            .into_iter()
            .find(|s| s.id == session && matches!(s.state, SessionState::Reconnecting { .. }))
    })
    .await;

    // The queue paused rather than failed. The transfer is fine; the connection is not.
    let paused = poll_job(&h, job, |state| {
        matches!(state, JobState::Paused { .. } | JobState::Queued)
    })
    .await;
    assert!(
        !paused.is_terminal(),
        "a dropped connection must not fail a transfer: {paused:?}"
    );

    // Plug it back in, and press Reconnect rather than waiting out the backoff.
    severed.store(false, std::sync::atomic::Ordering::SeqCst);
    h.engine.reconnect(session).await.expect("reconnect");

    let state = poll_job(&h, job, |state| state.is_terminal()).await;
    assert!(
        matches!(state, JobState::Done { skipped: false, .. }),
        "the queue carried on once the connection came back: {state:?}"
    );
    assert_eq!(
        std::fs::read(&local).expect("the finished download"),
        h.fs.read_file("/var/log/nginx/access.log").expect("source"),
    );
}

/// A rejected host key is an answer, not an accident. Retrying it ten times would
/// produce the same answer ten times and ten more sheets along the way.
#[tokio::test]
async fn a_declined_host_key_does_not_start_a_reconnect_countdown() {
    let h = harness(MockOptions {
        prompt_host_key: true,
        ..MockOptions::default()
    })
    .await;
    let sub = h.hub.subscribe();
    let session = h.engine.open_session(server()).expect("session opens");

    let prompt = until("the host key sheet", || {
        snapshot(&h.hub, &sub).prompts.into_iter().next()
    })
    .await;
    h.hub
        .prompts()
        .resolve(prompt.id, PromptReply::Deny)
        .expect("deny");

    let state = until("the session to settle", || {
        snapshot(&h.hub, &sub)
            .sessions
            .into_iter()
            .find(|s| s.id == session && !matches!(s.state, SessionState::Connecting))
            .map(|s| s.state)
    })
    .await;
    assert!(
        matches!(state, SessionState::Disconnected { .. }),
        "a decision is final until someone changes it: {state:?}"
    );
}

/// The gap between transferring and publishing exists so this can be noticed. A
/// resumed transfer takes its first bytes from one reading of the source and its last
/// from another; if the source moved in between, the result is a file that never
/// existed — and after the rename there is nothing left to protect.
#[tokio::test]
async fn a_source_that_changes_during_a_resume_is_caught_before_the_rename() {
    let h = harness(MockOptions {
        chunk: 64 * 1024,
        chunk_delay: Duration::from_millis(20),
        ..MockOptions::default()
    })
    .await;
    let cfg = server();
    let server_id = cfg.id;
    let session = h.engine.open_session(cfg).expect("session opens");

    let dir = tempfile::tempdir().expect("tempdir");
    let local = dir.path().join("access.log");
    let job = enqueue_one(
        &h.engine,
        session,
        server_id,
        Direction::Down,
        "/var/log/nginx/access.log",
        local.clone(),
    )
    .await;

    poll_job(&h, job, |state| matches!(state, JobState::Transferring)).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    h.engine
        .queue_control(QueueOp::Pause { job })
        .await
        .expect("pause");
    poll_job(&h, job, |state| matches!(state, JobState::Paused { .. })).await;

    // Resume, then replace the source underneath the running transfer with a file of
    // a different length. The bytes already read are from the old one.
    h.engine
        .queue_control(QueueOp::Resume { job })
        .await
        .expect("resume");
    poll_job(&h, job, |state| matches!(state, JobState::Transferring)).await;
    h.fs.write_file("/var/log/nginx/access.log", vec![b'!'; 5 * 1024 * 1024]);

    let state = poll_job(&h, job, |state| state.is_terminal()).await;
    assert!(
        matches!(
            state,
            JobState::Failed {
                error: EngineError::SourceChanged { .. },
                ..
            }
        ),
        "the splice was refused: {state:?}"
    );
    assert!(
        !local.exists(),
        "and the destination was never replaced with it"
    );
    assert_eq!(
        partial_in(dir.path()),
        None,
        "the rejected partial is not left behind"
    );
}
