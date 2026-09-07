# Phase 0 — Foundation & Compatibility Gates

Objective: a compiling, CI-green Tauri v2 workspace, a representative design-based
shell, typed IPC with recovery, and executable evidence for the protocol choices.
Run the compatibility prototypes before investing in complete visual polish.
This starts the SFTP 1.0 path: **P0 → P1 → P2 → P5**; P3/P4 follow after release.
Timebox each prototype initially to 1–2 engineering days, then record results and
re-estimate unresolved work. A timebox ending is not a passed acceptance gate.

## 0.1 Repository & workspace layout

```
relay/
├── Cargo.toml                 # [workspace] members = ["src-tauri", "crates/relay-core"]
├── package.json               # pnpm; scripts: dev, build, lint, typecheck, test
├── src/                       # frontend (React 19 + TS strict + current stable Vite)
├── src-tauri/                 # tauri shell crate  (crate name: relay-app)
├── crates/relay-core/         # engine library     (crate name: relay-core)
├── docs/phases/               # these files
├── reference/filezilla/       # optional local reference, untracked and read-only
└── .github/workflows/ci.yml
```

Tasks:
- [ ] `pnpm create tauri-app` (React-TS template), then convert to Cargo workspace;
      preserve the existing read-only `reference/` arrangement.
- [ ] Resolve compatible stable React 19, Vite, TypeScript, Zustand, and Tauri
      versions. Starting review baseline: React 19.2 and Vite 8.2; verify current
      patches and plugin/Node requirements at scaffolding time. Pin Node/pnpm/Rust
      toolchains and commit `pnpm-lock.yaml` and `Cargo.lock`.
      [React versions](https://react.dev/versions), [Vite support](https://vite.dev/releases)
- [ ] Rust toolchain pin (`rust-toolchain.toml`, stable), `rustfmt.toml`, `clippy` in CI
      (`-D warnings`), `.editorconfig`, ESLint + Prettier + `tsc --noEmit` in CI.
- [ ] CI matrix: `macos-14` (aarch64) and `windows-latest`; jobs: lint, `cargo test`,
      `pnpm test`, and a `tauri build --debug` smoke build on both OSes.
      Cache with `Swatinem/rust-cache` + pnpm store cache.
- [ ] Add foundational deps with needed features: `tokio`, `tokio-util` (cancellation),
      `async-trait`, `serde`/`serde_json`, `thiserror`, `tracing` + `tracing-subscriber`,
      `uuid`, `rusqlite` (bundled), `keyring`, `russh`, `russh-sftp`, `chrono`.
      Keep `suppaftp` in the compatibility harness/optional FTP feature until P3;
      defer `notify` and editor dependencies until P4. Test actual manifests on both OSes.
- [ ] Choose suppaftp's Tokio + rustls feature for the selected crypto provider
      (`tokio-rustls-ring` or `tokio-rustls-aws-lc-rs` in the reviewed API), verify
      transitive native build requirements, and pin the tested combination.
      [SuppaFTP feature documentation](https://github.com/veeso/suppaftp#cargo-features)

## 0.2 relay-core skeleton: the engine contract

Define the minimum command-in/event-out contract and refine it against the prototypes.
The examples below are interface sketches, not a frozen implementation. In particular,
prove how the browse backend creates separately owned transfer handles; do not force
concurrent transfers through one `&mut Protocol` borrow.

`relay-core` takes a `tokio::runtime::Handle` and uses `Handle::spawn`; the shell
supplies it and standalone tests create a Tokio runtime. No core module imports
Tauri, including task spawning and prompts. Keep blocking SQLite/keychain/filesystem
work on a dedicated worker or `spawn_blocking`, outside actor/scheduler hot loops.

```rust
// crates/relay-core/src/model.rs
pub type SessionId = Uuid;
pub type JobId = Uuid;
pub type PromptId = Uuid;

#[derive(Clone, Serialize, Deserialize)]
pub enum Proto { Sftp, Ftps, Ftp }            // only Sftp enabled in 1.0; reject unavailable protocols

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
    pub offset: u64,                            // nonzero only after P2 resume verification
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
pub trait Interact: Send + Sync {              // core broker records pending prompt, awaits oneshot
    async fn ask(&self, session: SessionId, prompt: Prompt) -> PromptReply;
}
```

Engine → app updates (one enum; the shell sends them over a typed Tauri channel):

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
    PromptOpened { prompt: PromptRequest },                   // stable prompt id + session id
    PromptClosed { id: PromptId },
}
```

Tasks:
- [ ] Implement the modules above with `todo!()`-free compiling stubs (`MockBackend`
      implementing `Protocol` over an in-memory tree — reused by frontend dev & tests).
- [ ] `EngineError` taxonomy (`thiserror`): `Auth`, `Network`, `Timeout`, `NotFound`,
      `PermissionDenied`, `TrustRejected`, `Cancelled`, `SourceChanged`,
      `ResumeUnverifiable`, `IntegrityMismatch`, `Protocol(String)` — the UI maps
      these to designed states (permission-denied pane, connection-lost banner, etc.).

## 0.3 Tauri shell: typed IPC

- [ ] Validate **tauri-specta** compatibility and generate TS types + typed command/
      channel bindings from Rust. `pnpm gen:ipc` regenerates `src/ipc/gen.ts`; CI
      rejects stale generated output. If integration fails, record an alternative
      generator decision without adding a Tauri dependency to the core.
- [ ] Commands (stubs returning mock data): `servers_list/save/delete`,
      `session_open(server_id) -> SessionId`, `session_close`, `session_list_dir`,
      `local_list_dir`, `queue_enqueue`, `queue_control(op)`, `resolve_prompt(prompt_id, reply)`,
      `settings_get/set`, `engine_subscribe(channel)`, `engine_unsubscribe(id)`,
      `engine_snapshot(subscription_id)`.
- [ ] Use a single frontend bridge and an ordered Tauri `Channel<EngineEnvelope>`
      carrying `{ epoch, seq, update: EngineEvent }`. Transfer bytes stay in Rust.
      Tauri channels are designed for ordered streaming; reserve global events for
      optional small notifications. [Tauri channels](https://v2.tauri.app/develop/calling-frontend/#channels)
- [ ] Core coordinator maintains the authoritative UI projection. Subscribe first,
      buffer updates, then request a consistent snapshot of sessions, jobs, pending
      prompts, and current listing state with an epoch/sequence watermark. Apply the
      snapshot and only buffered updates above its watermark. On gap, overflow,
      remount, or epoch change, resubscribe/resnapshot; discard the old subscription.
      Tag listing requests so stale responses cannot replace a newly navigated pane.
- [ ] Bound subscriber buffers, coalesce progress before assigning sequence numbers,
      and bound log history separately. Overflow closes/invalidates the subscription;
      the bridge detects closure or a failed liveness check and resnapshots. Snapshot
      state preserves job outcomes and unresolved prompts even if delivery was missed.
      Exclude passwords/passphrases from snapshots and logs.
- [ ] `AppState`: `HashMap<SessionId, SessionHandle>` behind `tokio::sync::RwLock`;
      `SessionHandle` contains the command sender, cancellation token, and tracked
      task handles (actor model — defined in P1).

## 0.4 Frontend: representative shell and platform checks

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
      Groups/bookmarks, grid view, and palette placeholders may be design references;
      hide unfinished controls in the SFTP release build.
- [ ] Zustand stores: `serversStore`, `sessionsStore` (tabs, active session, per-session
      pane state), `queueStore`, `uiStore` (theme, density, palette open, toasts).
      One `ipc/bridge.ts` owns channel lifecycle, snapshots, and dispatch into stores.
      Rust owns job/session transitions; Zustand holds projections and UI preferences.
      Use narrow selectors and batch progress updates; components never subscribe
      directly to IPC or persist an independent transfer state machine.
- [ ] Custom title bar: `decorations: false`, macOS traffic-light inset handling, and
      Windows minimize/maximize/close buttons (platform-conditional component).
- [ ] Exercise title bar, shortcuts, focus/keyboard navigation, and OS drag-in on
      macOS and Windows webviews. Render 10k mock entries with virtualization and
      concurrent progress updates. Full editor/preview/palette polish waits for P4.

## 0.5 Definition of done (exit criteria)

1. `pnpm tauri dev` shows the representative shell (both themes, both densities) with mock
   data on macOS and Windows; CI green on both.
2. `cargo test -p relay-core` runs MockBackend round-trip tests (list/download/upload
   against the in-memory tree, incl. a cancellation test).
3. A demo "fake session" wired end-to-end through real IPC: clicking a mock server tab
   calls `session_open`, MockBackend listing renders through the real event path —
   proving the contract independently of the network prototypes. Remount the UI
   mid-job and while a prompt is open; force a gap/overflow and recover without stale
   state, duplicated prompts, or duplicate command execution.
4. ADR docs committed: `docs/adr/001-no-cpp-engine.md`, `002-actor-per-session.md`,
   `003-typed-ipc-specta.md`, `004-protocol-compatibility.md`, and
   `005-resume-integrity.md`, with measured evidence and unresolved decisions.
5. `cargo tree -p relay-core` has no Tauri dependency; standalone engine tests pass.
6. SFTP/keychain prototype gates below pass on both supported OSes. FTPS has a
   reproducible pass or an explicit deferred backend decision; it cannot silently be
   marked validated. Update estimates from these results before starting P1.

## 0.6 Compatibility prototypes (before full product implementation)

Each prototype delivers a small reproducible harness/fixture and records exact
crate versions/features, OS/server versions, commands, results, and open failures
in the compatibility ADR. All gates below are **pending**, not completed work.

| Prototype | Required evidence | Failure disposition |
|---|---|---|
| SFTP authentication | Password, encrypted key, keyboard-interactive, macOS agent, and Windows OpenSSH named-pipe agent authenticate; rejected/changed host keys fail correctly | Resolve backend/adapter gaps before P1; Pageant remains out of scope |
| SFTP concurrency and cancellation | Browse while two independent channels transfer; test stalled sockets, bounded cancellation and cleanup; measure throughput with realistic latency | Refine handle ownership, deadlines, and request pipelining before freezing `Protocol` |
| Interrupted transfer | Both directions: disconnect and terminate harness, recover a recorded partial, compare final hashes; same-size source mutation and corrupted partial refuse unsafe resume | Establish the P2 verification/checkpoint contract before advertising resume |
| FTPS session reuse | Real control + protected data connections against vsftpd and ProFTPD with reuse required; upload/download succeeds with certificate validation intact | Investigate selected suppaftp/rustls integration; record fallback or defer backend selection, without blocking SFTP 1.0 |
| FTPS certificate interaction | Trusted chain succeeds, rejected certificate fails, user exception binds to the exact host/port/certificate and retry re-verifies it | Prove verifier/prompt plumbing in harness before P3; never block an async runtime on a UI prompt |
| OS secrets | Keychain/Credential Manager write/read/delete and missing/denied-store behavior on both OSes | Resolve or document an explicit ask-only mode before P1 |

The early FTPS harness is not production FTP support. Keep it isolated and carry its
fixtures into P3. A proof of basic offset I/O is not proof of safe resume; the source
and prefix verification requirements in P2 govern the interrupted-transfer prototype.
