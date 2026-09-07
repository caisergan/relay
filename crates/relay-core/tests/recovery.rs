//! Phase 0 exit criterion 3: the parts of the IPC contract that survive a UI remount.
//!
//! These tests are the reason the coordinator exists. Each one describes a failure a
//! user would actually hit — a refreshed window losing a finished transfer, a trust
//! sheet vanishing mid-connect, a double-answered prompt, a stale listing overwriting
//! the directory someone just navigated to.

use std::time::Duration;

use chrono::Utc;
use relay_core::EngineHub;
use relay_core::coordinator::SnapshotError;
use relay_core::events::{EngineEvent, ListingSnapshot, LogKind, LogLine};
use relay_core::interact::{Interact, Prompt, PromptReply, ResolveError};
use relay_core::job::{JobSnapshot, JobState};
use relay_core::model::{Direction, FileKind, Proto, RemoteEntry, Session, SessionState};
use relay_core::wire::{Bytes, Order, Seq};
use uuid::Uuid;

fn hub() -> std::sync::Arc<EngineHub> {
    EngineHub::start(&tokio::runtime::Handle::current())
}

fn session(id: Uuid) -> Session {
    Session {
        id,
        server_id: Uuid::new_v4(),
        name: "fixture".into(),
        proto: Proto::Sftp,
        state: SessionState::Connecting,
        latency_ms: None,
        remote_path: None,
    }
}

fn job(id: Uuid, session: Uuid, state: JobState, transferred: u64) -> Box<JobSnapshot> {
    Box::new(JobSnapshot {
        id,
        session,
        server_id: Uuid::new_v4(),
        direction: Direction::Down,
        remote_path: "/var/www/app.js".into(),
        local_path: "/tmp/app.js".into(),
        size: Some(Bytes(1000)),
        transferred: Bytes(transferred),
        state,
        order: Order(1024),
        speed_bps: None,
        eta_secs: None,
        conflict_policy: None,
        parent: None,
        started_at: None,
    })
}

fn listing(session: Uuid, path: &str, request: u64) -> Box<ListingSnapshot> {
    Box::new(ListingSnapshot {
        session,
        path: path.into(),
        request: Seq(request),
        entries: vec![RemoteEntry {
            name: path.trim_start_matches('/').to_string(),
            kind: FileKind::File,
            target_kind: None,
            size: Bytes(1),
            modified: None,
            perms: None,
            mode: None,
            owner: None,
            group: None,
        }],
        at: Utc::now(),
    })
}

/// Wait for the coordinator to have sequenced at least `n` envelopes for us.
async fn drain(
    rx: &mut tokio::sync::mpsc::Receiver<relay_core::EngineEnvelope>,
    n: usize,
) -> Vec<relay_core::EngineEnvelope> {
    let mut out = Vec::new();
    for _ in 0..n {
        match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
            Ok(Some(env)) => out.push(env),
            _ => break,
        }
    }
    out
}

#[tokio::test]
async fn a_remount_recovers_state_without_replaying_updates_twice() {
    let hub = hub();
    let events = hub.events();
    let sid = Uuid::new_v4();
    let jid = Uuid::new_v4();

    events
        .send(EngineEvent::SessionOpened {
            session: Box::new(session(sid)),
        })
        .await
        .unwrap();
    events
        .send(EngineEvent::JobUpdate {
            job: job(jid, sid, JobState::Done { at: Utc::now() }, 1000),
        })
        .await
        .unwrap();

    // A fresh mount: subscribe first, then snapshot. That order is the contract.
    let mut sub = hub.subscribe();
    // Anything that happened before the subscription must still be in the snapshot.
    let snapshot = loop {
        let snap = hub.snapshot(sub.id).expect("snapshot");
        if !snap.jobs.is_empty() {
            break snap;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };

    assert_eq!(snapshot.sessions.len(), 1);
    assert_eq!(
        snapshot.jobs.len(),
        1,
        "a finished job must survive the remount"
    );
    assert!(matches!(snapshot.jobs[0].state, JobState::Done { .. }));

    // Updates published after the snapshot carry a strictly greater sequence number,
    // which is exactly the rule the bridge uses to avoid double-applying.
    events
        .send(EngineEvent::Latency { id: sid, ms: 42 })
        .await
        .unwrap();
    let later = drain(&mut sub.rx, 1).await;
    let latency = later
        .iter()
        .find(|e| matches!(e.update, EngineEvent::Latency { .. }));
    if let Some(env) = latency {
        assert!(
            env.seq > snapshot.seq,
            "seq {} must exceed watermark {}",
            env.seq,
            snapshot.seq
        );
        assert_eq!(env.epoch, snapshot.epoch);
    }
}

#[tokio::test]
async fn an_unknown_subscription_is_told_to_resubscribe() {
    let hub = hub();
    let err = hub.snapshot(Uuid::new_v4()).unwrap_err();
    assert_eq!(err, SnapshotError::UnknownSubscription);

    let sub = hub.subscribe();
    assert!(hub.snapshot(sub.id).is_ok());
    hub.coordinator().unsubscribe(sub.id);
    assert_eq!(
        hub.snapshot(sub.id).unwrap_err(),
        SnapshotError::UnknownSubscription
    );
}

#[tokio::test]
async fn a_subscriber_that_stops_reading_is_dropped_rather_than_losing_updates() {
    let hub = hub();
    let sub = hub.subscribe();
    let sid = Uuid::new_v4();
    hub.events()
        .send(EngineEvent::SessionOpened {
            session: Box::new(session(sid)),
        })
        .await
        .unwrap();

    // Never read from sub.rx. Once the buffer fills, the subscription is invalidated
    // so the bridge resnapshots instead of silently missing an update.
    for i in 0..(relay_core::coordinator::SUBSCRIBER_BUFFER + 50) {
        hub.events()
            .send(EngineEvent::Log {
                id: sid,
                line: LogLine {
                    at: Utc::now(),
                    kind: LogKind::Status,
                    line: format!("line {i}"),
                },
            })
            .await
            .unwrap();
    }

    let dropped = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if hub.snapshot(sub.id).is_err() {
                break true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or(false);

    assert!(
        dropped,
        "an overflowing subscriber must be invalidated, not quietly starved"
    );
    assert_eq!(hub.coordinator().subscriber_count(), 0);
}

#[tokio::test]
async fn an_open_prompt_is_still_in_the_snapshot_and_answers_exactly_once() {
    let hub = hub();
    let sub = hub.subscribe();
    let sid = Uuid::new_v4();

    let asking = {
        let prompts = std::sync::Arc::clone(hub.prompts());
        tokio::spawn(async move {
            prompts
                .ask(
                    sid,
                    Prompt::HostKey {
                        host: "example.test:22".into(),
                        algo: "ssh-ed25519".into(),
                        sha256: "SHA256:aaaa".into(),
                        changed: false,
                    },
                )
                .await
        })
    };

    // The remounted UI reconstructs the sheet from the snapshot, not from the event.
    let snapshot = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let snap = hub.snapshot(sub.id).expect("snapshot");
            if !snap.prompts.is_empty() {
                break snap;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("prompt reached the snapshot");

    let prompt_id = snapshot.prompts[0].id;
    hub.prompts()
        .resolve(prompt_id, PromptReply::Accept { remember: true })
        .expect("first answer is accepted");

    // A remount that replays its answer must be a no-op, not a second decision.
    assert_eq!(
        hub.prompts()
            .resolve(prompt_id, PromptReply::Accept { remember: true }),
        Err(ResolveError::Unknown)
    );

    let reply = tokio::time::timeout(Duration::from_secs(2), asking)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(reply, PromptReply::Accept { remember: true }));
    assert!(hub.prompts().pending().is_empty());
}

#[tokio::test]
async fn a_reply_that_answers_a_different_question_is_rejected() {
    let hub = hub();
    let sid = Uuid::new_v4();
    let prompts = std::sync::Arc::clone(hub.prompts());
    let asking = tokio::spawn(async move {
        prompts
            .ask(
                sid,
                Prompt::Password {
                    hint: "deploy".into(),
                },
            )
            .await
    });

    let id = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(p) = hub.prompts().pending().first() {
                break p.id;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    assert_eq!(
        hub.prompts().resolve(
            id,
            PromptReply::Conflict {
                action: relay_core::interact::ConflictAction::Overwrite,
                apply_to_remaining: false,
            }
        ),
        Err(ResolveError::Mismatched),
        "a conflict answer must not resolve a password question"
    );

    hub.prompts().deny_session(sid);
    assert!(matches!(asking.await.unwrap(), PromptReply::Deny));
}

#[tokio::test]
async fn closing_a_session_denies_its_open_prompts() {
    let hub = hub();
    let sid = Uuid::new_v4();
    let other = Uuid::new_v4();
    let prompts = std::sync::Arc::clone(hub.prompts());

    let asking = tokio::spawn({
        let prompts = std::sync::Arc::clone(&prompts);
        async move {
            prompts
                .ask(sid, Prompt::Password { hint: "a".into() })
                .await
        }
    });
    let untouched = tokio::spawn({
        let prompts = std::sync::Arc::clone(&prompts);
        async move {
            prompts
                .ask(other, Prompt::Password { hint: "b".into() })
                .await
        }
    });

    tokio::time::timeout(Duration::from_secs(2), async {
        while hub.prompts().pending().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    hub.prompts().deny_session(sid);
    assert!(matches!(asking.await.unwrap(), PromptReply::Deny));
    assert_eq!(
        hub.prompts().pending().len(),
        1,
        "other sessions keep their prompts"
    );

    hub.prompts().deny_session(other);
    assert!(matches!(untouched.await.unwrap(), PromptReply::Deny));
}

#[tokio::test]
async fn a_slow_listing_cannot_replace_a_newer_directory() {
    let hub = hub();
    let sub = hub.subscribe();
    let sid = Uuid::new_v4();
    hub.events()
        .send(EngineEvent::SessionOpened {
            session: Box::new(session(sid)),
        })
        .await
        .unwrap();

    // The user navigated to /var/www (request 2) while /home/deploy (request 1) was
    // still in flight; the late arrival must not win.
    hub.events()
        .send(EngineEvent::Listing {
            listing: listing(sid, "/var/www", 2),
        })
        .await
        .unwrap();
    hub.events()
        .send(EngineEvent::Listing {
            listing: listing(sid, "/home/deploy", 1),
        })
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let snap = hub.snapshot(sub.id).expect("snapshot");
            if snap.listings.first().is_some_and(|l| l.request == Seq(2)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("newer listing stayed");

    let snap = hub.snapshot(sub.id).unwrap();
    assert_eq!(snap.listings[0].path, "/var/www");
    assert_eq!(snap.sessions[0].remote_path.as_deref(), Some("/var/www"));
}

#[tokio::test]
async fn progress_is_coalesced_but_the_terminal_state_always_arrives() {
    let hub = hub();
    let mut sub = hub.subscribe();
    let sid = Uuid::new_v4();
    let jid = Uuid::new_v4();
    hub.events()
        .send(EngineEvent::SessionOpened {
            session: Box::new(session(sid)),
        })
        .await
        .unwrap();

    for transferred in 1..=200u64 {
        hub.events()
            .send(EngineEvent::JobUpdate {
                job: job(jid, sid, JobState::Transferring, transferred * 5),
            })
            .await
            .unwrap();
    }
    hub.events()
        .send(EngineEvent::JobUpdate {
            job: job(jid, sid, JobState::Done { at: Utc::now() }, 1000),
        })
        .await
        .unwrap();

    let envelopes = drain(&mut sub.rx, 400).await;
    let job_updates: Vec<_> = envelopes
        .iter()
        .filter(|e| matches!(e.update, EngineEvent::JobUpdate { .. }))
        .collect();

    assert!(
        job_updates.len() < 200,
        "progress should be coalesced, saw {} updates",
        job_updates.len()
    );
    let last = job_updates.last().expect("at least one job update");
    match &last.update {
        EngineEvent::JobUpdate { job } => {
            assert!(
                matches!(job.state, JobState::Done { .. }),
                "final state must not be dropped"
            );
            assert_eq!(job.transferred, Bytes(1000));
        }
        other => panic!("unexpected {other:?}"),
    }

    // Sequence numbers are contiguous: coalescing happens before they are assigned.
    let seqs: Vec<u64> = envelopes.iter().map(|e| e.seq.get()).collect();
    assert!(
        seqs.windows(2).all(|w| w[1] == w[0] + 1),
        "the stream must have no gaps"
    );
}
