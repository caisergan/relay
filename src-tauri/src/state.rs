//! What the shell owns. Everything durable belongs to `relay-core`; this is the
//! adapter's own bookkeeping.

use std::collections::HashMap;
use std::sync::Arc;

use relay_core::engine::{Engine, EnginePaths};
use relay_core::hub::EngineHub;
use tauri::Manager;
use tauri::async_runtime::JoinHandle;
use tokio::sync::RwLock;
use uuid::Uuid;

pub struct AppState {
    pub hub: Arc<EngineHub>,
    /// The engine. Phase 0's mock-backed stand-in is gone: this opens real sessions.
    pub engine: Arc<Engine>,
    /// Forwarder task per subscription, so unsubscribing actually stops the work.
    /// Tauri's `JoinHandle`, not tokio's: they are distinct types and the shell spawns
    /// through Tauri's runtime.
    pub forwarders: RwLock<HashMap<Uuid, JoinHandle<()>>>,
}

impl AppState {
    /// Starts the engine. Named `start` rather than `new` because it spawns the pump
    /// and needs a live Tauri async runtime — a `Default` that panics off-runtime
    /// would be worse than an honest name.
    pub fn start(app: &tauri::AppHandle) -> Self {
        let rt = tauri::async_runtime::handle().inner().clone();
        let hub = EngineHub::start(&rt);
        let engine = Arc::new(Engine::new(Arc::clone(&hub), rt, paths(app)));
        Self {
            hub,
            engine,
            forwarders: RwLock::new(HashMap::new()),
        }
    }

    /// Stop every forwarder, close every session, and let the engine drain. Called on
    /// window close, before the process goes away.
    pub async fn shutdown(&self) {
        let handles: Vec<JoinHandle<()>> = self
            .forwarders
            .write()
            .await
            .drain()
            .map(|(_, h)| h)
            .collect();
        for handle in handles {
            handle.abort();
        }
        self.engine.shutdown().await;
    }
}

/// Where the server list and the trust store live.
///
/// Falling back to the current directory is deliberate over refusing to start: a
/// missing app-data directory is an unusual environment, not a reason to make the app
/// unusable. Both stores tolerate a path they cannot write, and say so in the log.
fn paths(app: &tauri::AppHandle) -> EnginePaths {
    let dir = app.path().app_data_dir().unwrap_or_else(|err| {
        tracing::warn!(%err, "no app data directory; falling back to the working directory");
        std::path::PathBuf::from(".")
    });
    EnginePaths {
        servers: dir.join("servers.json"),
        trust: dir.join("trust.json"),
    }
}
