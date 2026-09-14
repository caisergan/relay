//! Where the person left off: which servers were open, and which folder each pane was in.
//!
//! Read back only when the launch setting asks for it, but recorded either way, so that
//! turning the setting on restores the session that just ended rather than the next one.
//! How it is recorded, and why it is frozen at exit, is [`crate::recorded`].

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::model::ServerId;
use crate::recorded::Recorded;

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

/// `workspace.json`.
pub type WorkspaceStore = Recorded<Workspace>;

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
