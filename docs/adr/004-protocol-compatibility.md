# ADR 004 — Protocol stack, and the compatibility gates that are still open

**Status:** provisional. Dependency versions are pinned and building; **no protocol
prototype has been run against a real server.** Do not read this as validation.

## Pinned, and how far that has been verified

| Concern | Pin | Verified so far |
|---|---|---|
| SSH/SFTP | `russh 0.63.2`, `russh-sftp 2.4.0` | Compiles and links (Linux, `cargo check`) |
| Crypto provider | `ring`, via `russh` `default-features = false, features = ["flate2", "ring", "rsa"]` | Builds on Linux; the reason for the choice is below |
| FTP/FTPS | `suppaftp 11.0.0`, features `async-secure` + `tokio-rustls-ring`, behind the non-default `ftp` feature | Manifest only; not built in the default profile |
| Queue persistence | `rusqlite 0.40.2`, `bundled` | Compiles; bundled SQLite builds with the system C toolchain |
| Secrets | `keyring 4.2.0` (default `v1` feature) | Compiles on Linux; **no keychain call has been made on macOS or Windows** |
| Toolchain | Rust 1.97.0, edition 2024, resolver 3 | Workspace builds, clippy clean, 22 tests pass |

### Why `ring` rather than `aws-lc-rs`

One crypto provider across `russh`, `rustls`, and `suppaftp`, so a single stack is
tested rather than two linked into one binary. `ring` was chosen over `russh`'s default
`aws-lc-rs` for having the lighter build-time requirements on Windows — but this is a
prediction, not a measurement, until the Windows CI leg runs. If it fails there,
`aws-lc-rs` is the documented fallback and this table is where the result goes.

## The gates. All of them are pending

Phase 0 §0.6 is explicit that a timebox ending is not a passed gate. Nothing below has
run. Each needs a reproducible harness and a recorded result — crate versions, OS and
server versions, exact commands, and what actually happened.

| Prototype | Required evidence | Status |
|---|---|---|
| SFTP authentication | Password, encrypted key, keyboard-interactive, macOS agent, Windows OpenSSH named-pipe agent; rejected and changed host keys fail correctly | **Not run** |
| SFTP concurrency and cancellation | Browse while two channels transfer; stalled sockets; bounded cancellation and cleanup; throughput at realistic latency | **Not run** against a real server. The shape is proven against the in-memory backend only |
| Interrupted transfer | Both directions; disconnect and hard-kill; recover a recorded partial; compare hashes; same-size source mutation and corrupt partial refuse resume | **Not run** |
| FTPS session reuse | Real control and protected data connections against vsftpd and ProFTPD with reuse required | **Not run** |
| FTPS certificate interaction | Trusted chain succeeds; rejected certificate fails; an exception binds to the exact host, port, and certificate | **Not run** |
| OS secrets | Keychain and Credential Manager write/read/delete, plus missing and denied store behaviour, on both platforms | **Not run** |

A failed SFTP gate must be resolved before phase 1. A failed FTPS gate records a
deferred backend decision and does not block SFTP 1.0.

## What the in-memory backend does and does not tell us

`MockBackend` proves the *interface* holds up: lanes are independently owned, transfers
do not block browsing, cancellation is bounded and cleans up only what it owns, an
unbacked resume offset is refused. It proves nothing about `russh`. Every row in the
table above is about the difference between those two statements.
