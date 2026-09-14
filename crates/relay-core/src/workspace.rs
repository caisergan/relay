//! Where the person left off: which servers were open, and which folder each pane was in.
//!
//! Recorded as it changes rather than at exit, because an exit is not something to rely
//! on reaching — a force quit, a crash or a power cut runs no shutdown hook. And frozen
//! at the start of a clean exit, because the exit itself closes every session: left
//! open, the store would faithfully record a workspace with nothing in it, and that
//! would be the last thing written.
//!
//! Read back only when the launch setting asks for it, but recorded either way, so that
//! turning the setting on restores the session that just ended rather than the next one.
//! Like the settings, a missing or unreadable file is an empty workspace and never a
//! reason not to start.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::{EngineError, Result};
use crate::model::ServerId;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    /// In tab order.
    pub tabs: Vec<WorkspaceTab>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTab {
    pub server_id: ServerId,
    /// The local pane's folder; `None` if it had not listed one yet.
    pub local_path: Option<PathBuf>,
    /// The remote pane's folder; `None` if the session had not landed yet.
    pub remote_path: Option<String>,
    /// The tab that was showing.
    pub active: bool,
}

/// `workspace.json`, with the last recorded workspace in front of it.
pub struct WorkspaceStore {
    path: PathBuf,
    current: Mutex<Workspace>,
    frozen: AtomicBool,
}

impl WorkspaceStore {
    pub fn load(path: PathBuf) -> Self {
        let workspace = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Workspace>(&bytes).unwrap_or_else(|err| {
                tracing::warn!(?path, %err, "the workspace is unreadable; starting empty");
                Workspace::default()
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Workspace::default(),
            Err(err) => {
                tracing::warn!(?path, %err, "the workspace could not be read; starting empty");
                Workspace::default()
            }
        };
        Self {
            path,
            current: Mutex::new(workspace),
            frozen: AtomicBool::new(false),
        }
    }

    /// In-memory only, for tests.
    pub fn ephemeral() -> Self {
        Self::load(PathBuf::new())
    }

    pub fn get(&self) -> Workspace {
        self.current.lock().expect("workspace poisoned").clone()
    }

    /// Record the workspace, unless the app is on its way out.
    ///
    /// An unchanged workspace is not rewritten: this is called on every navigation, and
    /// most of those change something other than where the panes are. The lock is held
    /// across the write so two calls cannot interleave on the temporary file.
    pub fn set(&self, workspace: Workspace) -> Result<()> {
        if self.frozen.load(Ordering::SeqCst) {
            return Ok(());
        }
        let mut current = self.current.lock().expect("workspace poisoned");
        if *current == workspace {
            return Ok(());
        }
        self.persist(&workspace)?;
        *current = workspace;
        Ok(())
    }

    /// Stop recording. Called as a clean exit begins; see the module docs.
    pub fn freeze(&self) {
        self.frozen.store(true, Ordering::SeqCst);
    }

    fn persist(&self, workspace: &Workspace) -> Result<()> {
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        let json = serde_json::to_vec_pretty(workspace)
            .map_err(|e| EngineError::protocol(format!("serialising the workspace: {e}")))?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| EngineError::from_io(parent, &e))?;
        }
        let temp = self.path.with_extension("json.tmp");
        std::fs::write(&temp, &json).map_err(|e| EngineError::from_io(&temp, &e))?;
        std::fs::rename(&temp, &self.path).map_err(|e| EngineError::from_io(&self.path, &e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_tab(remote: &str) -> Workspace {
        Workspace {
            tabs: vec![WorkspaceTab {
                server_id: uuid::Uuid::new_v4(),
                local_path: Some(PathBuf::from("/Users/ada/Downloads")),
                remote_path: Some(remote.into()),
                active: true,
            }],
        }
    }

    #[test]
    fn a_workspace_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        let saved = one_tab("/var/www");

        WorkspaceStore::load(path.clone())
            .set(saved.clone())
            .unwrap();

        assert_eq!(WorkspaceStore::load(path).get(), saved);
    }

    /// The exit closes every session, and the interface reports each closing tab. None of
    /// that may reach the file, or every restore would open to nothing.
    #[test]
    fn nothing_is_recorded_once_frozen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        let store = WorkspaceStore::load(path.clone());
        let open = one_tab("/var/www");
        store.set(open.clone()).unwrap();

        store.freeze();
        store.set(Workspace::default()).unwrap();

        assert_eq!(store.get(), open);
        assert_eq!(WorkspaceStore::load(path).get(), open);
    }

    #[test]
    fn a_missing_or_unreadable_file_is_an_empty_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("workspace.json");
        assert_eq!(WorkspaceStore::load(missing).get(), Workspace::default());

        let broken = dir.path().join("broken.json");
        std::fs::write(&broken, b"{ not json").unwrap();
        assert_eq!(WorkspaceStore::load(broken).get(), Workspace::default());
    }
}
