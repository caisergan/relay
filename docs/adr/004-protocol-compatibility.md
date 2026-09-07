# ADR 004 — Protocol stack, and what the compatibility gates actually showed

**Status:** SFTP gates **passed** on Linux against real OpenSSH (2026-09-07).
FTP/FTPS, OS keychains, and the Windows agent remain **unrun**. Read the two halves of
this document separately: the first is measured, the second is not.

## Pinned versions

| Concern | Pin | Verified |
|---|---|---|
| SSH/SFTP | `russh 0.63.2`, `russh-sftp 2.4.0` | Exercised against OpenSSH; see below |
| Crypto provider | `ring` — `russh` with `default-features = false, features = ["flate2", "ring", "rsa"]` | Builds and negotiates on Linux |
| FTP/FTPS | `suppaftp 11.0.0`, `async-secure` + `tokio-rustls-ring`, behind the non-default `ftp` feature | Manifest only |
| Queue persistence | `rusqlite 0.40.2`, `bundled` | Compiles; bundled SQLite builds with the system C toolchain |
| Secrets | `keyring 4.2.0` (default `v1`) | Compiles. **No keychain call has been made on either shipped platform** |
| Toolchain | Rust 1.97.0, edition 2024, resolver 3 | Workspace green |

One crypto provider across `russh`, `rustls` and `suppaftp`, so one stack is tested
rather than two linked into one binary. `ring` was preferred over `russh`'s default
`aws-lc-rs` for lighter build-time requirements on Windows. That prediction is
**currently unverified**: the Windows CI leg is switched off (see `.github/workflows/ci.yml`),
so nothing is building this stack for Windows. `aws-lc-rs` remains the documented fallback.

## The fixture

`scripts/sftp-fixture.sh` runs `atmoz/sftp:alpine` (OpenSSH) with fixed host keys, a
password user, three client keys — ed25519, RSA-3072, and a passphrase-protected
ed25519 — and a host-mounted data directory so uploads can be checked outside the
client under test. `rotate` restarts it presenting a *different* host key on the same
host and port, which is what makes a changed-key test possible.

```sh
./scripts/sftp-fixture.sh up > target/sftp-fixture/env.sh
set -a; . target/sftp-fixture/env.sh; set +a
cargo test -p relay-core --features integration --test sftp_prototype -- --test-threads=1 --nocapture
```

Harness: `crates/relay-core/tests/sftp_prototype.rs`. It tests `russh`, not Relay's
engine — that is the point.

## Results — 13 prototypes, all passing

Host: Debian 12, Linux 6.1, x86_64, 4 cores. Server reached over loopback.

### Authentication

| Case | Result |
|---|---|
| Password | Succeeds |
| Wrong password | `AuthResult::Failure`, not a silent success |
| ed25519 public key | Succeeds |
| RSA-3072 public key (`rsa-sha2-256`) | Succeeds |
| Passphrase-protected key, no passphrase | `load_secret_key` returns `KeyIsEncrypted` — a **distinguishable** error, which is what phase 1 turns into a passphrase prompt rather than a generic failure |
| Passphrase-protected key, with passphrase | Succeeds |
| Agent over `SSH_AUTH_SOCK` | Succeeds. `AgentClient::connect_uds` → `request_identities` → `authenticate_publickey_with`. This is the same mechanism macOS uses |

**Keyboard-interactive is not proven.** The fixture's sshd advertises it
(`MethodSet([PublicKey, Password, KeyboardInteractive])`) but
`authenticate_keyboard_interactive_start` returns `Failure` without issuing a prompt,
because the image has no PAM conversation configured. The client-side code path is
written and reachable; that a server which *requires* keyboard-interactive can be
satisfied is still an open question, and phase 1 needs a fixture that forces it.

### Host keys

- Refusing the key inside `check_server_key` fails the handshake **before**
  authentication is attempted. Credentials are never offered to an untrusted server.
- After `rotate`, a handler pinned to the original fingerprint sees the new key
  (`SHA256:0cHMTGq5…` where it expected `SHA256:NxWl5MjU…`) and refuses the connection.
  This is the shape phase 1's trust store plugs into: the decision is made inside the
  handshake, from an `Arc` the handler already holds.

### Concurrency and cancellation

Two 32 MiB downloads on separate SFTP channels of one SSH connection, with a third
channel listing a directory throughout:

```
lane: 33554432 bytes in 2.79s (11.5 MiB/s)
lane: 33554432 bytes in 2.80s (11.4 MiB/s)
browsing stayed responsive: 40 listings, worst 94.7 ms
```

Browsing is not starved by transfers, which is the assumption ADR 002's lane design
rests on. Cancellation was observed **1.28 ms** after the token flipped, and the
session remained usable afterwards — a cancelled lane does not poison the connection,
so phase 2 has something left to retry on.

### Interrupted transfer and resume

- A download cut at an unaligned 393,327 bytes and resumed **on a fresh channel** from
  that offset produced a file whose SHA-256 matches the source exactly.
- Chunked writes to a temporary remote path followed by `rename` over the destination
  land byte-for-byte. Atomic finalisation is available on this server.
- **The evidence for ADR 005.** A 256 KiB file was rewritten with identical length and
  different content. Afterwards:

  ```
  size before/after:  Some(262144)/Some(262144)
  mtime before/after: Some(1788783784)/Some(1788783784)
  ```

  Size *and* modification time were unchanged, because the rewrite happened inside the
  same one-second granularity. Only the content digest caught it. Any resume policy
  built on size, or on size plus mtime, would have spliced two different files together
  and reported success.

### Chunk sizing

```
chunk  32 KiB: 15.1 MiB/s        chunk 128 KiB: 17.3 MiB/s
chunk  64 KiB: 16.0 MiB/s        chunk 256 KiB: 27.8 MiB/s
```

Larger chunks win, and 128 KiB — phase 1's starting figure — is not obviously the right
default. **But this is loopback.** What it measures is per-request round-trip overhead
with a near-zero network, so it is a lower bound on the benefit of larger chunks and
says nothing about behaviour at 50 ms RTT. Re-measure with injected latency
(`--cap-add=NET_ADMIN` plus `tc netem` in the fixture container) before choosing a
default or deciding whether request pipelining is needed.

## Still open

| Gate | Why it is not answered |
|---|---|
| Windows OpenSSH named-pipe agent | Needs Windows. The unix-socket path is proven; the named-pipe path is not the same code |
| Anything else Windows | The Windows CI leg is currently disabled, so `src-tauri` is not even compiled for it |
| OS secrets — Keychain and Credential Manager | `keyring` compiles and nothing more. Write/read/delete and denied-store behaviour on both platforms are untested |
| Throughput under realistic latency | Loopback only |
| Keyboard-interactive success path | Needs a server configured to require it |
| FTPS session reuse | Not run |
| FTPS certificate interaction | Not run |

A failed SFTP gate would have had to be resolved before phase 1. None failed. The FTPS
gates stay open without blocking SFTP 1.0, as phase 0 §0.6 allows. Pageant remains out
of scope.
