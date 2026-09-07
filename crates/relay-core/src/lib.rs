//! Relay's transfer engine.
//!
//! A library with a command-in / event-out surface and no GUI dependency: the Tauri
//! shell is an adapter over it, and `cargo test -p relay-core` runs the whole engine
//! headlessly. The design rule is carried over from FileZilla's engine, for the same
//! reason — the interface should be replaceable without touching transfer logic.
//!
//! The pieces:
//!
//! - [`model`] / [`job`] — the value types the shell and the generated TypeScript share.
//! - [`protocol`] — what a backend must implement, and the transfer lanes it hands out.
//! - [`queue`] — the job record and the single function allowed to change its state.
//! - [`store`] — the same records on disk, so a restart resumes rather than forgets.
//! - [`scheduler`] — the one task that decides which job runs, and when.
//! - [`interact`] — prompts, and the broker that owns every unanswered question.
//! - [`events`] / [`coordinator`] — the ordered update stream and the authoritative
//!   projection behind snapshot recovery.
//! - [`hub`] — the front door: takes a `tokio::runtime::Handle`, owns the pump.
//! - [`engine`] — the command surface the shell calls.
//! - [`session`] — one actor per connection; [`sftp`] is the backend behind it.
//! - [`walk`] — turning a folder into the files inside it.
//! - [`mock`] — an in-memory backend for tests and frontend development.

pub mod bindings;
pub mod coordinator;
pub mod engine;
pub mod error;
pub mod events;
pub mod hub;
pub mod interact;
pub mod job;
pub mod local;
pub mod mock;
pub mod model;
pub mod protocol;
pub mod queue;
pub mod scheduler;
pub mod secrets;
pub mod servers;
pub mod session;
pub mod settings;
pub mod sftp;
pub mod store;
pub mod trust;
pub mod walk;
pub mod wire;

pub use coordinator::{Coordinator, EngineEnvelope, EngineSnapshot, Subscription};
pub use engine::{Engine, EnginePaths};
pub use error::{EngineError, Result};
pub use events::EngineEvent;
pub use hub::EngineHub;
pub use interact::{Interact, Prompt, PromptBroker, PromptReply};
pub use model::{ServerConfig, SessionId};
pub use protocol::{Protocol, TransferLane, TransferReq};
pub use scheduler::{Dispatcher, RunRequest, Scheduler};
pub use session::SessionHandle;
pub use trust::{TrustDecision, TrustStore};
pub use wire::{Bytes, Order, Seq};
