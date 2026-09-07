# Relay

A lightweight, cross-platform FTP / FTPS / SFTP client for macOS and Windows — FileZilla's
capability set rebuilt on a pure-Rust engine behind a custom-designed interface.

> **Status: pre-implementation.** The architecture, protocol stack, and 14-week delivery
> plan are settled and written down; no application code exists yet. Phase 0 is next.

## Shape of the thing

| Layer | Choice |
|---|---|
| Shell | Tauri v2 (macOS + Windows) |
| Engine | `relay-core` — a Tauri-free Rust library with a command-in / event-out API |
| SFTP | `russh` + `russh-sftp` (Apache-2.0) |
| FTP / FTPS | `suppaftp` + `rustls` |
| Frontend | React 18 + TypeScript + Vite + Zustand, fully custom design tokens |
| Secrets | `keyring` — macOS Keychain / Windows Credential Manager |

The FileZilla C++ engine is deliberately **not** reused: wxWidgets is load-bearing
throughout it, it is GPL-2.0-or-later, and its 2011-era TLS stack is not shippable.
See [`ROADMAP.md`](ROADMAP.md) §1 for the full analysis.

## Repository map

```
relay/
├── ROADMAP.md                     # architecture decisions, scope, risks, 6-phase plan
├── docs/phases/                   # per-phase technical implementation plans
├── claude-design-relay/           # Relay.dc.html — the design source of truth
└── reference/                     # untracked GPL reference sources (see its README)
```

## Plan

Detailed per-phase plans live in [`docs/phases/`](docs/phases/):

| Phase | Weeks | Outcome |
|---|---|---|
| [0 — Foundation](docs/phases/phase-0-foundation.md) | 1 | CI-green Tauri workspace, static UI matching the design, IPC contract defined as types |
| [1 — SFTP slice](docs/phases/phase-1-sftp-slice.md) | 2–4 | A genuinely usable SFTP client |
| [2 — Transfer queue](docs/phases/phase-2-transfer-queue.md) | 5–6 | Scheduler, resume, conflicts, recursive transfers, persistence |
| [3 — FTP/FTPS](docs/phases/phase-3-ftp-ftps.md) | 7–9 | Second protocol family, LIST parsing, dockerized interop tests |
| [4 — Relay experience](docs/phases/phase-4-relay-experience.md) | 10–12 | Command palette, editor, Quick Look, activity feed, settings |
| [5 — Ship](docs/phases/phase-5-ship.md) | 13–14 | Signed + notarized installers, updater, perf pass, v1.0 |

## Contributing

One hard rule: **`reference/` is read-only.** Never copy code from it — not verbatim, not
translated, not line-by-line adapted. Read it to understand behavior, then implement
independently. See [`reference/README.md`](reference/README.md).
