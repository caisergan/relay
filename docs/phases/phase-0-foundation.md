# Phase 0 — Foundation & Skeleton (week 1)

Objective: a compiling, CI-green Tauri v2 workspace whose static UI already pixel-matches
the Relay design, with the full IPC contract defined as types before any protocol code exists.
Everything later plugs into interfaces created here — this phase is about getting the
boundaries right, not features.

## 0.1 Repository & workspace layout

```
relay/
├── Cargo.toml                 # [workspace] members = ["src-tauri", "crates/relay-core"]
├── package.json               # pnpm; scripts: dev, build, lint, typecheck, test
├── src/                       # frontend (React 18 + TS strict + Vite 6)
├── src-tauri/                 # tauri shell crate  (crate name: relay-app)
├── crates/relay-core/         # engine library     (crate name: relay-core)
├── docs/phases/               # these files
├── reference/filezilla/       # MOVE the GPL tree here; add README: "reference only, never copy"
└── .github/workflows/ci.yml
```

Tasks:
- [ ] `pnpm create tauri-app` (React-TS template), then convert to Cargo workspace; move
      GPL FileZilla source under `reference/` with a warning README.
- [ ] Rust toolchain pin (`rust-toolchain.toml`, stable), `rustfmt.toml`, `clippy` in CI
      (`-D warnings`), `.editorconfig`, ESLint + Prettier + `tsc --noEmit` in CI.
- [ ] CI matrix: `macos-14` (aarch64) and `windows-latest`; jobs: lint, `cargo test`,
      `pnpm test`, and a `tauri build --debug` smoke build on both OSes.
      Cache with `Swatinem/rust-cache` + pnpm store cache.
- [ ] Decide crate deps up front (add now, even if unused, so CI proves they build on
      both platforms): `tokio` (full), `serde`/`serde_json`, `thiserror`, `tracing` +
      `tracing-subscriber`, `uuid` (v4, serde), `rusqlite` (bundled), `keyring`,
      `notify`, `russh`, `russh-sftp`, `suppaftp` (async-rustls feature), `chrono`.

## 0.2 relay-core skeleton: the engine contract

No protocol code yet — only the types and traits every later phase implements against.
This is the FileZilla lesson applied: a narrow command-in/event-out API.

```rust
// crates/relay-core/src/model.rs
pub type SessionId = Uuid;
pub type JobId = Uuid;
pub type PromptId = Uuid;

#[derive(Clone, Serialize, Deserialize)]
pub enum Proto { Sftp, Ftps, Ftp }            // Ftps = explicit AUTH TLS; add Implicit later if demanded

#[derive(Clone, Serialize, Deserialize)]
pub enum AuthMethod { Password, KeyFile { path: PathBuf }, Agent, Ask }

#[derive(Clone, Serialize, Deserialize)]
pub struct ServerConfig {                      // persisted in servers.json (NO secrets)
    pub id: Uuid, pub name: String, pub host: String, pub port: u16,
    pub proto: Proto, pub username: String, pub auth: AuthMethod,
    pub color: Option<String>, pub group: Option<String>,
    pub bookmarks: Vec<Bookmark>,              // { label, remote_path }
    pub initial_remote_path: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RemoteEntry {
    pub name: String,
    pub kind: EntryKind,                       // File | Dir | Symlink { target_kind }
    pub size: u64,
    pub modified: Option<DateTime<Utc>>,
    pub perms: Option<String>,                 // "rwxr-xr-x" display string
    pub mode: Option<u32>,                     // numeric, for future chmod
    pub owner: Option<String>, pub group: Option<String>,
}
```

```rust
// crates/relay-core/src/protocol.rs — implemented by SftpBackend (P1) and FtpBackend (P3)
#[async_trait]
pub trait Protocol: Send {
    async fn connect(&mut self, cfg: &ServerConfig, secrets: &SecretSource,
                     interact: &dyn Interact) -> Result<ServerInfo, EngineError>;
    async fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>, EngineError>;
    async fn download(&mut self, req: TransferReq<'_>) -> Result<(), EngineError>;
    async fn upload(&mut self, req: TransferReq<'_>) -> Result<(), EngineError>;
    async fn stat(&mut self, path: &str) -> Result<Option<RemoteEntry>, EngineError>;
    async fn mkdir(&mut self, path: &str) -> Result<(), EngineError>;
    async fn rename(&mut self, from: &str, to: &str) -> Result<(), EngineError>;
    async fn remove_file(&mut self, path: &str) -> Result<(), EngineError>;
    async fn remove_dir(&mut self, path: &str) -> Result<(), EngineError>;
    async fn read_file(&mut self, path: &str, max: u64) -> Result<Vec<u8>, EngineError>; // quick look / editor
    async fn noop(&mut self) -> Result<(), EngineError>;                                  // keepalive + latency
    async fn disconnect(&mut self);
}

pub struct TransferReq<'a> {
    pub remote_path: String, pub local_path: PathBuf,
    pub offset: u64,                            // resume support from day one
    pub progress: &'a (dyn Fn(u64) + Send + Sync),  // bytes-so-far; caller throttles
    pub cancel: CancellationToken,
}
```

Prompts (`Interact`) follow FileZilla's async-request pattern, made async/await-native:

```rust
// crates/relay-core/src/interact.rs
pub enum Prompt {
    HostKey { host: String, algo: String, sha256: String, changed: bool },
    TlsCert { chain_pem: Vec<String>, sha256: String, host: String },
    Password { hint: String },
    Conflict { local: FileFacts, remote: FileFacts, direction: Direction, remaining: u32 },
}
pub enum PromptReply { Accept { remember: bool }, Deny,
                       Password(String),
                       Conflict { action: ConflictAction, apply_to_remaining: bool } }
pub enum ConflictAction { Overwrite, Skip, KeepBoth, Resume }

#[async_trait]
pub trait Interact: Send + Sync {              // impl in src-tauri: emits event, awaits oneshot
    async fn ask(&self, session: SessionId, prompt: Prompt) -> PromptReply;
}
```

Engine → app events (one enum; the Tauri layer serializes and `emit`s them):

```rust
// crates/relay-core/src/events.rs
pub enum EngineEvent {
    SessionState { id: SessionId, state: SessState },   // Connecting/Connected{info}/Reconnecting{attempt}/Disconnected{reason}
    Latency     { id: SessionId, ms: u32 },
    Listing     { id: SessionId, path: String, entries: Vec<RemoteEntry> },
    JobUpdate   { job: JobSnapshot },                    // covers progress/state changes
    QueueStats  { active: u32, queued: u32, failed: u32, speed_bps: u64 },
    Log         { id: SessionId, kind: LogKind, line: String }, // Status|Command|Response|Error (raw log)
    Activity    { id: SessionId, entry: ActivityEntry },        // human-readable feed
}
```

Tasks:
- [ ] Implement the modules above with `todo!()`-free compiling stubs (`MockBackend`
      implementing `Protocol` over an in-memory tree — reused by frontend dev & tests).
- [ ] `EngineError` taxonomy (`thiserror`): `Auth`, `Network`, `Timeout`, `NotFound`,
      `PermissionDenied`, `TrustRejected`, `Cancelled`, `Protocol(String)` — the UI maps
      these to designed states (permission-denied pane, connection-lost banner, etc.).

## 0.3 Tauri shell: typed IPC

- [ ] Adopt **tauri-specta** to generate TS types + typed `invoke`/event bindings from
      Rust. All payloads defined once in Rust; `pnpm gen:ipc` regenerates `src/ipc/gen.ts`.
- [ ] Commands (stubs returning mock data): `servers_list/save/delete`,
      `session_open(server_id) -> SessionId`, `session_close`, `session_list_dir`,
      `local_list_dir`, `queue_enqueue`, `queue_control(op)`, `resolve_prompt(prompt_id, reply)`,
      `settings_get/set`.
- [ ] Event channel names: `engine://event` (single multiplexed channel carrying
      `EngineEvent` + session id; simpler than per-session channels, and the frontend
      store fans out internally).
- [ ] `AppState`: `HashMap<SessionId, SessionHandle>` behind `tokio::sync::RwLock`;
      `SessionHandle = mpsc::Sender<SessionCmd>` (actor model — defined in P1).

## 0.4 Frontend: static shell, pixel-matched

- [ ] `styles/tokens.css`: transcribe the FULL variable set from Relay.dc.html — both
      themes (surface/panel/panel-2, ink triad, signal `#2456E6`, transit `#E8A03C`,
      ok/danger + -soft variants, line/line-2, row heights 40/32, elevation shadows),
      fonts (Space Grotesk / Inter 13px / JetBrains Mono — self-host via fontsource,
      no CDN), keyframes (skeleton, spin, pulse, trail, flow, sheet-in, fade).
      `prefers-reduced-motion` guards from the start.
- [ ] Component shells with mock data (no logic): TitleBar+SessionTabs, Sidebar
      (groups/avatars/bookmarks/new-server), SessionHeader (status dot states),
      PaneHeader (breadcrumbs/filter/view toggle), FileList (virtualized from day one —
      `@tanstack/react-virtual`), FlowGutter, QueueDrawer (3 tabs), ConnectView,
      Toasts, Modal/Sheet primitives.
- [ ] Zustand stores: `serversStore`, `sessionsStore` (tabs, active session, per-session
      pane state), `queueStore`, `uiStore` (theme, density, palette open, toasts).
      One `ipc/bridge.ts` subscribes to `engine://event` and dispatches into stores —
      components never call `listen()` directly.
- [ ] Custom title bar: `decorations: false`, macOS traffic-light inset handling, and
      Windows minimize/maximize/close buttons (platform-conditional component).

## 0.5 Definition of done (exit criteria)

1. `pnpm tauri dev` shows the full static shell (both themes, both densities) with mock
   data on macOS and Windows; CI green on both.
2. `cargo test -p relay-core` runs MockBackend round-trip tests (list/download/upload
   against the in-memory tree, incl. a cancellation test).
3. A demo "fake session" wired end-to-end through real IPC: clicking a mock server tab
   calls `session_open`, MockBackend listing renders through the real event path —
   proving the contract before any network code exists.
4. ADR docs committed: `docs/adr/001-no-cpp-engine.md`, `002-actor-per-session.md`,
   `003-typed-ipc-specta.md`.

Risks to burn down THIS week (small spikes, timeboxed ~half a day each):
- keyring crate on Windows Credential Manager (write/read/delete round-trip).
- tauri-specta compatibility with current Tauri 2.x minor.
- Virtualized list with 10k mock rows + column sort at 60fps in the webview.
