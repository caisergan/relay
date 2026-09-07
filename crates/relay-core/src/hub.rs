//! The engine's front door: everything the shell holds onto.
//!
//! `EngineHub::start` takes a `tokio::runtime::Handle` and spawns on it. Nothing here
//! imports Tauri — `cargo tree -p relay-core` proving that is a phase 0 exit criterion.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::coordinator::{
    Coordinator, EngineSnapshot, SnapshotError, Subscription, SubscriptionId,
};
use crate::events::EngineEvent;
use crate::interact::PromptBroker;
use crate::job::JobSnapshot;

/// Engine → coordinator queue depth. Bounded: a wedged pump must apply backpressure
/// to whoever is generating events, not grow without limit.
pub const EVENT_BUFFER: usize = 1024;
/// Progress coalescing window. Phase 5 budgets ten job updates per second.
pub const PROGRESS_FLUSH: Duration = Duration::from_millis(100);
/// How long shutdown waits for the pump to drain before giving up on it.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

pub struct EngineHub {
    coordinator: Arc<Coordinator>,
    prompts: Arc<PromptBroker>,
    events: mpsc::Sender<EngineEvent>,
    cancel: CancellationToken,
    pump: Mutex<Option<JoinHandle<()>>>,
}

impl EngineHub {
    pub fn start(rt: &tokio::runtime::Handle) -> Arc<Self> {
        let (tx, rx) = mpsc::channel(EVENT_BUFFER);
        let coordinator = Arc::new(Coordinator::new());
        let prompts = Arc::new(PromptBroker::new(tx.clone()));
        let cancel = CancellationToken::new();

        let pump = rt.spawn(pump(rx, Arc::clone(&coordinator), cancel.clone()));

        Arc::new(Self {
            coordinator,
            prompts,
            events: tx,
            cancel,
            pump: Mutex::new(Some(pump)),
        })
    }

    pub fn coordinator(&self) -> &Arc<Coordinator> {
        &self.coordinator
    }

    pub fn prompts(&self) -> &Arc<PromptBroker> {
        &self.prompts
    }

    /// Sender for engine tasks. Cloning is the intended way to hand it to a session.
    pub fn events(&self) -> mpsc::Sender<EngineEvent> {
        self.events.clone()
    }

    pub fn subscribe(&self) -> Subscription {
        self.coordinator.subscribe()
    }

    /// Snapshot including unanswered prompts — the form the shell exposes.
    pub fn snapshot(&self, subscription: SubscriptionId) -> Result<EngineSnapshot, SnapshotError> {
        self.coordinator
            .snapshot_with_prompts(subscription, self.prompts.pending())
    }

    /// Stop the pump, letting it flush what it already has.
    pub async fn shutdown(&self) {
        self.cancel.cancel();
        let handle = self.pump.lock().expect("pump handle poisoned").take();
        if let Some(handle) = handle {
            match tokio::time::timeout(SHUTDOWN_GRACE, handle).await {
                Ok(_) => {}
                Err(_) => tracing::warn!("engine pump did not stop within the grace period"),
            }
        }
    }
}

/// Sequence events into the coordinator, coalescing progress-only job updates.
///
/// Coalescing happens *before* a sequence number is assigned, so the ordered stream
/// never contains a gap. A state change (including every terminal state) supersedes
/// any pending progress for that job, which is what guarantees a final update always
/// reaches the UI.
async fn pump(
    mut rx: mpsc::Receiver<EngineEvent>,
    coordinator: Arc<Coordinator>,
    cancel: CancellationToken,
) {
    let mut pending: HashMap<Uuid, Box<JobSnapshot>> = HashMap::new();
    let mut ticker = tokio::time::interval(PROGRESS_FLUSH);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            event = rx.recv() => {
                let Some(event) = event else { break };
                match event {
                    EngineEvent::JobUpdate { job } if event_is_progress(&job) => {
                        pending.insert(job.id, job);
                    }
                    EngineEvent::JobUpdate { job } => {
                        pending.remove(&job.id);
                        coordinator.publish(EngineEvent::JobUpdate { job });
                    }
                    other => {
                        coordinator.publish(other);
                    }
                }
            }
            _ = ticker.tick() => flush(&mut pending, &coordinator),
            _ = cancel.cancelled() => break,
        }
    }

    // Drain whatever is already queued so a shutdown cannot swallow a final state.
    rx.close();
    while let Some(event) = rx.recv().await {
        if let EngineEvent::JobUpdate { job } = &event {
            pending.remove(&job.id);
        }
        coordinator.publish(event);
    }
    flush(&mut pending, &coordinator);
}

fn event_is_progress(job: &JobSnapshot) -> bool {
    matches!(job.state, crate::job::JobState::Transferring)
}

fn flush(pending: &mut HashMap<Uuid, Box<JobSnapshot>>, coordinator: &Coordinator) {
    for (_, job) in pending.drain() {
        coordinator.publish(EngineEvent::JobUpdate { job });
    }
}
