//! Phase 1 §1.7: the engine, end to end, over the in-memory backend.
//!
//! Same path the shell calls — `Engine` command, session actor, transfer lane,
//! coordinator, prompt broker, ordered event stream. The only substitution is the
//! backend, which is the one thing that needs a network. Everything these tests assert
//! is behaviour the interface depends on and that no type signature can guarantee.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use relay_core::engine::{BackendFactory, Engine};
use relay_core::error::EngineError;
use relay_core::events::EngineEvent;
use relay_core::hub::EngineHub;
use relay_core::interact::{ConflictAction, PromptReply};
use relay_core::job::{JobState, QueueOp};
use relay_core::mock::{MockFactory, MockFs, MockOptions, NoSecrets};
use relay_core::model::{AuthMethod, Direction, Proto, ServerConfig, SessionId};
use relay_core::servers::ServerStore;
use relay_core::{EngineSnapshot, Subscription};

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

fn harness(opts: MockOptions) -> Harness {
    let hub = EngineHub::start(&tokio::runtime::Handle::current());
    let fs = MockFs::seeded();
    let factory: Arc<dyn BackendFactory> = Arc::new(MockFactory::new(fs.clone(), opts));
    let engine = Arc::new(Engine::with_parts(
        Arc::clone(&hub),
        tokio::runtime::Handle::current(),
        factory,
        Arc::new(NoSecrets),
        Arc::new(ServerStore::ephemeral()),
    ));
    Harness { engine, hub, fs }
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
    });
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
    let job = h
        .engine
        .enqueue(
            session,
            server_id,
            Direction::Down,
            "/home/deploy/notes.md".into(),
            local.clone(),
        )
        .await
        .expect("the transfer is accepted");

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
    });
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
    });
    let sub = h.hub.subscribe();
    let cfg = server();
    let server_id = cfg.id;
    let session = h.engine.open_session(cfg).expect("session opens");

    let dir = tempfile::tempdir().expect("tempdir");
    let local = dir.path().join("big.bin");
    let job = h
        .engine
        .enqueue(
            session,
            server_id,
            Direction::Down,
            "/var/log/nginx/access.log".into(),
            local.clone(),
        )
        .await
        .expect("accepted");

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
    assert_eq!(state, JobState::Cancelled);
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
    let h = harness(MockOptions::default());
    let sub = h.hub.subscribe();
    let cfg = server();
    let server_id = cfg.id;
    let session = h.engine.open_session(cfg).expect("session opens");

    let dir = tempfile::tempdir().expect("tempdir");
    let local = dir.path().join("notes.md");
    std::fs::write(&local, b"mine, not the server's").expect("seed the destination");

    let job = h
        .engine
        .enqueue(
            session,
            server_id,
            Direction::Down,
            "/home/deploy/notes.md".into(),
            local.clone(),
        )
        .await
        .expect("accepted");

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
    let h = harness(MockOptions::default());
    let sub = h.hub.subscribe();
    let cfg = server();
    let server_id = cfg.id;
    let session = h.engine.open_session(cfg).expect("session opens");

    let dir = tempfile::tempdir().expect("tempdir");
    let local = dir.path().join("notes.md");
    std::fs::write(&local, b"in the way").expect("seed");

    h.engine
        .enqueue(
            session,
            server_id,
            Direction::Down,
            "/home/deploy/notes.md".into(),
            local,
        )
        .await
        .expect("accepted");

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
    });
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

#[tokio::test]
async fn unsupported_queue_operations_say_so_instead_of_pretending() {
    let h = harness(MockOptions::default());
    let err = h
        .engine
        .queue_control(QueueOp::PauseAll)
        .await
        .expect_err("phase 1 has no scheduler");
    assert!(matches!(err, EngineError::Unsupported { .. }));
}

#[tokio::test]
async fn a_command_for_a_session_that_does_not_exist_is_an_error_not_a_panic() {
    let h = harness(MockOptions::default());
    let missing: SessionId = uuid::Uuid::new_v4();
    assert!(h.engine.list_dir(missing, "/").await.is_err());
    assert!(
        h.engine
            .enqueue(
                missing,
                uuid::Uuid::new_v4(),
                Direction::Down,
                "/home/deploy/notes.md".into(),
                PathBuf::from("/tmp/x"),
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn browsing_stays_responsive_while_two_transfers_run() {
    let h = harness(MockOptions {
        chunk: 8 * 1024,
        chunk_delay: Duration::from_millis(10),
        ..MockOptions::default()
    });
    let sub = h.hub.subscribe();
    let cfg = server();
    let server_id = cfg.id;
    let session = h.engine.open_session(cfg).expect("session opens");

    let dir = tempfile::tempdir().expect("tempdir");
    let mut jobs = Vec::new();
    for name in ["one.bin", "two.bin"] {
        jobs.push(
            h.engine
                .enqueue(
                    session,
                    server_id,
                    Direction::Down,
                    "/var/www/assets/app.js".into(),
                    dir.path().join(name),
                )
                .await
                .expect("accepted"),
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
    let h = harness(MockOptions::default());
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
