# Phase 1 — SFTP Vertical Slice

Objective: a genuinely usable SFTP client: connect (all auth methods), trust host keys,
browse local+remote, transfer single files with live progress, manage files. Everything
runs through the real engine — no mocks left on the SFTP path.

Prerequisite: P0's SFTP/keychain gates and runtime/IPC contract are proven. This is
an internal usable slice; the public SFTP 1.0 release follows P2 and P5. Record a new
delivery estimate after this slice instead of assuming the previous week allocation.

## 1.1 Session actor (the concurrency backbone)

One tokio task per session owns the connection exclusively; nothing else touches it.
This mirrors FileZilla's one-engine-per-connection design and sidesteps `Send`/lifetime
pain with connection handles.

```rust
// crates/relay-core/src/session.rs
pub enum SessionCmd {
    ListDir { path: String, reply: oneshot::Sender<Result<Vec<RemoteEntry>>> },
    Stat    { path: String, reply: oneshot::Sender<Result<Option<RemoteEntry>>> },
    Mkdir/Rename/Delete { ..., reply: oneshot::Sender<Result<()>> },
    ReadFile { path: String, max: u64, reply: oneshot::Sender<Result<Vec<u8>>> },
    Transfer(TransferOrder),          // fire-and-forget; progress via EngineEvent
    Disconnect,
}

pub struct SessionActor {
    id: SessionId,
    backend: Box<dyn Protocol>,        // SftpBackend now, FtpBackend in P3
    cmd_rx: mpsc::Receiver<SessionCmd>,
    events: mpsc::Sender<EngineEvent>, // -> core state coordinator -> shell channel adapter
    interact: Arc<dyn Interact>,
}
```

Actor loop rules:
- `tokio::select!` over `cmd_rx`, a 30s keepalive interval (`noop()`, also measures
  latency → `Latency` event), and the connection's liveness.
- Browse operations (list/stat/mkdir/...) are serialized per session, with deadlines
  and cancellation; remote operations can stall. A stalled listing must not prevent
  disconnect or leave shutdown waiting indefinitely.
- **Transfers do NOT run in the browse actor** (they'd block browsing). See 1.4.
- On fatal error: emit `SessionState::Disconnected{reason}`, drop backend. (Reconnect
  logic arrives in P2; P1 shows the designed connection-lost pane + manual reconnect.)
- Spawn through the injected `tokio::runtime::Handle`; store cancellation tokens and
  join handles in `SessionHandle`. No Tauri imports in `relay-core`. `session_close`
  cancels the session and its children, denies outstanding prompts, and awaits cleanup
  with a grace deadline; abort is only a fallback. The shell initiates the same process
  on app exit. Plain Tokio tests exercise this lifecycle without a GUI.

## 1.2 SftpBackend (russh + russh-sftp)

Connection sequence (`connect()`):
1. TCP + SSH handshake via `russh::client::connect` with a `ClientHandler`.
2. **Host key verification** happens inside `ClientHandler::check_server_key`:
   compute SHA256 fingerprint → look up `TrustStore` → if unknown/changed, call
   `interact.ask(HostKey{..})` and await the UI's sheet reply. `Accept{remember:true}`
   → pin to store. Deny → abort with `EngineError::TrustRejected`.
   Implementation note: the handler needs the `Interact` handle + a preloaded trust
   entry; pass `Arc`s into the handler struct.
3. **Auth ladder** driven by `ServerConfig.auth`:
   - `Agent`: `russh` agent client — env `SSH_AUTH_SOCK` on macOS; on Windows try
     OpenSSH agent named pipe `\\.\pipe\openssh-ssh-agent`. (Pageant: out of scope,
     document it.) Iterate agent identities, try each.
   - `KeyFile`: `russh::keys::load_secret_key(path, passphrase)`; on
     passphrase-required error → `interact.ask(Password{hint:"key passphrase"})`;
     support OpenSSH + PEM formats, ed25519/ecdsa/rsa.
   - `Password`: from `SecretSource` (keychain) or `Ask` → prompt. On failure,
     try `keyboard-interactive` before giving up (some servers only offer k-i).
4. Open SFTP subsystem channel → `russh_sftp::client::SftpSession`.
5. `realpath(".")` for the landing directory (or `initial_remote_path`); emit
   `SessionState::Connected{ server_info }` (includes negotiated kex/cipher for the UI's
   encryption details).

Operation mapping:
- `list`: `read_dir(path)` → map attrs: size, mtime, `perms` rendered from mode bits
  (`rwxr-xr-x` renderer + unit tests), uid/gid→owner strings when `longname` provides
  them. Symlinks: `stat` the target for `EntryKind::Symlink{target_kind}`.
- `download`/`upload`: build on **`RawSftpSession::read/write(handle, offset, …)`**
  using the versions and handle ownership proven in P0. Offset addressing supplies
  the mechanics for P2 resume; it does not establish source or partial-file integrity.
  Loop: read/write chunk at `offset + transferred` → `progress(bytes_total)`;
  chunk 128 KiB (spike: measure 32/64/128/256 KiB against a real server; russh-sftp
  negotiates server limits). Check cancellation while awaiting I/O as well as between
  chunks; apply deadlines and close/discard a lane when cancellation leaves its
  protocol state uncertain. Use P0 latency measurements to choose bounded request
  pipelining if sequential chunks limit throughput.
- Download safety: exclusively create a job-specific sibling temporary file, e.g.
  `.{name}.{job_id}.relaypart`, flush/fsync, then finalize with the selected conflict
  policy. Verify replacement/rename behavior on macOS and Windows; keep-both must
  not overwrite a file created between the initial conflict check and completion.
  A suffix alone never establishes ownership or resume eligibility. P1 cancels and
  removes only its own partials; P2 adds durable ownership records and paused recovery.
- Uploads also use an owned job-specific remote temporary path where supported,
  then finalize after acknowledged writes and validation. Record rename/replace
  capabilities; if safe finalization is unavailable, fail with a clear error in the
  initial SFTP slice rather than claiming atomic replacement.
- `read_file`: for Quick Look/editor; enforce `max` (default 512 KiB) and return
  `EngineError::Protocol("too large")` beyond it.

## 1.3 Trust store & secrets

```
app_data_dir()/
├── servers.json        # Vec<ServerConfig> — never contains secrets
├── trust.json          # { "host:port": { algo, sha256, first_seen, last_seen } }
└── relay.sqlite        # queue persistence (P2)
```

- `TrustStore`: load-on-start, `Mutex<HashMap>`, atomic write-on-change (write temp +
  rename). `changed: true` prompt variant when a pinned key differs — red styling in
  the sheet per design.
- `SecretSource` (trait) → `KeyringSecrets`: service `"relay"`, account
  `"{server_uuid}:password"` / `":passphrase"`. Fallback `Ask` path when the entry is
  missing. Server delete ⇒ best-effort keyring cleanup.
- Run blocking OS keychain calls off async worker threads. The engine's prompt broker
  retains pending prompt state; the shell only transports requests and replies.

## 1.4 Transfers without a queue (P1 scope)

P2 builds the real scheduler; P1 needs single transfers that don't block browsing:
- `Transfer(TransferOrder)` in the actor **opens a second SFTP channel** on the same
  SSH connection (`sftp` subsystem channel #2) and spawns a child task for the
  transfer; the actor keeps browsing. Cap: 2 concurrent ad-hoc transfers in P1.
- Progress: child task calls `progress(bytes)`; a `ProgressThrottle` (100 ms min
  interval + always-emit-final) wraps the event sender → `JobUpdate` events feed both
  the flow-gutter pills and (P2) the drawer.
- Conflict handling in P1 = the design's sheet with Overwrite/Skip only (KeepBoth &
  Resume land in P2 with the queue); default from settings.

## 1.5 Local filesystem pane

Rust-side (consistency + hidden files + future watching):
- `local_list_dir(path) -> Vec<LocalEntry>` via `std::fs::read_dir` + `metadata()`;
  hidden = dotfiles (macOS) / `FILE_ATTRIBUTE_HIDDEN` (Windows, via
  `std::os::windows::fs::MetadataExt`). Sort dirs-first server-side… i.e. Rust-side.
- Run blocking filesystem enumeration on a bounded blocking worker; cancel or ignore
  stale requests when the user navigates away.
- `local_default_dir()` = home dir. Volume/drive enumeration for breadcrumb root menu:
  macOS `/Volumes/*`, Windows `GetLogicalDrives` (the `sysinfo` crate covers both).
- Errors map to the designed permission-denied / not-found pane states.

## 1.6 Frontend integration

- ConnectView: URL parser (`sftp://user@host:22/path`) as a pure TS function with unit
  tests — proto/user/host/port/path chips + "inferred from port" warning per design;
  Enter → `servers_save` (ephemeral recent) + `session_open`.
- SFTP is the only selectable protocol for 1.0. Reject FTP/FTPS URLs with a clear
  unavailable-protocol message rather than trying SFTP on their ports.
- Prompt plumbing: the P0 channel carries `PromptOpened`/`PromptClosed`; snapshots
  include unresolved prompts. `uiStore` renders by stable prompt id; answers use
  `resolve_prompt`. The core broker holds prompt facts and oneshot replies; it accepts
  a reply once, rejects stale/duplicate ids, and times out after 10 min (auto-Deny).
  A UI remount reconstructs the existing prompt rather than creating another one.
- Implement bounded per-session status/error logs and a basic copyable log view now,
  with secrets redacted. Rich activity/raw-protocol UI is P4. Logs use the P0 channel
  and a bounded Rust backlog fetched on demand.
- Remote pane: listing render, sort (name/size/modified; dirs first), filter, dimmed
  dotfiles, skeleton rows while `ListDir` in flight, breadcrumb navigation, double-click
  dir enter, F2 rename, Del delete (with confirm modal), new-folder button.
- Transfers: hover action pills (Upload→/↓Download) + drag between panes enqueue a
  `TransferOrder`; flow gutter animates from real `JobUpdate` events.
- Server editor sheet: full form incl. **Test connection** → `session_test(cfg)` command
  that runs `connect()` + `disconnect()` on a throwaway actor, streaming state text
  (testing/ok/fail + error message) — reuses the real prompt flow for host keys.

## 1.7 Tests & exit criteria

Tests:
- Unit: perms renderer, URL parser (TS), trust store round-trip incl. changed-key,
  chunked read/write against a mock `Protocol`.
- Integration (CI, Linux runner is fine for engine tests): dockerized
  `atmoz/sftp` (OpenSSH): connect w/ password + key, list, up/download 50 MB with
  progress monotonicity + cancellation mid-transfer, host-key reject path.
  Gate behind `--features integration` so plain `cargo test` stays offline.
- Manual matrix: macOS + Windows against a real VPS (openssh) — agent auth on both OSes
  reuses and expands the passing P0 Windows/macOS agent prototype.
- Lifecycle: stalled listing/transfer followed by close; no orphan tasks or unresolved
  prompts. Remount during transfer and trust prompt; snapshot restores current state.

Exit criteria:
1. Connect to a real server via password, key file (with passphrase), and agent.
2. Host-key sheet appears on first connect; pin persists; changed-key shows red variant.
3. Browse 5k-entry directory smoothly; rename/delete/mkdir reflected after refresh.
4. Upload + download with live gutter pills; bounded cancellation works; only owned
   temporary files are cleaned up, and successful finalization respects conflicts on
   both OSes. The destination is never replaced by an incomplete transfer.
5. Passwords live only in the OS keychain (verify `servers.json` by inspection).
