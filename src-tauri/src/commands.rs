//! The IPC surface: parse, delegate to `relay-core`, return. No engine decisions here.
//!
//! Every command is thin on purpose. If a command starts making a choice about
//! transfer state, that choice belongs in the engine where it can be tested without
//! a webview.

use std::path::PathBuf;
use std::sync::Arc;

use relay_core::coordinator::{EngineEnvelope, EngineSnapshot, SnapshotError, SubscriptionId};
use relay_core::error::EngineError;
use relay_core::events::LogLine;
use relay_core::interact::{PromptReply, ResolveError};
use relay_core::job::QueueOp;
use relay_core::model::{
    Direction, JobId, LocalEntry, RemoteEntry, ServerConfig, ServerId, SessionId,
};
use relay_core::settings::Settings;
use tauri::State;
use tauri::ipc::Channel;

use crate::state::AppState;

type Result<T> = std::result::Result<T, EngineError>;

// ---------------------------------------------------------------- servers

#[tauri::command]
pub async fn servers_list(state: State<'_, AppState>) -> Result<Vec<ServerConfig>> {
    Ok(state.servers.read().await.clone())
}

#[tauri::command]
pub async fn servers_save(
    state: State<'_, AppState>,
    config: ServerConfig,
) -> Result<ServerConfig> {
    if !config.proto.is_available() {
        return Err(EngineError::Unsupported {
            operation: format!("{} is not available in this release", config.proto.label()),
        });
    }
    let mut servers = state.servers.write().await;
    match servers.iter_mut().find(|s| s.id == config.id) {
        Some(existing) => *existing = config.clone(),
        None => servers.push(config.clone()),
    }
    Ok(config)
}

#[tauri::command]
pub async fn servers_delete(state: State<'_, AppState>, id: ServerId) -> Result<()> {
    state.servers.write().await.retain(|s| s.id != id);
    Ok(())
}

// ---------------------------------------------------------------- sessions

#[tauri::command]
pub async fn session_open(state: State<'_, AppState>, server_id: ServerId) -> Result<SessionId> {
    let config = state
        .server(server_id)
        .await
        .ok_or_else(|| EngineError::NotFound {
            path: server_id.to_string(),
        })?;
    Ok(state.engine.open_session(config))
}

#[tauri::command]
pub async fn session_close(state: State<'_, AppState>, id: SessionId) -> Result<()> {
    state.engine.close_session(id).await;
    Ok(())
}

#[tauri::command]
pub async fn session_list_dir(
    state: State<'_, AppState>,
    id: SessionId,
    path: String,
) -> Result<Vec<RemoteEntry>> {
    state.engine.list_dir(id, &path).await
}

#[tauri::command]
pub async fn session_logs(
    state: State<'_, AppState>,
    id: SessionId,
    limit: usize,
) -> Result<Vec<LogLine>> {
    Ok(state.hub.coordinator().logs(id, limit.min(1000)))
}

// ---------------------------------------------------------------- local pane

#[tauri::command]
pub async fn local_list_dir(path: PathBuf) -> Result<Vec<LocalEntry>> {
    // Enumeration blocks; keep it off the async runtime's worker threads.
    tauri::async_runtime::spawn_blocking(move || relay_core::local::list_dir(&path))
        .await
        .map_err(|e| EngineError::protocol(format!("local listing task failed: {e}")))?
}

#[tauri::command]
pub fn local_default_dir() -> PathBuf {
    relay_core::local::default_dir()
}

#[tauri::command]
pub fn local_roots() -> Vec<PathBuf> {
    relay_core::local::roots()
}

// ---------------------------------------------------------------- queue

#[tauri::command]
pub async fn queue_enqueue(
    state: State<'_, AppState>,
    session: SessionId,
    direction: Direction,
    remote_path: String,
    local_path: PathBuf,
) -> Result<JobId> {
    state
        .engine
        .enqueue(session, direction, remote_path, local_path)
        .await
}

#[tauri::command]
pub async fn queue_control(state: State<'_, AppState>, op: QueueOp) -> Result<()> {
    state.engine.queue_control(op)
}

// ---------------------------------------------------------------- prompts

#[tauri::command]
pub async fn resolve_prompt(
    state: State<'_, AppState>,
    prompt_id: relay_core::model::PromptId,
    reply: PromptReply,
) -> std::result::Result<(), ResolveError> {
    state.hub.prompts().resolve(prompt_id, reply)
}

// ---------------------------------------------------------------- settings

#[tauri::command]
pub async fn settings_get(state: State<'_, AppState>) -> Result<Settings> {
    Ok(state.settings.read().await.clone())
}

#[tauri::command]
pub async fn settings_set(state: State<'_, AppState>, settings: Settings) -> Result<Settings> {
    let settings = settings.normalised();
    *state.settings.write().await = settings.clone();
    Ok(settings)
}

// ---------------------------------------------------------------- engine stream

/// Subscribe to the ordered update stream.
///
/// The channel is Tauri's ordered streaming primitive; global events are reserved for
/// small optional notifications. The forwarder ends itself when the webview drops the
/// channel, so a reloaded window does not leave a task behind.
#[tauri::command]
pub async fn engine_subscribe(
    state: State<'_, AppState>,
    channel: Channel<EngineEnvelope>,
) -> Result<SubscriptionId> {
    let mut subscription = state.hub.subscribe();
    let id = subscription.id;

    let coordinator = Arc::clone(state.hub.coordinator());
    let handle = tauri::async_runtime::spawn(async move {
        while let Some(envelope) = subscription.rx.recv().await {
            if channel.send(envelope).is_err() {
                // The webview is gone (reload, close). Drop the subscription so the
                // coordinator stops fanning out to nobody.
                break;
            }
        }
        coordinator.unsubscribe(id);
    });

    state.forwarders.write().await.insert(id, handle);
    Ok(id)
}

#[tauri::command]
pub async fn engine_unsubscribe(state: State<'_, AppState>, id: SubscriptionId) -> Result<()> {
    state.hub.coordinator().unsubscribe(id);
    if let Some(handle) = state.forwarders.write().await.remove(&id) {
        handle.abort();
    }
    Ok(())
}

/// The consistent view to apply before replaying buffered envelopes. Fails with
/// `UnknownSubscription` when the subscription has been dropped — subscribe again
/// rather than reusing the old watermark.
#[tauri::command]
pub async fn engine_snapshot(
    state: State<'_, AppState>,
    subscription_id: SubscriptionId,
) -> std::result::Result<EngineSnapshot, SnapshotError> {
    state.hub.snapshot(subscription_id)
}
