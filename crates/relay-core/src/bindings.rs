//! The single list of types that cross the IPC boundary.
//!
//! `cargo run -p relay-core --example gen-ipc` turns this into `src/ipc/gen.ts`, and
//! CI regenerates it to fail on stale output. Generation lives in the core rather than
//! the shell so it runs on every platform — including a Linux workstation that cannot
//! build the webview — and so the core stays free of Tauri (ADR 003).

use specta::Types;

pub fn type_collection() -> Types {
    Types::default()
        .register::<crate::coordinator::EngineEnvelope>()
        .register::<crate::coordinator::EngineSnapshot>()
        .register::<crate::coordinator::SnapshotError>()
        .register::<crate::error::EngineError>()
        .register::<crate::events::ActivityEntry>()
        .register::<crate::events::EngineEvent>()
        .register::<crate::events::ListingSnapshot>()
        .register::<crate::events::LogKind>()
        .register::<crate::events::LogLine>()
        .register::<crate::interact::ConflictAction>()
        .register::<crate::interact::Prompt>()
        .register::<crate::interact::PromptReply>()
        .register::<crate::interact::PromptRequest>()
        .register::<crate::interact::ResolveError>()
        .register::<crate::job::JobSnapshot>()
        .register::<crate::job::JobState>()
        .register::<crate::job::PauseReason>()
        .register::<crate::job::QueueStats>()
        .register::<crate::model::AuthMethod>()
        .register::<crate::model::Bookmark>()
        .register::<crate::model::Direction>()
        .register::<crate::model::FileFacts>()
        .register::<crate::model::FileKind>()
        .register::<crate::model::LocalEntry>()
        .register::<crate::model::Proto>()
        .register::<crate::model::RemoteEntry>()
        .register::<crate::model::ServerConfig>()
        .register::<crate::model::ServerInfo>()
        .register::<crate::model::Session>()
        .register::<crate::model::SessionState>()
        .register::<crate::wire::Bytes>()
        .register::<crate::wire::Order>()
        .register::<crate::wire::Seq>()
}
