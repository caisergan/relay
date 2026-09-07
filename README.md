# Relay

A lightweight, cross-platform file-transfer client for macOS and Windows, with a Rust
engine behind a custom-designed interface. The first release focuses on dependable
SFTP; FTP/FTPS and richer editing tools follow after that release.

> **Status: Phase 1 in progress.** Phase 0 is done and the mock engine is gone.
> Relay now opens **real SFTP sessions**: a session actor per connection, host keys
> checked inside the handshake against a pinned trust store, an auth ladder over
> agent / key file / password, browsing, file operations, and transfers on their own
> channels with bounded cancellation. Fourteen integration tests run the backend
> against a real OpenSSH server on every push. Still unrun: the OS keychains on the
> machines that ship, FTPS, and the Windows named-pipe agent.
> [ADR 004](docs/adr/004-protocol-compatibility.md) has the numbers and the open
> gates; believe it over this paragraph.

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
├── docs/adr/                      # decisions, with the evidence behind them
├── crates/relay-core/             # the engine: no GUI dependency, tests headlessly
├── src-tauri/                     # the shell: a thin adapter over relay-core
├── src/                           # React 19 frontend; src/ipc/gen.ts is generated
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

## Working on it

```sh
pnpm install
cargo test -p relay-core     # the engine, headless, no webview needed
pnpm test                    # frontend unit tests
pnpm gen:ipc                 # regenerate src/ipc/gen.ts from the Rust types
pnpm tauri dev               # the app — needs a macOS or Windows machine

# Against a real OpenSSH server in Docker. `sftp_prototype` tests the library
# (phase 0); `sftp_integration` tests Relay's own backend (phase 1).
./scripts/sftp-fixture.sh up > target/sftp-fixture/env.sh
set -a; . target/sftp-fixture/env.sh; set +a
cargo test -p relay-core --features integration -- --test-threads=1
./scripts/sftp-fixture.sh down
```

`relay-core` builds and tests anywhere Rust does. `src-tauri` needs a platform with a
webview, so on Linux use the engine and frontend targets and let CI cover the shell.

### Running a CI build on macOS

Unzipping `relay-macos-aarch64-debug` and double-clicking the result will usually just
bounce the icon and stop — no dialog, no crash report, nothing in the log. Install it
with the script instead:

```sh
scripts/install-macos-build.sh                    # newest zip in ~/Downloads
scripts/install-macos-build.sh path/to/Relay-app.zip
```

It copies the bundle to `~/Applications`, ad-hoc signs it, launches it, and checks that
it reached a webview rather than reporting success because `open` returned 0.

The reason the plain double-click fails: the download is quarantined and Relay is only
ad-hoc signed until phase 5, so the security assessment rejects it — and that verdict is
cached against the bundle's *file identity*. LaunchServices keys the app by inode
(`application.app.relay.desktop.<inode>.<n>` in the log), and a rejected bundle stalls at
`_dyld_start` at about 32 KB resident, where a healthy one settles above 100 MB. Running
`xattr -dr com.apple.quarantine` on it or re-signing it in place does not lift the block;
the bundle needs a new inode. Copying it elsewhere is what actually fixes it, which is
all the script is doing. The directory does not matter — a fresh copy runs even from
`~/Downloads`; the reused inode is the problem.

## Contributing

One hard rule: **`reference/` is read-only.** Never copy code from it — not verbatim, not
translated, not line-by-line adapted. Read it to understand behavior, then implement
independently. See [`reference/README.md`](reference/README.md).
