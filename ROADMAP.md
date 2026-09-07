# Relay — Structure Plan & Roadmap

A lightweight, cross-platform (macOS + Windows) FTP/FTPS/SFTP client built with Tauri v2,
re-imagining FileZilla with the Relay design (Claude Design project `Relay.dc.html`).

---

## 1. The core decision: do NOT reuse the FileZilla C++ engine

The vendored `filezilla/` tree is **FileZilla 3.5.3 (January 2012)**. A deep review of
`src/engine` concluded that reusing it is impractical and legally problematic:

| Obstacle | Detail |
|---|---|
| wxWidgets is load-bearing | `wxString`, `wxDateTime`, `wxEvtHandler`, `wxThreadEx`, `wxProcess`, `wxFile` are woven through every engine header and subsystem. The engine cannot compile without wxWidgets; the devs themselves left a TODO admitting this. |
| Licensing | The engine is **GPL-2.0-or-later**. Linking it (statically or dynamically, even via a C shim) makes the whole app a derivative work → the entire app must be GPL. |
| Age | 2011-era GnuTLS usage (512-bit DH minimum, hard-pinned priority string, no modern cipher policy) — not shippable on today's internet without a security rewrite. |
| Build system | Autotools + a hand-maintained MSVC solution; no CMake. Cross-compiling it plus wxWidgets, GnuTLS, and libidn for macOS + Windows inside a Rust/Tauri pipeline is a project in itself. |
| FFI shape | The public API (`CFileZillaEngine::Command` / `GetNextNotification`) is C++ virtual classes + wx events, not a C ABI. Every boundary type needs manual marshaling. |

**Instead: a pure-Rust engine.** The FileZilla tree stays in the repo strictly as a
*reference implementation* — we study its behavior (state machines, edge cases,
heuristics) but never copy code from it (copying GPL code into an MIT/proprietary app
creates a derivative work; re-implementing observed behavior and documented formats is fine).

What the FileZilla source is genuinely valuable for:

- **`src/engine/directorylistingparser.cpp`** — the catalog of real-world LIST formats
  (Unix, DOS/IIS, EPLF, VMS, MVS, OS/9, z/VM, HP NonStop, MLSD) and date/size quirks.
- **The command → notification API pattern** — `Command()` returns immediately,
  results/questions arrive as typed notifications (`listing`, `transferstatus`,
  `asyncrequest` for file-exists/hostkey/cert prompts). We mirror this as
  Tauri commands + events.
- **Capability & timezone detection** — per-server capability cache (FEAT/MLSD/MFMT
  support, timezone offset inferred by comparing MDTM vs LIST times).
- **One engine instance per connection; the transfer queue lives above the engine.**

## 2. Protocol stack (all MIT/Apache — no GPL exposure)

| Concern | Choice | Notes |
|---|---|---|
| SFTP/SSH | **`russh` + `russh-sftp`** (both Apache-2.0; russh now maintained under warp-tech, v0.62.x; russh-sftp v2.3.x) | Pure Rust, Tokio-async, password/key/agent/keyboard-interactive auth, OpenSSH certs. Parallel transfers = multiple SFTP channels; resume + chunked parallelism built on `RawSftpSession::read/write(handle, offset, …)` — true pread/pwrite. |
| FTP/FTPS | **`suppaftp`** (async + `rustls`) | MLSD/MLST/LIST/NLST, REST/APPE for resume. Fall back to `native-tls` only if old-server interop demands it. |
| TLS | **rustls** | Pure Rust; avoids OpenSSL cross-compile pain on Windows. |
| LIST parsing | MLSD-first; hand-rolled fallback parser | No crate matches FileZilla's hardening. Evaluate `ftp-cmd-list-parse`, but budget for our own Unix/DOS/IIS parser with a golden-sample test corpus. |
| Credentials | **`keyring`** crate | macOS Keychain / Windows Credential Manager — matches the design's "credentials live in macOS Keychain" promise. |
| Avoid | `ssh2` (blocking, C deps), `openssh` (ControlMaster broken on Windows) | Documented fallbacks only. |

### Prior art — adopt-vs-build survey (verified July 2026)

A dedicated due-diligence pass over every candidate repo concluded **build our own
engine**; nothing is fork-worthy:

- **AeroFTP** — the most complete Tauri+Rust client, but **GPL-3.0**: no code reuse;
  don't even read its source with intent to borrow (contamination risk).
- **r-shell** (MIT, active) — Tauri-wiring reference only: no real queue, no resume,
  and its host-key verification is stubbed to accept everything (MITM-vulnerable).
- **remotefs-rs** (MIT) — fully *synchronous* trait, no progress callbacks, core stale
  since 2024; wrong concurrency model for a Tokio/Tauri app.
- **filezilla-rs**, **Fileman** — unlicensed + abandoned; legally unusable.
- **rclone sidecar** (MIT) — no per-transfer pause and no partial resume on FTP/SFTP;
  defeats core requirements.
- **Cyberduck / WinSCP** — entirely GPLv3, no separable core.
- **openssh-sftp-client** — requires OpenSSH ControlMaster, hard-fails on Windows.

## 3. Project structure

```
relay/
├── filezilla/                  # GPL reference source — READ ONLY, never copy code
├── docs/
│   └── design-notes.md         # tokens & specs transcribed from Relay.dc.html
├── src/                        # Frontend: React + TypeScript + Vite
│   ├── app/                    # shell: title bar, tabs, sidebar, theming
│   ├── components/             # panes, queue drawer, flow gutter, palette, sheets
│   ├── views/                  # SessionView, ConnectView, EditorView
│   ├── state/                  # Zustand stores: servers, sessions, transfers, ui
│   ├── ipc/                    # typed wrappers over invoke() + event listeners
│   └── styles/tokens.css       # CSS variables lifted from the design (light+dark)
├── src-tauri/
│   ├── src/
│   │   ├── commands/           # #[tauri::command] surface (thin: parse → core → emit)
│   │   ├── events.rs           # typed event payloads to the frontend
│   │   └── state.rs            # SessionManager handle, settings
│   └── tauri.conf.json
└── crates/
    └── relay-core/             # engine library — NO tauri dependency, testable alone
        ├── protocol/           # trait Protocol: connect/list/get/put/mkdir/rename/
        │   ├── sftp.rs         #   delete/chmod/raw + capability probing
        │   └── ftp.rs
        ├── listing/            # DirEntry model, MLSD + LIST heuristic parsers
        ├── session.rs          # one Session per server tab: state machine,
        │                       #   reconnect w/ backoff, latency probe
        ├── queue.rs            # transfer scheduler: priorities, concurrency (1–8),
        │                       #   pause/retry/reorder, resume offsets, persistence
        ├── trust.rs            # host-key + TLS-cert pinning store (known-hosts style)
        ├── secrets.rs          # keyring integration
        └── events.rs           # engine → app notification enum (mirrors FZ pattern)
```

Design rule carried over from FileZilla: **`relay-core` is a library with a
command-in / event-out API**; the Tauri layer is a thin adapter. This keeps the engine
unit-testable without a GUI and portable if the shell ever changes.

### IPC contract (from the design's implied backend)

Events streamed to the frontend (`emit` per session):
`session:state` (connecting/connected/reconnecting/disconnected + latency),
`listing:updated`, `transfer:progress` (id, bytes, total, speed, eta, status),
`transfer:done|failed`, `prompt:hostkey`, `prompt:conflict` (with resume/rename options),
`log:entry` (human event + raw protocol line — feeds the Activity slide-over).
Prompts follow FileZilla's async-request pattern: engine pauses that operation,
frontend answers via a command (`resolve_prompt`).

Frontend concurrency pitfalls to respect (Tauri v2): use `tauri::async_runtime::spawn`
(not bare `tokio::spawn`); tasks do NOT auto-cancel on window close — every transfer
holds an abort handle tied to its queue entry.

## 4. Frontend plan

`Relay.dc.html` is a high-fidelity functional prototype (a Claude Design "DC" component),
**not lift-and-shift code** — it must be re-implemented as real React components, but its
token system, layout metrics, states, and copy transcribe directly.

What the design specifies (build to this):
- **Shell**: title bar with browser-style session tabs (⌘T/⌘W), collapsible server
  sidebar with groups/color avatars/bookmarks, ⌘K command palette, light/dark themes
  (Space Grotesk / Inter / JetBrains Mono; full CSS-variable token set).
- **Session view**: dual panes (local "This Mac" / remote) with breadcrumbs, per-pane
  filter, list/grid toggle, hover action pills, drag & drop (rows, folders, breadcrumbs,
  OS drops); the signature **animated flow gutter** with traveling transfer pills;
  bottom **queue drawer** (Active/Failed/Completed tabs, drag-to-reprioritize, retry).
- **Flows**: connect screen with live URL parsing (`sftp://user@host`), host-key trust
  sheet, conflict sheet (Overwrite / Skip / Keep both / Resume partial + "apply to
  remaining"), server editor with auth methods + Test connection, settings sheet,
  Quick Look (Space), built-in remote editor tab with save-to-server, activity
  slide-over with raw-log toggle.
- Designed empty/loading/denied/connection-lost states; `prefers-reduced-motion` respected.

Stack: React 18 + TypeScript + Vite + Zustand. No heavy UI framework — the design is
fully custom-tokened. Virtualized file lists (large directories) from day one.

## 5. Feature scope (FileZilla parity, filtered)

**In (MVP–v1.0):** quick connect (URL bar), server store w/ groups + bookmarks, SFTP +
FTP + FTPS, auth password/key/agent/ask, host-key & cert trust with pinning, dual-pane
browse w/ perms column, hidden-file handling, transfer queue (priorities via reorder,
retry, pause, persistence across restart), resume + full conflict action set, concurrent
transfers (1–8), remote edit (built-in editor + external-editor watch → re-upload),
Quick Look, activity/raw log, import/export of servers, keychain storage.

**v1.x (post-1.0, FileZilla features the design omits but users expect):** chmod dialog
(perms are display-only in the design), speed limits, remote file search, directory
comparison, synchronized browsing, filename filter sets, per-site charset override,
timezone-offset handling, queue-completion actions.

**Out (legacy):** FTP proxy format strings, active-mode network wizard (default to
passive; active mode config only in advanced settings), icon theme packs, VMS/MVS/OS-9
server hints (parser keeps Unix/DOS/IIS/MLSD only unless demand appears), update
checker (replaced by Tauri updater), shell-extension drag & drop.

## 6. Roadmap

> Detailed technical implementation plans per phase live in `docs/phases/`:
> [Phase 0](docs/phases/phase-0-foundation.md) · [Phase 1](docs/phases/phase-1-sftp-slice.md) ·
> [Phase 2](docs/phases/phase-2-transfer-queue.md) · [Phase 3](docs/phases/phase-3-ftp-ftps.md) ·
> [Phase 4](docs/phases/phase-4-relay-experience.md) · [Phase 5](docs/phases/phase-5-ship.md)

### Phase 0 — Foundation (week 1)
- Scaffold: Tauri v2 app + Cargo workspace with `relay-core`, React/Vite/TS frontend, CI
  (GitHub Actions matrix: macOS + Windows, `tauri-action` builds).
- Transcribe design tokens → `tokens.css`; static app shell (tabs, sidebar, panes,
  drawer) with mock data — pixel-match the design early.
- Define the full IPC contract as shared types (Rust structs ↔ TS types; consider `specta`/`tauri-specta` for codegen).

### Phase 1 — SFTP vertical slice (weeks 2–4)
- `relay-core`: Session + Protocol trait; SFTP backend (russh): connect, auth
  (password/key/agent), host-key trust store + prompt flow, list, download/upload with
  progress events, mkdir/rename/delete.
- Local filesystem pane (Rust-side listing for hidden files/metadata consistency).
- Wire real data into the UI: connect screen → session tab → browse → single transfers
  with flow-gutter pills. Keychain storage. **Milestone: usable SFTP client.**

### Phase 2 — Transfer queue done right (weeks 5–6)
- Scheduler: queue, concurrency limit, priorities/reorder, pause-all, retry w/ backoff,
  cancel via abort handles, queue persistence (SQLite via `rusqlite` or JSON).
- Resume: byte-offset resume (SFTP random access; FTP REST later), conflict sheet with
  full action set incl. "apply to remaining N" and keep-both rename.
- Recursive folder transfers (traversal engine ≈ FileZilla's recursive_operation).
- Auto-reconnect with queue preservation + connection-lost banner.

### Phase 3 — FTP/FTPS (weeks 7–9)
- suppaftp backend behind the same Protocol trait; rustls FTPS + cert trust sheet;
  INSECURE_FTP warnings per the design.
- Listing: FEAT/MLSD detection; heuristic LIST parser (Unix/DOS/IIS) with a golden-file
  test corpus harvested from real servers + FileZilla's documented formats.
- Parallel FTP transfers = additional control connections (per-site connection cap).
- Integration tests against dockerized vsftpd/ProFTPD/pure-ftpd + openssh-sftp in CI.

### Phase 4 — The Relay experience (weeks 10–12)
- Command palette actions, Quick Look, built-in editor tab (save-to-server + local
  backup), external-editor watch → upload prompt, activity feed + raw log toggle,
  server editor Test-connection, settings (theme, density, concurrency, default
  conflict action), toasts, bookmarks, import/export (incl. FileZilla sitemanager.xml
  import — parse their XML, don't link their code).

### Phase 5 — Ship (weeks 13–14)
- macOS: signing + notarization, universal binary (aarch64 + x86_64).
- Windows: MSI/NSIS via Tauri bundler, code-signing cert.
- Tauri updater plugin + release pipeline; crash/log capture; perf pass
  (10k-file directories, memory under long transfer sessions).
- v1.0 → then v1.x parity items from §5.

## 7. Risks & open questions

1. **LIST parsing breadth** — the eternal FTP tarpit. Mitigation: MLSD-first, corpus
   tests, ship with "server type" override like FileZilla's.
2. **russh-sftp maturity** — smaller project than russh; pin versions, keep `ssh2` as a
   documented escape hatch, verify its license at pin time.
3. **FTPS interop with ancient servers** (session reuse requirements, odd TLS stacks) —
   rustls may reject what native-tls tolerates; keep the TLS backend feature-flagged.
4. **GPL hygiene** — contributors must treat `filezilla/` as read-only reference.
   Consider moving it to a separate `reference/` clone outside the shipped repo before
   open-sourcing Relay.
5. **Windows agent auth** — russh agent support vs. Pageant/OpenSSH-agent-on-Windows
   named pipes; needs an early spike in Phase 1.
6. **Performance** — virtualized lists + Rust-side sorting/filtering for huge
   directories; throttle progress events (~10 Hz) so the webview isn't flooded.
