# Phase 5 — SFTP 1.0 Hardening & Ship

Objective: signed, notarized, auto-updating installers for macOS and Windows, with the
performance and reliability bar of a tool people trust with their servers.

Execute this phase **after P2 and before P3/P4**. Phase numbering is retained for
stable links; the first release supports SFTP only. Run the applicable checks again
for later releases, adding FTP/FTPS and editor checks when those features land.
Allow a dedicated beta window and estimate hardening from P0/P1 evidence; there is
no fixed week-14 deadline. Start signing-account/setup work during foundation so
external provisioning does not become a last-minute release dependency.

## 5.0 Essential SFTP experience (required before beta)

- Saved-server CRUD/Test connection, dual panes, queue/conflict flows, and settings
  (theme, density, concurrency, default conflict action, download directory) complete.
- Basic copyable status/error log with redaction and bounded history. Report issue
  reachable from a menu/settings entry; the P4 activity panel/palette is not required.
- Complete empty/loading/permission-denied/connection-loss states and actionable
  source-changed/resume-unverifiable errors. No visible enabled FTP/editor/preview
  controls that lead to unimplemented flows.
- Keyboard navigation and shortcuts for shipped actions, focus traps in dialogs,
  visible focus rings, icon labels, reduced motion, and contrast in both themes.
  Verify title bar and OS drag-in on macOS and Windows; check the actual webviews.
- Exercise P0 snapshot recovery while transferring, after completion, and while
  answering prompts. Restarting/remounting the UI must not duplicate jobs or replies.

## 5.1 Performance pass (budget-driven, measure first)

Targets (measured, in release builds):
- 10k-entry directory: list render < 150 ms after data arrives; sort < 50 ms;
  scroll 60 fps. (Rust-side sort/filter if TS profiling says so; virtualization
  already in from P0.)
- Progress events ≤ 10 Hz per job, UI CPU < 10% during a 8-lane transfer.
- Cold start < 1.5 s to interactive shell; memory < 200 MB with 3 sessions + queue.
- SFTP 1.0 has no editor bundle. On the later P4 release, lazy-load CodeMirror and
  language packs (editor tab opens < 300 ms warm) and recheck startup/memory budgets.
Tooling: `tracing` spans + a hidden perf HUD (frame time, event rate); `cargo flamegraph`
on the engine under a synthetic 1k-job queue.

## 5.2 Reliability hardening

- Scripted fault injection into the SFTP fixture matrix — connection resets
  mid-chunk, stalled sockets (tc netem delay/loss), server restarts, disk-full on
  download (tmpfs quota), permission flips mid-recursive-transfer. No panics, no stuck
  jobs, every failure lands in a designed state.
- Force-kill the process without an exit hook. Verify durable queue/partial recovery,
  checkpoint reconciliation, idempotent finalization, and hashes in both directions.
  Include same-size/same-mtime source replacement, corrupt partials, unavailable
  verification, and SQLite write failures. A size match alone never passes recovery.
- Re-run the P0/P1 authentication matrix on both OSes, including Windows OpenSSH
  agent, encrypted keys, keyboard-interactive, and rejected/changed host keys.
- Panic policy: `panic = "abort"` is NOT acceptable in the shell — install a panic hook
  that logs + shows a crash dialog with log-file path; engine tasks are
  `catch_unwind`-wrapped at the actor boundary so one session can't take the app down.
- Secrets audit: grep-audit + test asserting `servers.json`/`relay.sqlite`/logs contain
  no password/passphrase strings after a scripted SFTP session. Extend with masked
  FTP PASS checks when P3 ships.
- Update safety: queue db schema version + migration table from day one (already in P2;
  verify upgrade path with a fixture db).

## 5.3 macOS distribution

- Universal binary: `cargo tauri build --target universal-apple-darwin`
  (aarch64 + x86_64).
- Signing: Developer ID Application cert; entitlements minimal (network client,
  user-selected files); hardened runtime ON.
- Notarization: `notarytool` via tauri's built-in notarize env vars
  (`APPLE_API_ISSUER/KEY_ID/KEY`), staple ticket. CI: certs in GitHub encrypted
  secrets, keychain setup action.
- DMG with background art (design's icon set); verify Gatekeeper on a clean VM.

## 5.4 Windows distribution

- Bundler: NSIS (per-user install, no admin) — MSI only if enterprise asks later.
- Code signing: select a provider after checking current eligibility, pricing, and
  CI integration during P0 (evaluate Microsoft's signing service and certificate
  providers). Record the decision and verify a signed artifact on a clean machine;
  the release gate requires signed installers, not a particular vendor.
- WebView2: bootstrapper embed mode (`downloadBootstrapper`) — default Win11 has it.
- Test on clean Win10 22H2 + Win11 VMs (SmartScreen behavior, HiDPI, credential manager).

## 5.5 Updater & release pipeline

- `tauri-plugin-updater`: static JSON manifest on GitHub Releases (`latest.json`),
  update keypair generated ONCE and stored offline + CI secret; in-app "check for
  updates" + settings toggle for auto-check (weekly), designed toast → restart flow.
- Release workflow (`release.yml`): tag push → matrix build (macos-14, windows-latest)
  → sign/notarize → draft GitHub Release with artifacts + `latest.json` → manual
  publish button. Changelog from conventional commits (`git-cliff`).
- Versioning: semver, `0.9.x` beta channel first (see 5.6), `1.0.0` when exit criteria met.

## 5.6 Beta & feedback loop

- Plan an initial 2-week private beta (0.9.x) with 5–10 real users before SFTP 1.0;
  extend if release blockers remain. Cover both OSes, authentication methods, long
  queues, unreliable networks, and real SSH servers. Turn failures into fixtures.
  Later P3 beta adds FTP/FTPS servers and expands the LIST corpus.
- In-app "Report issue" (menu/settings; palette added in P4): opens GitHub issue template with app version,
  OS, and (user-confirmed) tail of the raw log with hostnames scrubbed.
- Crash/log collection stays LOCAL (no telemetry in v1 — privacy as a feature vs
  FileZilla's bundleware reputation; state it in the README).

## 5.7 v1.0 exit criteria

1. Fresh macOS (both archs) and Windows machines: download → install → connect to
   SFTP server → transfer 1 GB folder both directions — zero warnings
   beyond first-run Gatekeeper/SmartScreen norms, zero manual steps.
2. Auto-update 0.9.x → 1.0.0 works on both OSes (staged test with the real manifest).
3. SFTP chaos/integrity suite green; perf budgets met; secrets audit clean; UI
   snapshot and prompt recovery pass on both OSes.
4. Beta feedback triaged: all release-blocking/critical/high-severity bugs closed,
   with regression fixtures for authentication, transfer, and recovery failures.
5. Docs: README (screenshots, comparison table), quickstart, server-compat notes,
   `SECURITY.md` (host-key pinning, keychain, resume verification, no telemetry).
   State explicitly that FTP/FTPS, editors, and previews are planned extensions.
6. P3/P4 completion is not required. Before later releases, extend these criteria to
   cover shipped protocols, certificate trust, LIST corpus, editor save conflicts,
   and the same install/update/recovery checks.

## 5.8 Post-1.0 sequence and later backlog

First: P3 FTP/FTPS expansion, then P4 editors/Quick Look/palette/richer management.
Each returns through the applicable release gates above. Subsequent candidates,
prioritized from actual beta/user demand:

1. chmod dialog + recursive chmod (perms currently display-only — top user ask bet).
2. Speed limits (token bucket in the scheduler; FileZilla's CRateLimiter pattern).
3. Remote file search (recursive, filter-engine based).
4. Synchronized browsing + directory comparison (the design gap most PMs will push on).
5. Filename filter sets; per-site charset override; active FTP mode; implicit FTPS.
6. Drag-out to Finder/Explorer; tabs detach to windows; Linux build (Tauri makes it
   cheap — mostly QA + packaging cost).
