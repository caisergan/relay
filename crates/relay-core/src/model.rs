//! Value types shared by the engine, the shell, and the generated TypeScript.
//!
//! Every type here is part of the IPC surface: it derives `specta::Type` so
//! `cargo run -p relay-core --example gen-ipc` can emit `src/ipc/gen.ts`.
//! Enums are internally tagged with `kind` so TypeScript sees discriminated unions.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use specta::Type;
use uuid::Uuid;

use crate::wire::Bytes;

pub type SessionId = Uuid;
pub type JobId = Uuid;
pub type PromptId = Uuid;
pub type ServerId = Uuid;

/// Transfer protocol. Only [`Proto::Sftp`] is selectable in 1.0; the others exist so
/// persisted configs and the FTP prototype keep their shape (ROADMAP §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum Proto {
    Sftp,
    Ftps,
    Ftp,
}

impl Proto {
    /// Protocols the current build will actually open a session for.
    pub const fn is_available(self) -> bool {
        matches!(self, Proto::Sftp)
    }

    pub const fn default_port(self) -> u16 {
        match self {
            Proto::Sftp => 22,
            Proto::Ftps | Proto::Ftp => 21,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Proto::Sftp => "SFTP",
            Proto::Ftps => "FTPS",
            Proto::Ftp => "FTP",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AuthMethod {
    Password,
    #[serde(rename_all = "camelCase")]
    KeyFile {
        path: PathBuf,
    },
    Agent,
    /// Never stored; the user is prompted on every connect.
    Ask,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Bookmark {
    pub label: String,
    pub remote_path: String,
}

/// Persisted in `servers.json`. Secrets live in the OS keychain, never here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub id: ServerId,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub proto: Proto,
    pub username: String,
    pub auth: AuthMethod,
    /// Avatar tint from the design's palette, e.g. `"#2456E6"`.
    pub color: Option<String>,
    pub group: Option<String>,
    #[serde(default)]
    pub bookmarks: Vec<Bookmark>,
    pub initial_remote_path: Option<String>,
}

impl ServerConfig {
    /// `host:port`, the key used by the trust store.
    pub fn endpoint(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum FileKind {
    File,
    Dir,
    /// Symlinks carry the kind of their target so the UI can decide navigability;
    /// `None` means the target could not be stat'd (broken or permission-denied).
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RemoteEntry {
    pub name: String,
    pub kind: FileKind,
    /// Resolved kind of a symlink target; `None` for non-symlinks and broken links.
    pub target_kind: Option<FileKind>,
    pub size: Bytes,
    pub modified: Option<DateTime<Utc>>,
    /// Display string, e.g. `"rwxr-xr-x"`.
    pub perms: Option<String>,
    /// Numeric mode, kept for the post-1.0 chmod dialog.
    pub mode: Option<u32>,
    pub owner: Option<String>,
    pub group: Option<String>,
}

impl RemoteEntry {
    /// Directory-ness for sorting and navigation, following symlink targets.
    pub fn is_dir(&self) -> bool {
        self.kind == FileKind::Dir || self.target_kind == Some(FileKind::Dir)
    }

    pub fn is_hidden(&self) -> bool {
        self.name.starts_with('.')
    }
}

/// Local-pane entry. Enumerated in Rust so hidden-file rules and metadata match
/// the remote pane on both platforms (phase 1 §1.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LocalEntry {
    pub name: String,
    pub path: PathBuf,
    pub kind: FileKind,
    pub target_kind: Option<FileKind>,
    pub size: Bytes,
    pub modified: Option<DateTime<Utc>>,
    pub hidden: bool,
    pub readonly: bool,
}

/// What a successful connect learned about the peer. Surfaced in the session header's
/// encryption details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub banner: Option<String>,
    /// e.g. `"SSH-2.0-OpenSSH_9.6"`.
    pub software: Option<String>,
    pub kex: Option<String>,
    pub cipher: Option<String>,
    pub mac: Option<String>,
    pub host_key_algo: Option<String>,
    pub host_key_sha256: Option<String>,
    /// Directory the session landed in.
    pub home_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    Up,
    Down,
}

/// The facts a conflict or resume decision is allowed to depend on.
///
/// Phase 2 requires more than a size match before resuming: `size` alone never
/// authorises resume, which is why mtime and an optional content digest travel with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FileFacts {
    pub path: String,
    pub size: Bytes,
    pub modified: Option<DateTime<Utc>>,
    /// SHA-256 of the verified prefix, when one has been computed.
    pub digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SessionState {
    Connecting,
    #[serde(rename_all = "camelCase")]
    Connected {
        info: Box<ServerInfo>,
    },
    #[serde(rename_all = "camelCase")]
    Reconnecting {
        attempt: u32,
        /// Seconds until the next attempt, for the designed countdown.
        retry_in_secs: u32,
    },
    #[serde(rename_all = "camelCase")]
    Disconnected {
        reason: String,
        /// False after a deliberate close, which suppresses the connection-lost pane.
        unexpected: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: SessionId,
    pub server_id: ServerId,
    pub name: String,
    pub proto: Proto,
    pub state: SessionState,
    pub latency_ms: Option<u32>,
    pub remote_path: Option<String>,
}
