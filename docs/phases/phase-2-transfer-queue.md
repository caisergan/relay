# Phase 2 — Durable SFTP Transfer Queue

Objective: the queue is the heart of an FTP client and the thing users judge. Deliver a
scheduler with priorities, concurrency control, pause/retry/reorder, resume, recursive
folder transfers, persistence across restarts, and auto-reconnect that preserves the queue.

This is required for SFTP 1.0. Follow it with P5 hardening/beta/release before P3/P4.
Use the P0 interrupted-transfer harness and P1 owned temporary files as the starting
point; persistence is SQLite via `rusqlite`, with no JSON queue fallback.

## 2.1 Job model & state machine

```rust
// crates/relay-core/src/queue.rs
pub struct Job {
    pub id: JobId, pub session: SessionId, pub server_id: Uuid,
    pub direction: Direction,                  // Up | Down
    pub remote_path: String, pub local_path: PathBuf,
    pub size: Option<u64>, pub transferred: u64,
    pub state: JobState, pub order: i64,       // queue position (gap-based: 1024, 2048…)
    pub attempts: u32, pub error: Option<String>,
    pub parent: Option<JobId>,                 // folder jobs own child file jobs
    pub conflict_policy: Option<ConflictAction>, // "apply to remaining" propagates here
    pub resume: Option<ResumeRecord>,           // source facts, owned partial, verified checkpoint
}

pub enum JobState {
    Scanning,        // folder jobs: enumerating children
    Queued,
    Preparing,       // conflict check (stat destination), resume detection
    AwaitingPrompt,  // conflict sheet open for this job
    Transferring,
    Verifying,      // compare content/source facts before marking complete
    Paused,          // user pause OR session down (reason recorded)
    Failed,          // terminal until user retries (attempts < max auto-retries first)
    Done { at: DateTime<Utc> },
    Cancelled,
}
```

Legal transitions are enforced in one `fn advance(job, event)` (unit-tested exhaustively);
every transition updates authoritative Rust state and publishes `JobUpdate` through
the P0 coordinator/channel, with batched `QueueStats`. Zustand reflects these states;
optimistic UI reorder cannot create durable state transitions of its own.

## 2.2 Scheduler

One global `Scheduler` task (not per-session) owning the job table:

- Wakes on: job added/finished/failed, settings change (concurrency 1–8), pause/resume
  commands, session state changes.
- Pick rule: highest `order`-priority `Queued` job whose session has a free transfer
  slot. Per-session slot cap = min(global concurrency setting, per-server limit
  from P3 for FTP). Global cap = the settings slider (1–8).
- Dispatch: reserve capacity and run `Preparing` as bounded child work (stat,
  verification, conflict prompt), then hand the job to a transfer lane with a fresh
  `CancellationToken` and progress throttle. Return results as scheduler messages;
  never await network I/O, user input, or synchronous database calls inline in the
  scheduler. Prompt waits release transfer capacity; cap preparation workers/prompts
  separately so one unavailable server cannot stall other sessions.
- Reorder: frontend sends new neighbor ids → recompute `order` with gap strategy
  (renumber batch when gaps exhaust). Drag-to-reorder in the drawer = re-prioritize,
  per the design.
- Pause-all: cancel active preparation/transfer/verification work, await bounded
  cleanup, flush/acknowledge writes, and persist the verified checkpoint before
  publishing `Paused`. Preserve owned partials. Forced aborts leave an uncertain
  checkpoint that must be reconciled on recovery; displayed progress is not durable
  progress. Cancel is distinct: discard only the job's owned partials (failed remote
  cleanup is recorded for later), never unrelated destination files.
- Auto-retry: transient `EngineError::{Network, Timeout}` → backoff 2^attempts·1s,
  max 3 attempts, then `Failed` with reason (drawer Failed tab + designed failure line).
  `Auth`/`PermissionDenied`/`TrustRejected` are non-retryable → `Failed` immediately.
- Speed accounting: scheduler aggregates deltas from `JobUpdate`s into a 3s sliding
  window per direction → `QueueStats.speed_bps` (drawer header + per-row speed).

## 2.3 Session transfer lanes

Extend P1's ad-hoc second channel into a structured pool:
- `SessionActor` gains `transfer_lanes: Vec<Lane>` where a Lane = one SFTP channel
  (P3: a separate FTP control connection with per-operation data sockets) + one running
  child task. Lanes are created lazily up
  to the session cap and torn down after 60s idle.
- Browse channel is NEVER used for transfers — browsing stays snappy during a full queue.

## 2.4 Conflicts & resume

`Preparing` logic (per direction):
1. Look up owned-partial/resume records, then `stat` destination. Missing → no final
   destination conflict, but still verify any partial before resume. Exists → check `conflict_policy`
   (set by a previous "apply to remaining N"), else settings default
   (Ask/Overwrite/Skip), else prompt.
2. Prompt carries both sides' size+mtime (design shows side-by-side cards with "newer"
   highlight) + `remaining` count of pending conflicts in this batch.
3. Actions:
   - **Overwrite**: offset 0, truncate/create the owned temporary target; replace the
     final destination only after successful validation and finalization.
   - **Skip**: state `Done` (skipped flag) — drawer shows "skipped".
   - **KeepBoth**: rename target `name (2).ext` (increment until free — match design copy).
   - **Resume**: eligible only for a recorded Relay-owned partial that passes the
     verification below, in either direction. Never infer an upload offset from an
     arbitrary remote file's size, or a download's ownership from a suffix. A complete
     destination offers Overwrite/KeepBoth/Skip only. Determine partial eligibility
     even when the final destination is absent; conflicts still apply at finalization.
4. `apply_to_remaining` stores the action on the parent folder job (or a batch id for
   multi-select), so later eligible `Preparing` steps skip the prompt. It never bypasses
   per-file resume verification or authorizes overwriting an unrelated/new destination.

### Resume identity and verification

`ResumeRecord` contains server identity (including accepted host key), protocol,
direction, normalized source/destination paths, source size/mtime and file identity
where available, owned temporary path, acknowledged/durable offset, and a SHA-256
digest of the checkpointed prefix. Source facts are change detectors; size/mtime alone
do not prove content identity, including when a replacement preserves both.

- Before first transfer, capture source facts and exclusively create the temporary
  destination. Record ownership before streaming. Re-check source facts on completion.
- At pause/checkpoint, digest only the contiguous acknowledged prefix. For local
  downloads, flush/fsync before committing the matching offset/digest transaction.
  Remote write acknowledgement is not proof of durable storage: re-read/verify remote
  bytes on recovery, and use remote fsync only when the server supports it.
- Before resuming, match source facts and ownership, check lengths/bounds, and verify
  the source prefix and partial prefix against the stored digest. Use a supported
  server checksum operation or read the prefix to hash it. This adds I/O and may cost
  more than restarting; document the tradeoff and never substitute a size-only check.
- If partial length differs from the checkpoint after a crash, verify a common
  recorded prefix before truncating/reusing only that owned partial. Bytes beyond a
  durable checkpoint are untrusted until verified. No usable checkpoint means restart.
- Changed source facts, mismatching prefix, missing ownership record, or unavailable
  verification prevents automatic resume. Offer restart from zero, skip, or cancel;
  leave an existing completed destination intact. Report a clear reason.
- For resumed jobs, compare full source/destination hashes before finalization, using
  server checksums where supported and bounded streaming reads otherwise. Re-check
  source facts around verification; mutation or mismatch fails the job. Do not claim
  snapshot consistency for files being concurrently modified: require a stable source
  or restart after it stops changing. Only size equality is never sufficient.
- Finalization is idempotent: persist intent, validate, rename with the conflict
  policy, then persist `Done`. If a crash falls between rename and `Done`, verify the
  recorded final content before reconciling success; do not overwrite it blindly.

## 2.5 Recursive folder transfers

Folder job (`Scanning`) = FileZilla's recursive_operation reimagined:
- Downloads: BFS remote `list` (via browse channel, rate-limited to keep UI browsing
  responsive), create local dirs eagerly, enqueue child file jobs (batch-insert,
  `parent` set) as discovery proceeds — transfers START while scanning continues.
- Uploads: `walkdir` locally (fast), `mkdir -p` remote per directory level, then children.
- Parent job progress = Σ children (`transferred`/`size`); parent completes when all
  children terminal; parent cancel cascades to children.
- Cycle guard for symlinked dirs (visited set of canonical paths, depth cap 64).

## 2.6 Persistence (rusqlite)

```sql
CREATE TABLE jobs (
  id TEXT PRIMARY KEY, enqueue_request_id TEXT NOT NULL, request_item_id TEXT NOT NULL,
  server_id TEXT, direction INTEGER,
  remote_path TEXT, local_path TEXT, size INTEGER, transferred INTEGER,
  state INTEGER, ord INTEGER, attempts INTEGER, error TEXT,
  parent TEXT, conflict_policy INTEGER, created_at INTEGER,
  UNIQUE (enqueue_request_id, request_item_id)
) STRICT;

CREATE TABLE resume_records (
  job_id TEXT PRIMARY KEY REFERENCES jobs(id) ON DELETE CASCADE,
  source_facts_json TEXT NOT NULL, target_facts_json TEXT NOT NULL,
  temporary_path TEXT NOT NULL, checkpoint_offset INTEGER NOT NULL,
  prefix_sha256 TEXT NOT NULL, final_sha256 TEXT,
  finalization_state TEXT NOT NULL
) STRICT;

CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL) STRICT;
```

- Enable foreign keys. Resume facts encode the identity/paths listed above and contain
  no secrets. Persist job/resume state transactionally with explicit schema versions.
  Persist enqueue deduplication keys for each batch item/recursive child so retries
  of the same request return the existing job ids, including after process restart.
- Commit enqueue, ownership, checkpoints, and terminal transitions before acknowledging
  them as durable. Batch display-progress writes (e.g. every 5s); keep displayed byte
  count separate from the verified checkpoint. Exit flushing is best effort, not the
  crash-recovery mechanism. Add migrations and fixture upgrade tests from the start.
- Startup: `Transferring/Preparing/AwaitingPrompt/Verifying` → `Paused("app restarted")`; `Done`
  older than 7 days pruned. Drawer shows restored queue; "Resume all" banner.
- One SQLite db, `WAL` mode, durability settings appropriate to acknowledged state;
  a dedicated blocking database worker owns the connection (no pool needed). The
  scheduler submits ordered work and consumes completion messages without blocking.
  Disk-full/write failure must not be reported as a durable success.

## 2.7 Reconnect with queue preservation

- Session actor detects connection death (channel EOF / keepalive failure) →
  `SessionState::Reconnecting{attempt}`; its `Transferring` jobs checkpoint → `Paused("connection lost")`.
- Backoff 1s, 2s, 5s, 10s, 30s (cap), max 10 attempts, then `Disconnected` — the
  design's amber banner shows attempt count and a "Reconnect now" button
  (`session_reconnect` command resets the backoff).
- On reconnect: re-auth silently via stored secrets (prompts only if secrets are `Ask`),
  restore cwd, scheduler auto-resumes the session's `Paused("connection lost")` jobs
  only after source/partial verification. Changed host keys always require the trust
  flow. Unverifiable jobs stay paused with restart/skip/cancel choices; they do not
  loop through automatic retries. User-paused jobs remain paused after reconnect.

## 2.8 Frontend

- QueueDrawer goes fully live: Active/Failed/Completed tabs with counts, aggregate
  progress bar + total speed in the collapsed header, per-row progress/speed/ETA
  (ETA = remaining/speed_3s, display "—" under 5 KB/s), retry & remove buttons,
  drag-handle reorder (optimistic, reconciled by `JobUpdate`), Pause all,
  Clear completed.
- Flow gutter caps at 5 concurrent pills + "+N" count bubble (design).
- Conflict sheet: full 4-action version + apply-to-remaining checkbox.
- Toasts: transfer complete (with "Reveal" action — `tauri-plugin-opener`), transfer
  failed (with "Retry" action), reconnected.
- OS drag-in: Tauri `onDragDropEvent` → enqueue uploads to the active remote dir;
  drag-out to Finder/Explorer is deferred to v1.x (Tauri support is limited — document).
- Essential settings for 1.0: theme, density, concurrency, default conflict action,
  and download directory in typed `settings.json`. Basic status/error logs, focus
  behavior, and loading/error states are required; richer P4 workflows are deferred.
- Remount/reconnect uses P0 snapshot reconciliation to recover queue rows, aggregate
  progress, and pending prompts. A missed completion update must not leave a stale
  active row. Idempotent enqueue request ids prevent duplicate jobs after uncertain
  command delivery; reconcile optimistic reorder against Rust state.

## 2.9 Tests & exit criteria

Tests:
- Scheduler property tests: never exceeds caps; priority respected; pause-all reaches
  a safe paused state within a measured deadline, including stalled I/O; no starvation
  among a finite batch (fuzz 500 fake jobs on MockBackend with injected
  failures — deterministic seed).
- State machine table test: every (state, event) pair.
- Integration: kill the docker sftp container mid-20-file-transfer → reconnect →
  all files complete with correct hashes (sha256 compare); restart the app mid-queue →
  restored + resumable.
- Conflict matrix test: {policy default, apply-to-remaining, each action} × {up, down}.
- Integrity matrix, both directions: same-size/same-mtime source replacement, source
  mutation during transfer, corrupt partial, foreign `.relaypart`, missing record,
  partial longer/shorter than checkpoint, unavailable checksum/read permission, and
  crash between final rename and `Done`. Assert verified recovery or safe refusal,
  never silent success based on size; completed destination remains intact on failure.
- Persistence: forced process termination (no exit hook), SQLite write failure, and
  migration from fixture databases. Verify resumed hashes and preserved pause reasons.
- UI recovery: lose an update or remount while a conflict is open; snapshots restore
  state and one effective prompt reply. One session awaiting input cannot stall others.

Exit criteria:
1. Queue a 1,000-file folder download: browsing stays responsive, progress accurate,
   speed/ETA sane, reorder/pause/retry all work.
2. Pull the network cable (or docker kill) → amber banner → auto-reconnect → queue
   finishes; final hashes match.
3. Restart app mid-queue → queue restored, resume completes partial files correctly.
4. Concurrency slider 1–8 visibly changes parallelism live.
5. Changed-source and corrupt-partial cases refuse unsafe resume with actionable UI;
   same-size files cannot pass integrity checks by size alone.
6. Forced termination and missed IPC delivery recover without duplicate jobs, lost
   conflicts, or incorrect success. SQLite migrations and failure paths are tested.
