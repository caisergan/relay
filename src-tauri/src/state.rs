//! What the shell owns. Everything durable belongs to `relay-core`; this is the
//! adapter's own bookkeeping.

use std::collections::HashMap;
use std::sync::Arc;

use relay_core::demo::DemoEngine;
use relay_core::hub::EngineHub;
use relay_core::model::{ServerConfig, ServerId};
use relay_core::settings::Settings;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use uuid::Uuid;

pub struct AppState {
    pub hub: Arc<EngineHub>,
    /// Phase 0's engine. Phase 1 swaps this for the session manager; the command
    /// surface above it stays the same, which is the point of the exercise.
    pub engine: Arc<DemoEngine>,
    /// Saved servers. Phase 1 persists these to `servers.json` alongside the trust
    /// store — never with secrets, which live in the OS keychain.
    pub servers: RwLock<Vec<ServerConfig>>,
    pub settings: RwLock<Settings>,
    /// Forwarder task per subscription, so unsubscribing actually stops the work.
    pub forwarders: RwLock<HashMap<Uuid, JoinHandle<()>>>,
}

impl AppState {
    pub fn new() -> Self {
        let rt = tauri::async_runtime::handle().inner().clone();
        let hub = EngineHub::start(&rt);
        let engine = Arc::new(DemoEngine::new(Arc::clone(&hub), rt));
        Self {
            hub,
            engine,
            servers: RwLock::new(demo_servers()),
            settings: RwLock::new(Settings::default()),
            forwarders: RwLock::new(HashMap::new()),
        }
    }

    pub async fn server(&self, id: ServerId) -> Option<ServerConfig> {
        self.servers
            .read()
            .await
            .iter()
            .find(|s| s.id == id)
            .cloned()
    }

    /// Stop every forwarder and let the engine drain. Called on window close.
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
        self.hub.shutdown().await;
    }
}

/// Two servers so the sidebar, the tabs, and the connect flow all have something to
/// render before any real credentials exist.
fn demo_servers() -> Vec<ServerConfig> {
    use relay_core::model::{AuthMethod, Proto};
    vec![
        ServerConfig {
            id: Uuid::new_v4(),
            name: "staging".into(),
            host: "staging.example".into(),
            port: 22,
            proto: Proto::Sftp,
            username: "deploy".into(),
            auth: AuthMethod::Agent,
            color: Some("#2456E6".into()),
            group: Some("Work".into()),
            bookmarks: vec![relay_core::model::Bookmark {
                label: "Web root".into(),
                remote_path: "/var/www".into(),
            }],
            initial_remote_path: Some("/var/www".into()),
        },
        ServerConfig {
            id: Uuid::new_v4(),
            name: "backups".into(),
            host: "backups.example".into(),
            port: 2222,
            proto: Proto::Sftp,
            username: "root".into(),
            auth: AuthMethod::KeyFile {
                path: "~/.ssh/id_ed25519".into(),
            },
            color: Some("#2E9E5B".into()),
            group: Some("Work".into()),
            bookmarks: Vec::new(),
            initial_remote_path: Some("/home/deploy".into()),
        },
    ]
}
