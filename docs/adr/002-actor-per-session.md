# ADR 002 — One owner per connection, and transfers on their own lanes

**Status:** accepted (2026-09-07), with the actor itself landing in phase 1.

## Decision

A session's connection is owned exclusively by one task. Browse operations are
serialised through it. Transfers do **not** run inside it: `Protocol::open_lane`
hands out a `TransferLane`, an independently owned handle that moves into its own task.

## The problem with the original sketch

Phase 0 §0.2 sketched `download`/`upload` on `Protocol` behind `&mut self`, and asked
us to prove that concurrent transfers are not forced through that single borrow. They
cannot be. A `&mut Protocol` held for the length of a transfer is a lock on the whole
session: browsing stops until the bytes finish, which is precisely what phase 1 §1.4
forbids. Two transfers cannot run at all.

Lifetimes made the same point from the other side. A `TransferReq<'a>` holding
`&'a dyn Fn` cannot move into a spawned task without borrowing something that outlives
it, so the sketch's progress callback fights every attempt to run a transfer
concurrently. `TransferReq` is therefore owned, with an `Arc`-backed `ProgressSink`.

## Shape

```
SessionActor  ── owns ──▶ Box<dyn Protocol>     list / stat / mkdir / rename / …
     │
     └── open_lane() ──▶ Box<dyn TransferLane>  moved into a transfer task
```

For SFTP a lane is a second `sftp` subsystem channel on the same SSH connection —
`russh`'s `Handle` takes `&self`, so an `Arc` is enough. For FTP (phase 3) a lane is a
whole additional control connection, which is why `BackendCapabilities::max_lanes`
exists rather than assuming SFTP's multiplexing everywhere.

## Evidence

`crates/relay-core/tests/mock_backend.rs::browsing_continues_while_two_lanes_transfer`
holds two lanes, runs both transfers, and lists five times while they are in flight.
`cancellation_removes_the_partial_and_never_creates_the_destination` covers the other
half: a cancelled lane cleans up only the partial file it owns, and the destination is
never created.

## Consequences

Backends implement two traits rather than one. `close(self: Box<Self>)` exists so a
lane whose protocol state is uncertain after cancellation is discarded rather than
reused.

## Still open

The actor's `select!` loop, keepalive, deadlines, and reconnect are phase 1. Whether
sequential chunk I/O saturates a high-latency link, or bounded request pipelining is
needed, is a question for the prototypes in [ADR 004](004-protocol-compatibility.md).
