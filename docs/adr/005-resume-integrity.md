# ADR 005 — What has to be true before Relay resumes a transfer

**Status:** accepted as a constraint (2026-09-07). The verification machinery is
phase 2; this ADR fixes the rule the machinery has to satisfy.

## The rule

Relay offers resume only when it can show that the partial file it holds is a prefix
of the source it is about to continue reading. A size match is not that showing.

The failure this prevents is quiet. A 400 MB download is interrupted at 250 MB; the
file on the server is replaced with a different build of the same size; resuming
appends the tail of the new file to the head of the old one. The result is the right
length, has a plausible modification time, and is corrupt. Nothing in the interface
would say so.

## What "verified" has to mean

A `ResumeRecord` carries, and phase 2 must re-check before any non-zero offset is used:

1. **Source facts** recorded when the partial began — size, modification time, and any
   server-provided identity — compared against the source *now*.
2. **Ownership of the partial.** The job created it and recorded that it did. A file
   with the right name is not evidence; a `.relaypart` suffix is a naming convention,
   not a claim about provenance.
3. **A verified content prefix.** A digest over the bytes already written, checked
   against the same range of the source.

Any of these being unavailable is not permission to proceed. It produces
`EngineError::ResumeUnverifiable`, and the user is offered restart, skip, or cancel —
never a silent resume. A source known to have moved produces `SourceChanged`, which is
a different message because it is a different situation.

## Why the error taxonomy carries this

`ResumeUnverifiable`, `SourceChanged` and `IntegrityMismatch` are separate variants in
`EngineError`, and none of them is retryable (`is_retryable()` returns false; all three
return true from `needs_user_decision()`). Automatic retry on an integrity failure would
turn a caught corruption into a repeated one.

## What is already enforced

- `TransferReq::offset` is documented as non-zero *only* after verification, and the
  in-memory backend refuses an offset the partial file cannot back up
  (`a_resume_offset_is_refused_when_the_partial_does_not_back_it_up`).
- A cancelled download removes only the partial file its own job created, and never
  leaves a destination behind (`cancellation_removes_the_partial_...`).
- Downloads land through an owned temporary file and a rename, so an interrupted
  transfer cannot leave a truncated file at the destination path.
- Byte counts cross the IPC boundary as `Bytes`, a newtype rather than a bare integer —
  a small thing, but offsets and sizes are exactly the values that must not be
  casually converted. See [ADR 003](003-typed-ipc-specta.md).

## Still to build (phase 2)

Durable `ResumeRecord` rows in SQLite, prefix digest computation and its cost
(re-reading 250 MB to verify it is not free — the budget for this is a phase 2
decision), reconciliation of a checkpoint against a partial after a hard kill, and the
conflict sheet's "resume" option appearing only when `Prompt::Conflict::resume_allowed`
is true. The phase 0 interrupted-transfer prototype in
[ADR 004](004-protocol-compatibility.md) is where the real-server evidence goes.
