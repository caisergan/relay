# Relay — Structure Plan & Roadmap

A lightweight, cross-platform (macOS + Windows) FTP/FTPS/SFTP client built with Tauri v2,
re-imagining FileZilla with the Relay design (Claude Design project `Relay.dc.html`).

**Delivery decision (2026-09-07):** keep Tauri + Rust + React, ship dependable SFTP
as 1.0, and add FTP/FTPS and richer editing workflows afterward. The previous
14-week schedule is superseded by acceptance gates. The protocol choices remain
subject to executable compatibility checks in Phase 0; no prototypes have run yet.

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
| SFTP/SSH | **`russh` + `russh-sftp`** | Preferred Tokio-based backend, gated on Windows OpenSSH-agent auth, channel concurrency, cancellation, and recovery prototypes. Pin tested versions/features in Phase 0. Offset reads/writes enable resume mechanics; source and partial-file verification establish correctness. |
| FTP/FTPS | **`suppaftp`** (Tokio + `rustls`) | Post-1.0 backend; test FTPS control/data TLS session reuse in Phase 0. Consider `native-tls` only after a reproducible interoperability failure and a documented decision. |
| TLS | **rustls** | Preferred TLS stack; select and verify its crypto provider and native build requirements on both platforms during the prototype. |
| LIST parsing | MLSD-first; hand-rolled fallback parser | No crate matches FileZilla's hardening. Evaluate `ftp-cmd-list-parse`, but budget for our own Unix/DOS/IIS parser with a golden-sample test corpus. |
| Credentials | **`keyring`** crate | macOS Keychain / Windows Credential Manager — matches the design's "credentials live in macOS Keychain" promise. |
| Queue persistence | **SQLite via `rusqlite` (bundled)** | Required for durable jobs, resume records, and transactional state changes; no JSON queue alternative. |
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
├── reference/filezilla/        # optional untracked reference — READ ONLY, never copy code
├── docs/
│   └── design-notes.md         # tokens & specs transcribed from Relay.dc.html
├── src/                        # Frontend: React + TypeScript + Vite
│   ├── app/                    # shell: title bar, tabs, sidebar, theming
│   ├── components/             # panes, queue drawer, flow gutter, palette, sheets
│   ├── views/                  # SessionView, ConnectView, EditorView
│   ├── state/                  # Zustand stores: servers, sessions, transfers, ui
│   ├── ipc/                    # typed commands, channel bridge, snapshot recovery
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

### Runtime and IPC contract

`relay-core` accepts a `tokio::runtime::Handle` and uses it to spawn engine tasks;
it imports no Tauri types. The shell supplies the handle and owns application
lifecycle integration. Tests create their own Tokio runtime. Session and transfer
tasks use cancellation tokens and tracked join handles: shutdown requests cooperative
checkpoint/cleanup, waits for a bounded grace period, and aborts only as a last resort.

Rust owns sessions, jobs, prompts, and durable state. Zustand holds their frontend
projection plus local UI state; it never independently decides transfer transitions.
One generated, typed IPC bridge translates commands and consumes ordered Tauri
channels for engine updates (session/listing/job/prompt state, throttled progress,
and bounded logs). File-transfer bytes stay in Rust. Small optional UI notifications
may use events. Tauri recommends channels for ordered streaming rather than its
global event mechanism. [Tauri IPC documentation](https://v2.tauri.app/develop/calling-frontend/)

Provide `engine_subscribe` and `engine_snapshot` with an engine epoch and sequence
watermark. Subscribe and buffer before requesting a consistent snapshot, then apply
only newer updates. A gap, overflow, remount, or engine restart triggers resnapshot
and reconciliation; pending prompts are included. Bounded queues coalesce progress
before sequencing and retain terminal states through the authoritative snapshot.
Prompts pause only the affected operation and resolve by id via `resolve_prompt`.

## 4. Frontend plan

`Relay.dc.html` is a high-fidelity functional prototype (a Claude Design "DC" component),
**not lift-and-shift code** — it must be re-implemented as real React components, but its
token system, layout metrics, states, and copy transcribe directly.

What the design specifies (target experience; release scope is in §5):
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

Stack: React 19 + TypeScript strict + current stable Vite + Zustand. At the September
2026 review, React's current line is 19.2 and Vite's regularly patched line is 8.2;
Vite 6.4 receives security fixes only. Resolve compatible stable patch versions at
scaffolding time and commit lockfiles. [React versions](https://react.dev/versions),
[Vite release policy](https://vite.dev/releases)

No heavy UI framework — the design is fully custom-tokened. Virtualized file lists
from day one, narrow store subscriptions, and batched progress updates. Validate the
custom title bar, keyboard navigation, focus, and OS file drops on both macOS and
Windows in Phase 0/1; platform webviews need platform testing. Essential accessibility
and error states ship with SFTP 1.0. Editors, previews, and the full palette can wait.

## 5. Feature scope (FileZilla parity, filtered)

**SFTP 1.0:** quick connect, saved-server CRUD and Test connection, password/key/agent/
ask authentication, host-key trust with pinning, OS keychain storage, dual-pane browse,
permissions display, hidden files, rename/delete/mkdir, recursive transfers, concurrent
transfers (1–8), persistent queue with reorder/retry/pause/cancel, verified resume,
Overwrite/Skip/KeepBoth conflicts, reconnect, essential settings and status/error logs,
keyboard/focus accessibility, signed installers, and updater. Resume is offered only
when source/partial verification succeeds. FTP/FTPS selections are unavailable in 1.0.

**Post-1.0 product expansion:** FTP/FTPS with certificate trust and LIST parsing
(Phase 3); built-in/external editors, Quick Look, command palette, richer activity UI,
server groups/bookmarks, and import/export (Phase 4). These do not block SFTP release.

**Later parity work:** chmod dialog
(perms are display-only in the design), speed limits, remote file search, directory
comparison, synchronized browsing, filename filter sets, per-site charset override,
timezone-offset handling, queue-completion actions.

**Out (legacy):** FTP proxy format strings, active-mode network wizard (default to
passive; active mode deferred), icon theme packs, VMS/MVS/OS-9
server hints (parser keeps Unix/DOS/IIS/MLSD only unless demand appears), update
checker (replaced by Tauri updater), shell-extension drag & drop.

## 6. Roadmap

> Detailed technical implementation plans per phase live in `docs/phases/`:
> [Phase 0](docs/phases/phase-0-foundation.md) · [Phase 1](docs/phases/phase-1-sftp-slice.md) ·
> [Phase 2](docs/phases/phase-2-transfer-queue.md) · [Phase 3](docs/phases/phase-3-ftp-ftps.md) ·
> [Phase 4](docs/phases/phase-4-relay-experience.md) · [Phase 5](docs/phases/phase-5-ship.md)

Execution order: **0 → 1 → 2 → 5 → 3 → 4**. Existing phase numbers and filenames
remain stable. Phase 5 is a reusable release gate, first applied to SFTP 1.0 and
repeated for later protocol/experience releases. Re-estimate after Phase 0 evidence
and again after the Phase 1 slice; record effort ranges and unresolved dependencies
then. Do not turn the previous week numbers into deadlines or skip beta to meet them.

### Phase 0 — Foundation and compatibility gates
- Scaffold: Tauri v2 app + Cargo workspace with `relay-core`, React/Vite/TS frontend, CI
  (GitHub Actions matrix: macOS + Windows, `tauri-action` builds).
- Prove SFTP auth (including Windows OpenSSH agent), concurrent browse/transfer,
  cancellation, and interrupted-transfer verification with a small engine harness.
- Probe FTPS session reuse with actual control/data connections before declaring that
  backend settled. A failed FTPS prototype records a deferred decision, without
  blocking SFTP work; a failed SFTP requirement must be resolved before Phase 1.
- Establish design tokens and a representative static shell; prioritize compatibility
  evidence over complete visual polish. Validate keychain and platform interactions.
- Generate shared IPC types; prove channel subscription, snapshot recovery, and an
  engine test run with no Tauri dependency. Record exact tested dependency versions.

### Phase 1 — SFTP vertical slice
- `relay-core`: Session + Protocol trait; SFTP backend (russh): connect, auth
  (password/key/agent), host-key trust store + prompt flow, list, download/upload with
  progress events, mkdir/rename/delete.
- Local filesystem pane (Rust-side listing for hidden files/metadata consistency).
- Wire real data into the UI: connect screen → session tab → browse → single transfers
  with flow-gutter pills. Keychain storage. **Milestone: usable SFTP client.**

### Phase 2 — Durable SFTP queue
- Scheduler: queue, concurrency limit, priorities/reorder, pause-all, retry w/ backoff,
  cooperative cancellation, SQLite persistence via `rusqlite`, schema migrations.
- Resume: source facts + partial ownership + verified content prefix + durable
  offsets; changed or unverifiable source requires restart/skip/cancel. Final size
  alone is insufficient. Conflict actions include "apply to remaining" and keep-both.
- Recursive folder transfers (traversal engine ≈ FileZilla's recursive_operation).
- Auto-reconnect and app/UI restart recovery with queue preservation.

### Phase 5 — Ship SFTP 1.0 (before Phases 3 and 4)
- Complete essential settings, status/error logs, keyboard/focus behavior, and all
  loading/permission/connection-loss states needed by the SFTP flows.
- Fault injection, hash-verified recovery, supported-platform QA, and a private beta.
- macOS: signing + notarization, universal binary (aarch64 + x86_64).
- Windows: signed NSIS installer; updater and release pipeline on both platforms.
- Release only after Phase 5's SFTP exit criteria pass. Editors, LIST parsing, and
  FTP/FTPS interoperability are not prerequisites for SFTP 1.0.

### Phase 3 — FTP/FTPS (post-1.0)
- suppaftp backend behind the same Protocol trait; rustls FTPS + cert trust sheet;
  INSECURE_FTP warnings per the design.
- Listing: FEAT/MLSD detection; heuristic LIST parser (Unix/DOS/IIS) with a golden-file
  test corpus harvested from real servers + FileZilla's documented formats.
- Parallel FTP transfers = additional control connections (per-site connection cap).
- Integration tests against dockerized vsftpd/ProFTPD/pure-ftpd + openssh-sftp in CI.

### Phase 4 — The Relay experience (post-1.0)
- Command palette actions, Quick Look, built-in editor tab (save-to-server + local
  backup), external-editor watch → upload prompt, activity feed + raw log toggle,
  extended settings, bookmarks, import/export (incl. FileZilla sitemanager.xml
  import — parse their XML, don't link their code).
- Repeat Phase 5's applicable regression, beta, packaging, and updater checks for
  each later release; add protocol/editor-specific acceptance criteria.

## 7. Risks & open questions

1. **SFTP interoperability** — Phase 0 must prove Windows agent auth, channel
   ownership, bounded cancellation, and throughput under realistic latency. Record
   versions, commands, platforms, results, and fallback decisions in a compatibility ADR.
2. **Resume correctness** — offsets and matching sizes do not establish content
   identity. Test source replacement (including same-size changes), partial corruption,
   stale checkpoints, and forced termination. Prefer a safe restart to uncertain resume.
3. **FTPS interoperability** — prove session reuse and certificate handling early;
   a shared TLS cache alone is not acceptance evidence. Backend changes require a
   reproducible failing server fixture. Full FTP/FTPS support stays post-1.0.
4. **UI/runtime boundary** — Tauri-free core, nonblocking scheduler, snapshot recovery,
   and Rust-owned job transitions are architectural requirements, verified in P0–P2.
5. **Platform behavior and performance** — test macOS and Windows webviews early;
   virtualize listings, batch progress (~10 Hz per job), and bound event/log buffers.
6. **LIST parsing breadth** — post-1.0, MLSD-first with a Unix/DOS/IIS corpus and
   explicit degraded states; expand formats only with demonstrated demand.
7. **Reference handling** — `reference/` remains read-only and untracked except its
   README; implement independently and never copy reference code.
8. **Delivery scope** — estimate from prototype and slice results. Schedule beta and
   signing setup explicitly; do not promise full FileZilla parity on the original timeline.
