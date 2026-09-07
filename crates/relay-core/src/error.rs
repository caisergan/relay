//! The error taxonomy the UI maps onto designed states.
//!
//! Variants exist because a *screen* distinguishes them: `PermissionDenied` draws the
//! permission-denied pane, `TrustRejected` returns to connect, `SourceChanged` and
//! `ResumeUnverifiable` produce the "restart or skip" queue prompt rather than a
//! generic failure. Do not add a variant the interface cannot act on differently.

use std::path::Path;

use serde::{Deserialize, Serialize};
use specta::Type;
use thiserror::Error;

pub type Result<T, E = EngineError> = std::result::Result<T, E>;

#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EngineError {
    /// Credentials were rejected. Never carries the credential itself.
    #[error("authentication failed: {message}")]
    #[serde(rename_all = "camelCase")]
    Auth { message: String },

    #[error("network error: {message}")]
    #[serde(rename_all = "camelCase")]
    Network { message: String },

    #[error("{operation} timed out after {after_secs}s")]
    #[serde(rename_all = "camelCase")]
    Timeout { operation: String, after_secs: u32 },

    #[error("not found: {path}")]
    #[serde(rename_all = "camelCase")]
    NotFound { path: String },

    #[error("permission denied: {path}")]
    #[serde(rename_all = "camelCase")]
    PermissionDenied { path: String },

    /// The user declined a host key or certificate, or a pinned key changed and the
    /// change was not accepted.
    #[error("trust rejected for {endpoint}")]
    #[serde(rename_all = "camelCase")]
    TrustRejected { endpoint: String },

    #[error("cancelled")]
    Cancelled,

    /// The source differs from the facts recorded when the partial transfer began.
    /// Resuming would splice two different files together.
    #[error("source changed since the transfer started: {path}")]
    #[serde(rename_all = "camelCase")]
    SourceChanged { path: String },

    /// Resume could not be *proven* safe. Distinct from `SourceChanged`: there we know
    /// the source moved, here we cannot establish that it did not.
    #[error("resume could not be verified: {reason}")]
    #[serde(rename_all = "camelCase")]
    ResumeUnverifiable { reason: String },

    #[error("integrity check failed: expected {expected}, got {actual}")]
    #[serde(rename_all = "camelCase")]
    IntegrityMismatch { expected: String, actual: String },

    /// Local filesystem failure — a full disk, a vanished download directory, a
    /// read-only volume. Extends the taxonomy in phase 0 §0.2 because the local side
    /// fails independently of the connection and gets its own designed message.
    #[error("local file error at {path}: {message}")]
    #[serde(rename_all = "camelCase")]
    LocalIo { path: String, message: String },

    /// The server or backend cannot do something Relay requires for safety — notably
    /// atomic finalisation of an upload. Phase 1 fails loudly here rather than
    /// claiming a guarantee it cannot keep.
    #[error("unsupported by this server: {operation}")]
    #[serde(rename_all = "camelCase")]
    Unsupported { operation: String },

    #[error("protocol error: {message}")]
    #[serde(rename_all = "camelCase")]
    Protocol { message: String },
}

impl EngineError {
    /// Map a local IO failure onto the taxonomy, keeping the path for the UI.
    pub fn from_io(path: impl AsRef<Path>, err: &std::io::Error) -> Self {
        use std::io::ErrorKind;
        let path = path.as_ref().display().to_string();
        match err.kind() {
            ErrorKind::NotFound => EngineError::NotFound { path },
            ErrorKind::PermissionDenied => EngineError::PermissionDenied { path },
            ErrorKind::TimedOut => EngineError::Timeout {
                operation: format!("io on {path}"),
                after_secs: 0,
            },
            _ => EngineError::LocalIo {
                path,
                message: err.to_string(),
            },
        }
    }

    pub fn network(message: impl Into<String>) -> Self {
        EngineError::Network {
            message: message.into(),
        }
    }

    pub fn protocol(message: impl Into<String>) -> Self {
        EngineError::Protocol {
            message: message.into(),
        }
    }

    /// Whether the scheduler may retry this on its own (phase 2 backoff).
    ///
    /// Anything touching identity, trust, or integrity is deliberately excluded:
    /// retrying those either cannot succeed or would paper over a real mismatch.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            EngineError::Network { .. } | EngineError::Timeout { .. }
        )
    }

    /// Whether the failure needs a person to choose what happens next.
    pub fn needs_user_decision(&self) -> bool {
        matches!(
            self,
            EngineError::Auth { .. }
                | EngineError::TrustRejected { .. }
                | EngineError::SourceChanged { .. }
                | EngineError::ResumeUnverifiable { .. }
                | EngineError::IntegrityMismatch { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_errors_map_onto_designed_states() {
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(matches!(
            EngineError::from_io("/tmp/x", &denied),
            EngineError::PermissionDenied { .. }
        ));

        let full = std::io::Error::other("No space left on device");
        let mapped = EngineError::from_io("/tmp/x", &full);
        assert!(matches!(mapped, EngineError::LocalIo { .. }));
        assert!(
            !mapped.is_retryable(),
            "a full disk must not silently retry"
        );
    }

    #[test]
    fn integrity_failures_are_never_retried_automatically() {
        let err = EngineError::IntegrityMismatch {
            expected: "abc".into(),
            actual: "def".into(),
        };
        assert!(!err.is_retryable());
        assert!(err.needs_user_decision());
    }
}
