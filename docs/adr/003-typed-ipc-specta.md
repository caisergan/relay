# ADR 003 — Generate IPC types from the engine, not from the shell

**Status:** accepted (2026-09-07). Revisit when `tauri-specta` 2.0 is stable.

## Decision

`src/ipc/gen.ts` is generated from `relay-core` by `specta` + `specta-typescript`
(`pnpm gen:ipc` → `cargo run -p relay-core --example gen-ipc`). The typed command
wrappers in `src/ipc/commands.ts` are hand-written and thin. `tauri-specta` is not used.

## Why not tauri-specta

Phase 0 §0.3 named `tauri-specta` and allowed an alternative if integration failed.
It did not fail so much as arrive in the wrong condition:

- `tauri-specta`'s latest **stable** release, 1.0.2, depends on `tauri ^1.2.4`. It is a
  Tauri v1 crate.
- Tauri v2 support exists only on the `2.0.0-rc` line (`2.0.0-rc.25` at the time of
  writing), which also pins `specta =2.0.0-rc.25`.

Taking it means a release-candidate dependency in the shell for the sake of generating
the command signatures — the smallest and most stable part of the surface.

## Why generating from the core is better regardless

- **It runs everywhere the engine builds.** A development machine that cannot compile
  the webview (a Linux workstation, a container) can still regenerate the bindings, and
  every CI leg can verify they are current. Generating from the shell would have tied
  regeneration to macOS and Windows.
- **The core stays Tauri-free.** `specta` alone carries no Tauri dependency, so the
  phase 0 exit criterion — `cargo tree -p relay-core` free of Tauri — survives contact
  with type generation. CI asserts it.
- **The hand-written part is small and reviewed.** About fifty lines of `invoke`
  wrappers. Types, which are where drift actually hurts, are generated.

`specta` itself is pinned at `=2.0.0-rc.25`, which is the same release-candidate line —
but as a dependency of a Tauri-free library, not of the shell.

## Staleness

`src/ipc/gen.ts` is committed so the frontend builds without a Rust toolchain. The
`bindings` CI job regenerates it and fails on any diff.

## A consequence worth knowing about

Specta refuses to export `u64`, `i64`, and `usize`, because JavaScript numbers lose
precision above 2^53. Rather than widen or cast at each site, byte counts and sequence
numbers are newtypes in `crates/relay-core/src/wire.rs` (`Bytes`, `Seq`, `Order`) that
serialise transparently and export as TypeScript `number`. See
[ADR 005](005-resume-integrity.md) for why byte counts get this much attention.

## Revisit when

`tauri-specta` 2.0 ships stable. The gain would be generated command signatures; the
cost would be moving generation back onto the platforms that can build a webview.
