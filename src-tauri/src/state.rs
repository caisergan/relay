//! What the shell owns. Everything durable belongs to `relay-core`; this is the
//! adapter's own bookkeeping.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use relay_core::engine::{Engine, EnginePaths};
use relay_core::hub::EngineHub;
use relay_core::layout::{LayoutStore, Screen, WindowGeometry};
use relay_core::workspace::WorkspaceStore;
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
    /// The open tabs and their folders, for the "continue where I left off" launch.
    /// The shell's rather than the engine's: the engine knows sessions, but not which
    /// folder the local pane was in or which tab was showing.
    pub workspace: WorkspaceStore,
    /// How the window was arranged; see `relay_core::layout`.
    pub layout: LayoutStore,
    /// The window's size and place as of its last move, held here rather than written on
    /// every step of a drag, and recorded once as the app exits.
    pub window: Mutex<Option<WindowGeometry>>,
}

impl AppState {
    /// Starts the engine. Named `start` rather than `new` because it spawns the pump
    /// and needs a live Tauri async runtime — a `Default` that panics off-runtime
    /// would be worse than an honest name.
    ///
    /// Asynchronous since phase 2: the queue database is opened and migrated here, and
    /// the transfer queue is restored from it before anything can be enqueued.
    pub async fn start(app: &tauri::AppHandle) -> Result<Self, relay_core::EngineError> {
        let rt = tauri::async_runtime::handle().inner().clone();
        let hub = EngineHub::start(&rt);
        let engine = Arc::new(Engine::new(Arc::clone(&hub), rt, paths(app)).await?);
        let dir = data_dir(app);
        let layout = LayoutStore::load(dir.join("layout.json"));
        Ok(Self {
            hub,
            engine,
            forwarders: RwLock::new(HashMap::new()),
            workspace: WorkspaceStore::load(dir.join("workspace.json")),
            window: Mutex::new(layout.get().window),
            layout,
        })
    }

    /// Stop every forwarder, close every session, and let the engine drain. Called on
    /// window close, before the process goes away.
    pub async fn shutdown(&self) {
        // The window's last size and place, while nothing has been torn down yet.
        let window = *self.window.lock().expect("window geometry poisoned");
        if let Some(window) = window
            && let Err(err) = self.layout.update(|layout| layout.window = Some(window))
        {
            tracing::warn!(%err, "the window's size and place were not recorded");
        }
        // Then, before anything closes: every session this is about to end would
        // otherwise be reported as a closed tab, and recorded as the place to return to.
        self.layout.freeze();
        self.workspace.freeze();
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

    /// Note the window's size and place as it changes. In memory only; see `window`.
    pub fn note_window(&self, window: &tauri::Window) {
        // Minimised or full screen is not an arrangement to come back to. The one before
        // it is, and is already noted.
        if window.is_minimized().unwrap_or(false) || window.is_fullscreen().unwrap_or(false) {
            return;
        }
        let maximized = window.is_maximized().unwrap_or(false);
        let mut noted = self.window.lock().expect("window geometry poisoned");
        match noted.as_mut() {
            // Keep the size it had before, so un-maximising next time returns to it.
            Some(geometry) if maximized => geometry.maximized = true,
            _ => {
                if let Some(geometry) = geometry_of(window, maximized) {
                    *noted = Some(geometry);
                }
            }
        }
    }

    /// Put the window back the way it was left. Called before the window is first shown,
    /// so it never appears at the default size and then jumps.
    pub fn restore_window(&self, window: &tauri::WebviewWindow) {
        let Some(saved) = self.layout.get().window else {
            return;
        };
        let _ = window.set_size(tauri::LogicalSize::new(saved.width, saved.height));
        let screens: Vec<Screen> = window
            .available_monitors()
            .unwrap_or_default()
            .iter()
            .map(|monitor| Screen {
                x: monitor.position().x,
                y: monitor.position().y,
                width: monitor.size().width,
                height: monitor.size().height,
            })
            .collect();
        if saved.reachable_on(&screens) {
            let _ = window.set_position(tauri::PhysicalPosition::new(saved.x, saved.y));
        } else {
            // Its display is gone. Centred on one that is, rather than somewhere unreachable.
            let _ = window.center();
        }
        if saved.maximized {
            let _ = window.maximize();
        }
    }
}

fn geometry_of(window: &tauri::Window, maximized: bool) -> Option<WindowGeometry> {
    let scale = window.scale_factor().ok()?;
    let size = window.inner_size().ok()?.to_logical::<f64>(scale);
    let at = window.outer_position().ok()?;
    Some(WindowGeometry {
        width: size.width,
        height: size.height,
        x: at.x,
        y: at.y,
        maximized,
    })
}

/// Where the server list, the trust store, the settings and the transfer queue live.
///
/// Falling back to the current directory is deliberate over refusing to start: a
/// missing app-data directory is an unusual environment, not a reason to make the app
/// unusable. Both stores tolerate a path they cannot write, and say so in the log.
fn paths(app: &tauri::AppHandle) -> EnginePaths {
    let dir = data_dir(app);
    EnginePaths {
        servers: dir.join("servers.json"),
        trust: dir.join("trust.json"),
        queue: dir.join("relay.sqlite"),
        settings: dir.join("settings.json"),
    }
}

fn data_dir(app: &tauri::AppHandle) -> std::path::PathBuf {
    app.path().app_data_dir().unwrap_or_else(|err| {
        tracing::warn!(%err, "no app data directory; falling back to the working directory");
        std::path::PathBuf::from(".")
    })
}
