# Relay

A lightweight, cross-platform file-transfer client for macOS and Windows, with a Rust
engine behind a custom-designed interface. The first release focuses on dependable
SFTP; FTP/FTPS and richer editing tools follow after that release.

> **Status: pre-implementation.** The architecture and implementation plans are written
> down; no application code exists yet. Phase 0 is next, including compatibility
> prototypes to validate the protocol libraries. Delivery follows acceptance gates,
> with estimates revisited after those prototypes and the SFTP slice.

## Shape of the thing

| Layer | Choice |
|---|---|
| Shell | Tauri v2 (macOS + Windows) |
| Engine | `relay-core` — a Tauri-free Rust library with a command-in / event-out API |
| SFTP | `russh` + `russh-sftp`, subject to Phase 0 interoperability checks |
| FTP / FTPS | `suppaftp` + `rustls` — early prototype, post-1.0 product support |
| Frontend | React 19 + TypeScript + current stable Vite + Zustand, custom design tokens |
| Queue persistence | SQLite via `rusqlite`; Rust owns durable transfer state |
| IPC | Generated Rust/TypeScript bindings, ordered channels, recoverable state snapshots |
| Secrets | `keyring` — macOS Keychain / Windows Credential Manager |

The FileZilla C++ engine is deliberately **not** reused: wxWidgets is load-bearing
throughout it, it is GPL-2.0-or-later, and its 2011-era TLS stack is not shippable.
See [`ROADMAP.md`](ROADMAP.md) §1 for the full analysis.

## Repository map

```
relay/
├── ROADMAP.md                     # architecture decisions, scope, risks, release gates
├── docs/phases/                   # per-phase technical implementation plans
├── claude-design-relay/           # Relay.dc.html — the design source of truth
└── reference/                     # untracked GPL reference sources (see its README)
```

## Plan

Detailed plans live in [`docs/phases/`](docs/phases/). Phase identifiers are retained
for stable links; execution order is **0 → 1 → 2 → 5 → 3 → 4**. Phase 5's release
checks are repeated for later releases. There is no fixed 14-week release commitment.

| Phase | Release | Outcome |
|---|---|---|
| [0 — Foundation](docs/phases/phase-0-foundation.md) | SFTP 1.0 | Protocol prototypes, CI-green workspace, design foundation, typed IPC and recovery |
| [1 — SFTP slice](docs/phases/phase-1-sftp-slice.md) | SFTP 1.0 | Usable SFTP browsing, authentication, and single-file transfers |
| [2 — Transfer queue](docs/phases/phase-2-transfer-queue.md) | SFTP 1.0 | Scheduler, verified resume, conflicts, recursive transfers, SQLite persistence |
| [5 — Ship](docs/phases/phase-5-ship.md) | SFTP 1.0 | Essential UX, reliability tests, beta, signed installers and updater |
| [3 — FTP/FTPS](docs/phases/phase-3-ftp-ftps.md) | Post-1.0 | Second protocol family, LIST parsing, interoperability tests |
| [4 — Relay experience](docs/phases/phase-4-relay-experience.md) | Post-1.0 | Command palette, editors, Quick Look, richer activity and import/export |

## Contributing

One hard rule: **`reference/` is read-only.** Never copy code from it — not verbatim, not
translated, not line-by-line adapted. Read it to understand behavior, then implement
independently. See [`reference/README.md`](reference/README.md).
