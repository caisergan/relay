//! Phase 0 exit criterion 3, engine side: a whole session — connect, trust prompt,
//! listing, transfer, remount, close — driven through the real coordinator and the
//! real event stream, with only the backend faked.

use std::time::Duration;

use relay_core::EngineHub;
use relay_core::demo::DemoEngine;
use relay_core::interact::PromptReply;
use relay_core::job::JobState;
use relay_core::model::{AuthMethod, Direction, Proto, ServerConfig, SessionState};
use uuid::Uuid;

fn server() -> ServerConfig {
    ServerConfig {
        id: Uuid::new_v4(),
        name: "staging".into(),
        host: "staging.example".into(),
        port: 22,
        proto: Proto::Sftp,
        username: "deploy".into(),
        auth: AuthMethod::Agent,
        color: Some("#2456E6".into()),
        group: None,
        bookmarks: Vec::new(),
        initial_remote_path: Some("/var/www".into()),
    }
}

/// Poll until `f` returns something, or fail the test rather than hang CI.
async fn until<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    match tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(value) = f() {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    {
        Ok(value) => value,
        Err(_) => panic!("timed out waiting for {what}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_connects_lists_transfers_and_survives_a_remount() {
    let hub = EngineHub::start(&tokio::runtime::Handle::current());
    let engine = DemoEngine::new(
        std::sync::Arc::clone(&hub),
        tokio::runtime::Handle::current(),
    );
    let sub = hub.subscribe();

    let session = engine.open_session(server());

    // The connect blocks on a host-key question, which the UI answers by id.
    let prompt = until("the host-key prompt", || {
        hub.prompts().pending().first().cloned()
    })
    .await;
    assert_eq!(prompt.session, session);
    hub.prompts()
        .resolve(prompt.id, PromptReply::Accept { remember: true })
        .expect("accepting the host key");

    // Connecting → Connected, and the landing directory arrives as a listing.
    until("the session to connect", || {
        let snap = hub.snapshot(sub.id).ok()?;
        snap.sessions
            .iter()
            .find(|s| s.id == session)
            .filter(|s| matches!(s.state, SessionState::Connected { .. }))
            .map(|_| ())
    })
    .await;

    let listing = until("the landing listing", || {
        hub.snapshot(sub.id).ok()?.listings.first().cloned()
    })
    .await;
    assert_eq!(listing.path, "/var/www");
    assert!(
        listing.entries.iter().any(|e| e.name == "index.html"),
        "{:?}",
        listing.entries
    );

    // Navigating publishes a newer listing for the same session.
    let entries = engine
        .list_dir(session, "/var/www/assets")
        .await
        .expect("list assets");
    assert!(entries.iter().any(|e| e.name == "app.js"));

    // A transfer runs to completion and reports through the job stream.
    let dir = tempfile::tempdir().unwrap();
    let local = dir.path().join("app.js");
    let job = engine
        .enqueue(
            session,
            Direction::Down,
            "/var/www/assets/app.js".into(),
            local.clone(),
        )
        .await
        .expect("enqueue");

    // Mid-flight, a remount must see the job exactly once, not twice. The snapshot
    // may be taken before the enqueue is sequenced, which is precisely why the
    // contract is "subscribe, then snapshot, then replay above the watermark".
    let remount = hub.subscribe();
    let seen = until("the job to reach the projection", || {
        let snap = hub.snapshot(remount.id).ok()?;
        let count = snap.jobs.iter().filter(|j| j.id == job).count();
        (count > 0).then_some(count)
    })
    .await;
    assert_eq!(seen, 1, "a remount must not duplicate a job");

    let done = until("the transfer to finish", || {
        let snap = hub.snapshot(remount.id).ok()?;
        snap.jobs
            .iter()
            .find(|j| j.id == job && matches!(j.state, JobState::Done { .. }))
            .cloned()
    })
    .await;
    assert_eq!(done.transferred, done.size.expect("size known"));
    assert_eq!(
        std::fs::read(&local).unwrap().len() as u64,
        done.transferred.get()
    );

    // Closing takes the session out of the projection and leaves nothing pending.
    engine.close_session(session).await;
    until("the session to close", || {
        hub.snapshot(remount.id)
            .ok()?
            .sessions
            .is_empty()
            .then_some(())
    })
    .await;
    assert!(hub.prompts().pending().is_empty());

    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn declining_the_host_key_leaves_a_deliberate_disconnect_not_a_fault() {
    let hub = EngineHub::start(&tokio::runtime::Handle::current());
    let engine = DemoEngine::new(
        std::sync::Arc::clone(&hub),
        tokio::runtime::Handle::current(),
    );
    let sub = hub.subscribe();

    let session = engine.open_session(server());
    let prompt = until("the host-key prompt", || {
        hub.prompts().pending().first().cloned()
    })
    .await;
    hub.prompts()
        .resolve(prompt.id, PromptReply::Deny)
        .expect("declining");

    let state = until("the disconnect", || {
        let snap = hub.snapshot(sub.id).ok()?;
        snap.sessions
            .iter()
            .find(|s| s.id == session)
            .map(|s| s.state.clone())
            .filter(|s| matches!(s, SessionState::Disconnected { .. }))
    })
    .await;

    match state {
        SessionState::Disconnected { unexpected, reason } => {
            assert!(
                !unexpected,
                "declining trust must not raise the connection-lost pane"
            );
            assert!(
                reason.contains("trust"),
                "reason should name the cause: {reason}"
            );
        }
        other => panic!("unexpected {other:?}"),
    }

    hub.shutdown().await;
}
