# Phase 1 — SFTP Vertical Slice (weeks 2–4)

Objective: a genuinely usable SFTP client: connect (all auth methods), trust host keys,
browse local+remote, transfer single files with live progress, manage files. Everything
runs through the real engine — no mocks left on the SFTP path.

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
    events: mpsc::Sender<EngineEvent>, // -> Tauri emit loop
    interact: Arc<dyn Interact>,
}
```

Actor loop rules:
- `tokio::select!` over `cmd_rx`, a 30s keepalive interval (`noop()`, also measures
  latency → `Latency` event), and the connection's liveness.
- Browse operations (list/stat/mkdir/...) run inline in the actor — they're short and
  serialized per session, exactly like FileZilla's control connection.
- **Transfers do NOT run in the browse actor** (they'd block browsing). See 1.4.
- On fatal error: emit `SessionState::Disconnected{reason}`, drop backend. (Reconnect
  logic arrives in P2; P1 shows the designed connection-lost pane + manual reconnect.)
- Spawn with `tauri::async_runtime::spawn`; store `AbortHandle` in `SessionHandle`;
  `session_close` sends `Disconnect` then aborts after a grace timeout.

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
  (true pread/pwrite; verified in russh-sftp v2.3.x) rather than the higher-level
  `File` AsyncRead/Write wrapper — offset addressing gives us resume for free and
  leaves the door open to multi-chunk parallel single-file transfers later.
  Loop: read/write chunk at `offset + transferred` → `progress(bytes_total)`;
  chunk 128 KiB (spike: measure 32/64/128/256 KiB against a real server; russh-sftp
  honors `limits@openssh.com`). Check `cancel.is_cancelled()` each iteration.
- Download safety: write to `{name}.relaypart`, fsync, atomic rename on completion —
  this is also what makes resume detection trivial (a `.relaypart` file = partial).
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
- `local_default_dir()` = home dir. Volume/drive enumeration for breadcrumb root menu:
  macOS `/Volumes/*`, Windows `GetLogicalDrives` (the `sysinfo` crate covers both).
- Errors map to the designed permission-denied / not-found pane states.

## 1.6 Frontend integration

- ConnectView: URL parser (`sftp://user@host:22/path`) as a pure TS function with unit
  tests — proto/user/host/port/path chips + "inferred from port" warning per design;
  Enter → `servers_save` (ephemeral recent) + `session_open`.
- Prompt plumbing: `engine://event` carries `PromptRequest{prompt_id, session_id, kind, data}`;
  `uiStore` renders the matching sheet; answer → `resolve_prompt`. The Rust side holds
  `HashMap<PromptId, oneshot::Sender<PromptReply>>` — entries time out after 10 min
  (auto-Deny) so an ignored sheet can't leak a stuck operation.
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
  (**the Windows agent spike from P0 risk list happens here at the latest**).

Exit criteria:
1. Connect to a real server via password, key file (with passphrase), and agent.
2. Host-key sheet appears on first connect; pin persists; changed-key shows red variant.
3. Browse 5k-entry directory smoothly; rename/delete/mkdir reflected after refresh.
4. Upload + download with live gutter pills; cancel works; partial file cleaned up
   (`.relaypart` present during, atomic rename after).
5. Passwords live only in the OS keychain (verify `servers.json` by inspection).
