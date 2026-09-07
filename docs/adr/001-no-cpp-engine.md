# ADR 001 — Do not reuse the FileZilla C++ engine

**Status:** accepted (2026-09-07). Ratifies the analysis in `ROADMAP.md` §1.

## Decision

Relay implements its own engine in Rust. `reference/filezilla` stays untracked, read
only, and is never copied from — not verbatim, not translated, not adapted.

## Why

Four obstacles, any one of which would be sufficient:

- **Licensing.** The engine is GPL-2.0-or-later. Linking it in any form — statically,
  dynamically, or behind a C shim — makes Relay a derivative work and forces the whole
  application under the GPL.
- **wxWidgets is load-bearing.** `wxString`, `wxDateTime`, `wxEvtHandler`, `wxThreadEx`
  and friends run through every engine header. It does not compile without them.
- **Age.** The TLS layer is 2011-era GnuTLS with a hard-pinned priority string and a
  512-bit DH floor. Shipping it today would be a security decision, not a convenience.
- **Boundary shape.** `CFileZillaEngine::Command` / `GetNextNotification` is C++ virtual
  classes and wx events, not a C ABI. Every type needs hand marshalling.

## What we take instead

Behaviour, not code. Documented formats and observable behaviour are not copyrightable;
expression is. Specifically: the catalogue of real-world `LIST` formats, the
command-in / notification-out API shape, per-server capability and timezone detection,
and one engine instance per connection with the queue above it.

## Consequences

Every protocol edge case is ours to rediscover and test. The compensation is a
dependency graph that is all MIT/Apache, an engine that unit-tests without a GUI, and
a TLS stack from this decade.

`crates/relay-core` mirrors the command-in / event-out shape: see
[ADR 002](002-actor-per-session.md).
