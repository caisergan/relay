# Phase 3 — FTP / FTPS (weeks 7–9)

Objective: the second protocol family behind the same `Protocol` trait, including the
genuinely hard part — directory listing parsing — plus TLS trust, resume via REST, and
multi-connection parallelism. After this phase the protocol abstraction is proven
(two very different backends), which is the real test of the engine design.

## 3.1 FtpBackend on suppaftp (async + rustls)

Connect sequence:
1. TCP connect → for `Ftps` (explicit): `AUTH TLS` before login; reject downgrade.
   For `Ftp` (plain): proceed, but emit the design's "Unencrypted" badge state
   (`ServerInfo.encrypted = false`) and require the user to have clicked through the
   red warning in the connect flow (per design).
2. Login (USER/PASS via SecretSource/prompt; `anonymous` support with email password
   convention).
3. Post-login probes, cached per server (FileZilla's capability lesson):
   `FEAT` → parse MLSD/MLST/UTF8/REST STREAM/MFMT/SIZE/EPSV support;
   `OPTS UTF8 ON` when advertised; `SYST` → server type hint;
   `TYPE I` always (binary; no ASCII mode in v1 — modern reality, document it).
4. `PWD` for landing dir; emit Connected with TLS details (cipher via rustls handle)
   or `encrypted:false`.

```rust
pub struct ServerCaps {                    // persisted per server_id in caps.json
    pub mlsd: bool, pub mlst: bool, pub utf8: bool, pub rest_stream: bool,
    pub mfmt: bool, pub size_cmd: bool, pub epsv: bool,
    pub syst: Option<String>, pub tz_offset_min: Option<i32>,
}
```

Operation mapping:
- `list`: MLSD when `caps.mlsd` (structured facts: type/size/modify/UNIX.mode/
  UNIX.owner — parse "facts" per RFC 3659) else `LIST` → heuristic parser (3.2).
- `stat`: MLST → fallback: SIZE + MDTM pair → fallback: LIST of parent, find entry.
- `download`: passive `EPSV` (fallback `PASV`; active mode NOT in v1 — the design has
  no active-mode UI; document as v1.x advanced setting) → `REST offset` when resuming
  (only if `caps.rest_stream`) → `RETR`. Stream the data socket through the same
  chunk/progress/cancel loop as SFTP.
- `upload`: `REST`+`STOR` for resume when supported, else `APPE` fallback, else prompt
  degrades to Overwrite-only (conflict sheet hides Resume — `caps` flow into
  `Prompt::Conflict.options`).
- `mkdir/rename/remove`: MKD, RNFR/RNTO, DELE/RMD. `noop()`: NOOP (keepalive; FTP
  latency = round-trip of NOOP).
- Timestamp preservation on download from MDTM/MLSD `modify`; upload via MFMT when
  advertised.

## 3.2 The LIST parser (`crates/relay-core/src/listing/`)

Scope decision: support the formats that exist in 2026 — **Unix ls-style (incl. odd
date variants), DOS/IIS style, and MLSD** — skip VMS/MVS/OS-9/z/VM (FileZilla-era
mainframe formats; add only on real demand, keep the module pluggable).

Architecture (informed by FileZilla's `directorylistingparser.cpp` — behavior studied,
code NOT copied):

```rust
pub trait ListFormat: Send + Sync {
    fn score(&self, sample: &[&str]) -> u8;                 // 0..=100 confidence
    fn parse_line(&self, line: &str, ctx: &ParseCtx) -> Option<RemoteEntry>;
}
// Registry tries formats in score order on the first ~20 lines, locks the winner
// for the rest, tolerates ≤5% unparseable lines (logged), else falls back to
// name-only entries (never fail the whole listing — degraded > broken).
```

Unix format hard cases to handle (each is a test):
- 8- vs 9-column, `total NN` header lines, filenames with spaces / multiple spaces,
  symlinks `name -> target`, device major,minor in the size column, sticky/setuid
  perm chars (`rwsr-sr-t`), "year vs HH:MM" date column (recent = time, old = year —
  resolve against `now` with the 6-month window rule), non-English month names
  (at minimum: C locale + numeric; log unknown months), owner/group merged when
  numeric, hard-link count absent.
DOS/IIS: `MM-DD-YY HH:MMAM <DIR>|size name`, 2- vs 4-digit years.

Timezone handling (FileZilla's trick, simplified): when `caps.mfmt||mdtm` available,
compare MDTM (UTC) of one listed file against its LIST time → derive `tz_offset_min`,
cache in caps, apply to all LIST-derived times. Otherwise display as server-local with
a tooltip marker.

**Test corpus**: `crates/relay-core/tests/listings/*.txt` — golden files harvested from:
docker servers below, public test servers (test.rebex.net, ftp.gnu.org), FileZilla's
documented formats (transcribe sample lines from their tests, which are fine as data),
and any weird server we meet during beta. Every bug found later adds a corpus file.
Property test: parser never panics on arbitrary bytes (fuzz with `cargo-fuzz`, small
corpus in CI, longer runs ad hoc).

## 3.3 FTPS trust flow (rustls)

- Custom `ServerCertVerifier`: verify against `webpki-roots` first; on success →
  proceed silently. On failure (self-signed/expired/name-mismatch — rampant in FTPS
  land) → capture the chain, check `TrustStore` pin (`sha256` of leaf) → if unpinned,
  `interact.ask(TlsCert{..})` with the failure reason; Accept{remember} pins leaf
  fingerprint (per host:port). This mirrors the host-key flow so the UI sheet is shared.
- **TLS session resumption for data connections**: many servers (vsftpd, proftpd with
  `TLSOptions NoSessionReuseRequired` off) REQUIRE the data connection to resume the
  control connection's TLS session. rustls supports resumption; wire suppaftp's data
  socket TLS to share the control connection's `ClientSessionStore`. This is the #1
  FTPS interop trap — integration-test it explicitly against vsftpd default config.
- Data connection protection: `PBSZ 0` + `PROT P` always on FTPS.

## 3.4 FTP parallelism = connection pool

Unlike SFTP (channels over one connection), FTP transfer lanes are **full separate
control+data connections**:
- `Lane` for FTP = its own `FtpBackend` clone (own login). Per-server cap setting
  ("limit simultaneous connections", default 2, matches server `MaxClientsPerIP`
  realities); scheduler already respects per-session caps (P2) — just feed it the
  FTP-specific number.
- Lane connections skip capability probing (copy caps from the browse connection) and
  skip prompts (trust already established; auth reuses secrets — if the server rejects
  the extra login (530 "too many"), lane creation fails soft and the scheduler reduces
  the cap for this session with a logged notice).
- Idle lanes: send NOOP every 60s, close after 5 min idle.

## 3.5 Raw log & activity

FTP is line-oriented — perfect for the design's raw log toggle:
- `Log{kind: Command}` for every sent line (mask `PASS ****`), `kind: Response` for
  replies, `Status` for engine milestones, `Error` for failures — matches the
  FileZilla-style log the design shows under "Show raw log".
- SFTP equivalent (P1 retrofit): synthesize Status lines (opening channel, stat, read
  N bytes) at a debug verbosity setting; default shows Status/Error only.
- Ring buffer per session (10k lines) in Rust; frontend requests backlog on opening
  the Activity panel, then streams.

## 3.6 Integration test matrix (docker-compose in `crates/relay-core/tests/`)

| Server | Purpose |
|---|---|
| vsftpd (default config) | FTPS session-reuse requirement, classic Unix LIST |
| ProFTPD | MLSD path, MFMT, UTF8 |
| pure-ftpd | quirky LIST edge cases, virtual users |
| IIS FTP (Windows CI job or recorded transcripts) | DOS-style LIST, Windows paths |
| atmoz/sftp | (from P1) regression |

CI: Linux runner runs the docker matrix on every PR (`--features integration`);
recorded-transcript tests (raw server dialog replayed against the parser/state machine)
run everywhere including Windows, so protocol logic is tested even where docker isn't.

## 3.7 Exit criteria

1. Connect to vsftpd via FTPS: cert sheet on self-signed, PROT P, data-connection TLS
   resumption works; plain FTP shows the red "Unencrypted" badge and warning flow.
2. Same UX as SFTP everywhere: browse/transfer/queue/resume (REST) with zero
   frontend changes — only `caps`-driven option differences (e.g. Resume hidden when
   unsupported).
3. LIST corpus: 100% of golden files parse; fuzz run clean; unknown-format listing
   degrades to name-only with a warning toast, not a failure.
4. 4-lane parallel download from pure-ftpd saturates a local link; server capped at
   2 connections degrades gracefully.
5. Raw log pane shows the live FTP dialog with masked password.
