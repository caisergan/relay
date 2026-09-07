//! The scheduler under a load no hand-written test would produce.
//!
//! Five hundred jobs across three sessions, finishing and failing in an order chosen
//! by a seeded generator, with pauses, resumes and reorders thrown in while they run.
//! The seed is fixed, so a failure here is a failure anyone can reproduce; nothing in
//! this file depends on timing or on a real server.
//!
//! What it asserts is deliberately narrow, because these are the properties that must
//! hold *whatever* the sequence of events was:
//!
//! - The concurrency limit is never exceeded, globally or per session. A cap that holds
//!   in the quiet cases and not under pressure is not a cap.
//! - Every job reaches a terminal state. A job that gets stuck is invisible in a
//!   drawer showing four hundred others, and it is the failure a queue is judged on.
//! - Nothing is dispatched twice at once, and nothing is dispatched to a session that
//!   is down.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use relay_core::error::{EngineError, Result};
use relay_core::job::{JobKind, JobState, PauseReason, QueueOp};
use relay_core::model::{Direction, JobId, SessionId};
use relay_core::queue::{JobSpec, ResumeRecord};
use relay_core::scheduler::{Dispatcher, Report, RunRequest, Scheduler, SchedulerContext};
use relay_core::store::QueueStore;
use relay_core::wire::Bytes;
use tokio::sync::mpsc;
use uuid::Uuid;

/// The global concurrency limit the queue starts at.
const CONCURRENCY: u8 = 4;
/// The highest the test ever moves the slider to, and therefore the bound the peak is
/// checked against.
const HIGHEST: u8 = 8;
/// What each session's backend says it can take. Deliberately lower than the global
/// limit for one of them, so both caps are actually exercised.
const LANES: [u8; 3] = [8, 2, 3];
const JOBS: usize = 500;

/// xorshift64*. A generator rather than a dependency: the test needs *a* deterministic
/// sequence, not a good one, and a fixed seed is the whole point.
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound.max(1) as u64) as usize
    }
}

#[derive(Default)]
struct Bench {
    /// What is running right now, and on which session.
    inflight: Mutex<HashMap<JobId, SessionId>>,
    /// Every job ever dispatched, to catch a job started twice at once.
    peak_global: AtomicUsize,
    peak_by_session: Mutex<HashMap<SessionId, usize>>,
    /// Sessions the test has taken down, so a dispatch to one is a rule broken.
    down: Mutex<Vec<SessionId>>,
    double_dispatch: Mutex<Vec<JobId>>,
    to_a_dead_session: Mutex<Vec<JobId>>,
    reporters: Mutex<HashMap<JobId, RunRequest>>,
}

#[async_trait]
impl Dispatcher for Bench {
    async fn dispatch(&self, run: RunRequest) -> Result<()> {
        {
            let mut inflight = self.inflight.lock().unwrap();
            if inflight.contains_key(&run.job) {
                self.double_dispatch.lock().unwrap().push(run.job);
            }
            if self.down.lock().unwrap().contains(&run.session) {
                self.to_a_dead_session.lock().unwrap().push(run.job);
            }
            inflight.insert(run.job, run.session);

            self.peak_global.fetch_max(inflight.len(), Ordering::SeqCst);
            let mut per = self.peak_by_session.lock().unwrap();
            let here = inflight.values().filter(|s| **s == run.session).count();
            let slot = per.entry(run.session).or_default();
            *slot = (*slot).max(here);
        }
        self.reporters.lock().unwrap().insert(run.job, run);
        Ok(())
    }

    async fn abort(&self, _session: SessionId, job: JobId, _keep_partial: bool) {
        // A real transfer reports its own cancellation; this stands in for that, so a
        // paused job's slot is released exactly as it would be in the engine.
        let run = {
            self.inflight.lock().unwrap().remove(&job);
            self.reporters.lock().unwrap().remove(&job)
        };
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

impl Bench {
    fn running(&self) -> Vec<JobId> {
        self.inflight.lock().unwrap().keys().copied().collect()
    }

    /// Finish one job the way a transfer would: it started, then it ended.
    async fn finish(&self, job: JobId, result: std::result::Result<Bytes, EngineError>) {
        let run = {
            self.inflight.lock().unwrap().remove(&job);
            self.reporters.lock().unwrap().remove(&job)
        };
        let Some(run) = run else { return };
        run.report
            .send(Report::Started {
                job,
                size: Some(Bytes(1024)),
                resume_from: Bytes::ZERO,
                record: ResumeRecord::new(
                    relay_core::model::FileFacts {
                        path: run.remote_path.clone(),
                        size: Bytes(1024),
                        modified: None,
                        digest: None,
                    },
                    format!("{}.relaypart", run.local_path.display()),
                ),
            })
            .await;
        run.report.send(Report::Finished { job, result }).await;
    }
}

fn spec(session: SessionId, server_id: Uuid, index: usize) -> JobSpec {
    JobSpec {
        session,
        server_id,
        kind: JobKind::File,
        direction: if index.is_multiple_of(2) {
            Direction::Down
        } else {
            Direction::Up
        },
        remote_path: format!("/remote/f{index}"),
        local_path: PathBuf::from(format!("/local/f{index}")),
        size: Some(Bytes(1024)),
        parent: None,
        item: format!("f{index}"),
    }
}

/// Let the scheduler's task run, then read its state. A snapshot round trip is the
/// synchronisation: it cannot answer until everything queued before it is applied.
async fn settle(scheduler: &Scheduler) {
    for _ in 0..4 {
        tokio::task::yield_now().await;
        let _ = scheduler.snapshot().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn five_hundred_jobs_finish_without_breaking_a_cap_or_stranding_one() {
    let bench = Arc::new(Bench::default());
    let (events, mut drain) = mpsc::channel(4096);
    // The event stream is not what this test is about, but an unread channel would
    // apply backpressure to the scheduler and change the thing being measured.
    tokio::spawn(async move { while drain.recv().await.is_some() {} });

    let scheduler = Scheduler::spawn(SchedulerContext {
        store: QueueStore::in_memory().await.unwrap(),
        events,
        dispatcher: Arc::clone(&bench) as Arc<dyn Dispatcher>,
        rt: tokio::runtime::Handle::current(),
        concurrency: CONCURRENCY,
    })
    .await
    .unwrap();

    let sessions: Vec<SessionId> = LANES.iter().map(|_| Uuid::new_v4()).collect();
    // One server each, so a session going down and coming back adopts its own work and
    // not somebody else's.
    let servers: Vec<Uuid> = LANES.iter().map(|_| Uuid::new_v4()).collect();
    for ((session, server), lanes) in sessions.iter().zip(&servers).zip(LANES) {
        scheduler.session_up(*session, *server, lanes).await;
    }

    let mut rng = Seeded(0x5EED_C0FFEE);
    let batch = Uuid::new_v4();
    let specs: Vec<JobSpec> = (0..JOBS)
        .map(|i| {
            let at = i % sessions.len();
            spec(sessions[at], servers[at], i)
        })
        .collect();
    let ids = scheduler.enqueue(batch, specs).await.unwrap();
    assert_eq!(ids.len(), JOBS);

    // Drive it. Each round finishes some of what is running, sometimes badly, and
    // sometimes interferes with the queue while it does.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    let mut rounds = 0usize;
    let mut paused_by_hand: Vec<JobId> = Vec::new();
    loop {
        settle(&scheduler).await;
        let snapshot = scheduler.snapshot().await;
        if snapshot.iter().all(|job| job.state.is_terminal()) {
            break;
        }
        // Once nothing is left but what this test paused, put it all back and let the
        // queue drain. Anything still stuck after that is stuck because the engine
        // stranded it.
        if snapshot
            .iter()
            .filter(|job| !job.state.is_terminal())
            .all(|job| paused_by_hand.contains(&job.id))
        {
            for job in paused_by_hand.drain(..) {
                let _ = scheduler.control(QueueOp::Resume { job }).await;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the queue stopped making progress with {} jobs unfinished",
            snapshot.iter().filter(|j| !j.state.is_terminal()).count()
        );

        for job in bench.running() {
            match rng.below(10) {
                // Most transfers work.
                0..=5 => bench.finish(job, Ok(Bytes(1024))).await,
                // A failure a person has to deal with: terminal on the first attempt.
                6 | 7 => {
                    bench
                        .finish(
                            job,
                            Err(EngineError::PermissionDenied {
                                path: "/remote".into(),
                            }),
                        )
                        .await
                }
                // One the scheduler retries on its own, so the backoff and the attempt
                // cap are exercised rather than merely present.
                8 => {
                    bench
                        .finish(job, Err(EngineError::network("reset by peer")))
                        .await
                }
                // And one left running into the next round, so the caps are under
                // real pressure rather than emptying every time.
                _ => {}
            }
        }

        // Meddle. A queue nobody touches while it runs is not the queue people use.
        match rng.below(12) {
            0 => {
                if let Some(job) = pick(&mut rng, &scheduler).await {
                    let _ = scheduler.control(QueueOp::Pause { job }).await;
                    // Recorded only if it really became *user*-paused. Pausing a job
                    // the engine had already paused is a no-op, and recording it would
                    // hand this test a way to resume something the engine stranded —
                    // which is precisely the failure it exists to notice.
                    if matches!(
                        scheduler.state(job).await,
                        Some(JobState::Paused {
                            reason: PauseReason::User
                        })
                    ) {
                        paused_by_hand.push(job);
                    }
                }
            }
            // Only what this test paused. Resuming an arbitrary job would quietly
            // rescue one the *engine* had stranded — and stranding is half of what
            // this test is here to catch, so it must not be able to fix it by luck.
            1 => {
                if !paused_by_hand.is_empty() {
                    let job = paused_by_hand.remove(rng.below(paused_by_hand.len()));
                    let _ = scheduler.control(QueueOp::Resume { job }).await;
                }
            }
            2 => {
                if let Some(job) = pick(&mut rng, &scheduler).await {
                    let _ = scheduler
                        .control(QueueOp::Reorder { job, after: None })
                        .await;
                }
            }
            3 => {
                scheduler
                    .set_concurrency(1 + rng.below(HIGHEST as usize) as u8)
                    .await
            }
            // A session drops and comes back. Its jobs must pause and resume, not fail.
            4 => {
                let index = rng.below(sessions.len());
                let session = sessions[index];
                // Told first, and only marked down once the scheduler has processed
                // it. Marking it before would forbid dispatches that were still
                // perfectly legal, and the test would be checking its own ordering
                // rather than the engine's.
                scheduler.session_down(session).await;
                settle(&scheduler).await;
                bench.down.lock().unwrap().push(session);
                settle(&scheduler).await;
                bench.down.lock().unwrap().retain(|s| *s != session);
                scheduler
                    .session_up(session, servers[index], LANES[index])
                    .await;
            }
            _ => {}
        }

        rounds += 1;
        assert!(
            rounds < 10_000,
            "far more rounds than this can possibly need"
        );
    }

    // Nothing may be waiting on a decision nobody is going to make.
    let snapshot = scheduler.snapshot().await;
    assert_eq!(
        snapshot.len(),
        JOBS,
        "no job vanished and none was duplicated"
    );
    assert!(snapshot.iter().all(|job| job.state.is_terminal()));

    assert!(
        bench.double_dispatch.lock().unwrap().is_empty(),
        "a job was dispatched while it was already running: {:?}",
        bench.double_dispatch.lock().unwrap()
    );
    assert!(
        bench.to_a_dead_session.lock().unwrap().is_empty(),
        "work was dispatched to a session that was down: {:?}",
        bench.to_a_dead_session.lock().unwrap()
    );

    // The caps, measured at the moment of dispatch across the whole run. The bound is
    // the highest the slider was ever set to rather than the value it started at,
    // because the test moves it.
    let peak = bench.peak_global.load(Ordering::SeqCst);
    assert!(
        peak <= usize::from(HIGHEST),
        "the global limit was exceeded: {peak} transfers at once"
    );
    let per = bench.peak_by_session.lock().unwrap();
    for (session, lanes) in sessions.iter().zip(LANES) {
        let seen = per.get(session).copied().unwrap_or(0);
        assert!(
            seen <= usize::from(lanes),
            "session with {lanes} lanes ran {seen} transfers at once"
        );
    }
}

/// Some job in the queue, chosen by the generator.
async fn pick(rng: &mut Seeded, scheduler: &Scheduler) -> Option<JobId> {
    let snapshot = scheduler.snapshot().await;
    let alive: Vec<JobId> = snapshot
        .iter()
        .filter(|job| !job.state.is_terminal())
        .map(|job| job.id)
        .collect();
    (!alive.is_empty()).then(|| alive[rng.below(alive.len())])
}
