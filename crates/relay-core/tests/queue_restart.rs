//! What the queue looks like after the process goes away.
//!
//! Two different endings are covered, and the difference between them is the point:
//!
//! - A **clean shutdown** flushes and stops. What was running comes back paused.
//! - A **forced termination** — no exit hook, no flush — is what a crash or a `kill -9`
//!   looks like, and the queue has to be equally intact afterwards. The exit path is
//!   best effort; it is not the recovery mechanism, and a test that only ever closes
//!   politely would never find out.
//!
//! Neither goes near a network. The scheduler's dispatcher is a stand-in, because what
//! is being tested is the database and the recovery pass over it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use relay_core::error::{EngineError, Result};
use relay_core::interact::PromptBroker;
use relay_core::job::{JobKind, JobState, PauseReason};
use relay_core::model::{Direction, FileFacts, JobId, SessionId};
use relay_core::queue::{Job, JobSpec, ResumeRecord};
use relay_core::scheduler::{Dispatcher, Report, RunRequest, Scheduler, SchedulerContext};
use relay_core::store::QueueStore;
use relay_core::wire::Bytes;
use tokio::sync::mpsc;
use uuid::Uuid;

#[derive(Default)]
struct Held {
    running: Mutex<HashMap<JobId, RunRequest>>,
}

#[async_trait]
impl Dispatcher for Held {
    async fn dispatch(&self, run: RunRequest) -> Result<()> {
        self.running.lock().unwrap().insert(run.job, run);
        Ok(())
    }

    async fn abort(&self, _session: SessionId, job: JobId, _keep: bool) {
        let run = self.running.lock().unwrap().remove(&job);
        if let Some(run) = run {
            run.report
                .send(Report::Finished {
                    job,
                    result: Err(EngineError::Cancelled),
                })
                .await;
        }
    }
}

impl Held {
    /// Report a start, the way a transfer does before it writes a byte, so the job
    /// gets an ownership record on disk.
    async fn start(&self, job: JobId) {
        let run = self.running.lock().unwrap().get(&job).cloned();
        let Some(run) = run else { return };
        run.report
            .started(Report::Started {
                job,
                size: Some(Bytes(4096)),
                resume_from: Bytes::ZERO,
                record: ResumeRecord::new(
                    FileFacts {
                        path: run.remote_path.clone(),
                        size: Bytes(4096),
                        modified: None,
                        digest: None,
                    },
                    format!("{}.relaypart", run.local_path.display()),
                ),
            })
            .await;
    }

    /// Commit a verified checkpoint, as the transfer loop does every few megabytes.
    async fn checkpoint(&self, job: JobId, at: u64, digest: &str) {
        let run = self.running.lock().unwrap().get(&job).cloned();
        if let Some(run) = run {
            run.report
                .send(Report::Checkpoint {
                    job,
                    offset: Bytes(at),
                    digest: digest.to_string(),
                })
                .await;
        }
    }

    fn ids(&self) -> Vec<JobId> {
        self.running.lock().unwrap().keys().copied().collect()
    }
}

fn spec(session: SessionId, name: &str) -> JobSpec {
    JobSpec {
        session,
        server_id: SERVER,
        kind: JobKind::File,
        direction: Direction::Down,
        remote_path: format!("/remote/{name}"),
        local_path: PathBuf::from(format!("/local/{name}")),
        size: Some(Bytes(4096)),
        parent: None,
        item: name.into(),
    }
}

struct Running {
    scheduler: Scheduler,
    held: Arc<Held>,
    session: SessionId,
}

/// The server the queue belongs to. Fixed across restarts, unlike the session id,
/// which is what makes a restored queue findable at all.
const SERVER: Uuid = Uuid::from_u128(0x5E5_0001);

async fn start(store: QueueStore) -> Running {
    let held = Arc::new(Held::default());
    let (events, mut drain) = mpsc::channel(1024);
    let (prompts, _unread) = mpsc::channel(64);
    tokio::spawn(async move { while drain.recv().await.is_some() {} });

    let scheduler = Scheduler::spawn(SchedulerContext {
        store,
        events,
        dispatcher: Arc::clone(&held) as Arc<dyn Dispatcher>,
        prompts: Arc::new(PromptBroker::new(prompts)),
        rt: tokio::runtime::Handle::current(),
        concurrency: 3,
        default_conflict: None,
    })
    .await
    .unwrap();

    let session = Uuid::new_v4();
    scheduler.session_up(session, SERVER, 4).await;
    Running {
        scheduler,
        held,
        session,
    }
}

async fn settle(scheduler: &Scheduler) {
    for _ in 0..6 {
        tokio::task::yield_now().await;
        let _ = scheduler.snapshot().await;
    }
}

/// The exit criterion: restart mid-queue and find the queue.
#[tokio::test]
async fn a_queue_interrupted_by_a_restart_comes_back_paused_and_resumable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("relay.sqlite");
    let batch = Uuid::new_v4();

    // ---- the first run ------------------------------------------------------
    let first = start(QueueStore::open(&path).await.unwrap()).await;
    let names = ["a.bin", "b.bin", "c.bin", "d.bin", "e.bin"];
    let queued = first
        .scheduler
        .enqueue(
            batch,
            names.iter().map(|n| spec(first.session, n)).collect(),
        )
        .await
        .unwrap();
    settle(&first.scheduler).await;

    // Three run (the concurrency limit), and one of them gets far enough to record a
    // verified checkpoint.
    for job in first.held.ids() {
        first.held.start(job).await;
    }
    settle(&first.scheduler).await;
    let checkpointed = first.held.ids()[0];
    first
        .held
        .checkpoint(checkpointed, 2048, "deadbeef".repeat(8).as_str())
        .await;
    settle(&first.scheduler).await;

    // Forced termination: no `shutdown`, no flush, nothing given a chance to tidy up.
    // The database has to be enough on its own.
    drop(first);

    // ---- the second run -----------------------------------------------------
    let second = start(QueueStore::open(&path).await.unwrap()).await;
    let restored = second.scheduler.snapshot().await;

    assert_eq!(restored.len(), names.len(), "the whole queue came back");
    let by_name: HashMap<String, _> = restored
        .iter()
        .map(|job| (job.remote_path.clone(), job.clone()))
        .collect();
    for name in names {
        assert!(by_name.contains_key(&format!("/remote/{name}")), "{name}");
    }

    // Whatever was running is paused for verification, not restarted and not believed.
    let interrupted: Vec<_> = restored
        .iter()
        .filter(|job| {
            matches!(
                job.state,
                JobState::Paused {
                    reason: PauseReason::Restarted
                }
            )
        })
        .collect();
    assert_eq!(
        interrupted.len(),
        3,
        "the three that were running come back paused: {:?}",
        restored.iter().map(|j| &j.state).collect::<Vec<_>>()
    );

    // The checkpoint survived the crash, on the job that recorded it and no other.
    let stored = QueueStore::open(&path)
        .await
        .unwrap()
        .load_all()
        .await
        .unwrap();
    let record = record_for(&stored, checkpointed).expect("the checkpointed job's record");
    assert_eq!(record.checkpoint, Bytes(2048));
    assert_eq!(record.prefix_sha256, "deadbeef".repeat(8));
    for job in &stored {
        if job.id != checkpointed {
            assert!(
                record_for(&stored, job.id).is_none_or(|r| r.checkpoint == Bytes::ZERO),
                "only the job that checkpointed has a non-zero one"
            );
        }
    }

    // And the queue is resumable: everything runs again once someone says so.
    second
        .scheduler
        .control(relay_core::job::QueueOp::ResumeAll)
        .await
        .unwrap();
    settle(&second.scheduler).await;
    assert_eq!(
        second.held.ids().len(),
        3,
        "the restored queue picks up where it left off, still under the limit"
    );

    // Nothing was queued twice by coming back.
    let ids: Vec<JobId> = second
        .scheduler
        .snapshot()
        .await
        .iter()
        .map(|j| j.id)
        .collect();
    assert_eq!(ids.len(), queued.len());
    for id in &queued {
        assert!(
            ids.contains(id),
            "a job changed identity across the restart"
        );
    }
}

/// Re-sending a gesture after a restart must find the jobs it already made, not queue
/// the folder a second time. The deduplication key is in the database, which is the
/// only party that still remembers the first attempt.
#[tokio::test]
async fn an_enqueue_replayed_after_a_restart_finds_the_jobs_it_already_made() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("relay.sqlite");
    let batch = Uuid::new_v4();

    let first = start(QueueStore::open(&path).await.unwrap()).await;
    let session = first.session;
    let before = first
        .scheduler
        .enqueue(batch, vec![spec(session, "a.bin"), spec(session, "b.bin")])
        .await
        .unwrap();
    settle(&first.scheduler).await;
    drop(first);

    let second = start(QueueStore::open(&path).await.unwrap()).await;
    let again = second
        .scheduler
        .enqueue(
            batch,
            vec![spec(second.session, "a.bin"), spec(second.session, "b.bin")],
        )
        .await
        .unwrap();
    settle(&second.scheduler).await;

    assert_eq!(again, before, "the replay found the original jobs");
    assert_eq!(
        second.scheduler.snapshot().await.len(),
        2,
        "and did not queue the same two files twice"
    );
}

/// A clean shutdown is the ordinary case, and the displayed progress it flushes is the
/// number the drawer draws on the next launch.
#[tokio::test]
async fn a_clean_shutdown_flushes_the_progress_it_was_holding() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("relay.sqlite");

    let first = start(QueueStore::open(&path).await.unwrap()).await;
    let ids = first
        .scheduler
        .enqueue(Uuid::new_v4(), vec![spec(first.session, "a.bin")])
        .await
        .unwrap();
    settle(&first.scheduler).await;
    first.held.start(ids[0]).await;
    settle(&first.scheduler).await;

    let run = first.held.running.lock().unwrap().get(&ids[0]).cloned();
    run.expect("dispatched")
        .report
        .progress(ids[0], Bytes(3333));
    settle(&first.scheduler).await;
    first.scheduler.shutdown().await;
    drop(first);

    let stored = QueueStore::open(&path)
        .await
        .unwrap()
        .load_all()
        .await
        .unwrap();
    assert_eq!(
        stored[0].transferred,
        Bytes(3333),
        "progress held in memory reached the disk on the way out"
    );
}

fn record_for(jobs: &[Job], id: JobId) -> Option<ResumeRecord> {
    jobs.iter()
        .find(|job| job.id == id)
        .and_then(|job| job.resume.clone())
}
