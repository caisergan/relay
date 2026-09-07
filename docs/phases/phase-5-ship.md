# Phase 5 — Hardening & Ship (weeks 13–14)

Objective: signed, notarized, auto-updating installers for macOS and Windows, with the
performance and reliability bar of a tool people trust with their servers.

## 5.1 Performance pass (budget-driven, measure first)

Targets (measured, in release builds):
- 10k-entry directory: list render < 150 ms after data arrives; sort < 50 ms;
  scroll 60 fps. (Rust-side sort/filter if TS profiling says so; virtualization
  already in from P0.)
- Progress events ≤ 10 Hz per job, UI CPU < 10% during a 8-lane transfer.
- Cold start < 1.5 s to interactive shell; memory < 200 MB with 3 sessions + queue.
- Startup: lazy-load CodeMirror + language packs (editor tab opens < 300 ms warm).
Tooling: `tracing` spans + a hidden perf HUD (frame time, event rate); `cargo flamegraph`
on the engine under a synthetic 1k-job queue.

## 5.2 Reliability hardening

- Chaos week: scripted fault injection into the docker matrix — connection resets
  mid-chunk, stalled sockets (tc netem delay/loss), server restarts, disk-full on
  download (tmpfs quota), permission flips mid-recursive-transfer. No panics, no stuck
  jobs, every failure lands in a designed state.
- Panic policy: `panic = "abort"` is NOT acceptable in the shell — install a panic hook
  that logs + shows a crash dialog with log-file path; engine tasks are
  `catch_unwind`-wrapped at the actor boundary so one session can't take the app down.
- Secrets audit: grep-audit + test asserting `servers.json`/`relay.sqlite`/logs contain
  no password/passphrase strings after a scripted session (incl. masked PASS in logs).
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
- Code signing: **Azure Trusted Signing** (cheapest sane 2026 option for individual
  devs) via `azuresigntool` in CI; fallback: OV cert + HSM token requires local signing
  — decide based on budget; UNSIGNED IS NOT AN OPTION (SmartScreen would kill adoption).
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

- 2-week private beta (0.9.x) with 5–10 real users before 1.0: their weird servers feed
  the LIST corpus and interop fixes (this is how FileZilla got good — compressed).
- In-app "Report issue" (palette + menu): opens GitHub issue template with app version,
  OS, and (user-confirmed) tail of the raw log with hostnames scrubbed.
- Crash/log collection stays LOCAL (no telemetry in v1 — privacy as a feature vs
  FileZilla's bundleware reputation; state it in the README).

## 5.7 v1.0 exit criteria

1. Fresh macOS (both archs) and Windows machines: download → install → connect to
   SFTP + FTPS + FTP servers → transfer 1 GB folder both directions — zero warnings
   beyond first-run Gatekeeper/SmartScreen norms, zero manual steps.
2. Auto-update 0.9.x → 1.0.0 works on both OSes (staged test with the real manifest).
3. Chaos suite green; perf budgets met; secrets audit clean.
4. Beta feedback triaged: all P0/P1 bugs closed, LIST corpus expanded with every
   format met during beta.
5. Docs: README (screenshots, comparison table), quickstart, server-compat notes,
   `SECURITY.md` (trust model: host-key pinning, cert pinning, keychain, no telemetry).

## 5.8 Post-1.0 backlog (v1.x, ordered by expected demand)

1. chmod dialog + recursive chmod (perms currently display-only — top user ask bet).
2. Speed limits (token bucket in the scheduler; FileZilla's CRateLimiter pattern).
3. Remote file search (recursive, filter-engine based).
4. Synchronized browsing + directory comparison (the design gap most PMs will push on).
5. Filename filter sets; per-site charset override; active FTP mode; implicit FTPS.
6. Drag-out to Finder/Explorer; tabs detach to windows; Linux build (Tauri makes it
   cheap — mostly QA + packaging cost).
