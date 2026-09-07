# Phase 2 — Transfer Queue Done Right (weeks 5–6)

Objective: the queue is the heart of an FTP client and the thing users judge. Deliver a
scheduler with priorities, concurrency control, pause/retry/reorder, resume, recursive
folder transfers, persistence across restarts, and auto-reconnect that preserves the queue.

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
}

pub enum JobState {
    Scanning,        // folder jobs: enumerating children
    Queued,
    Preparing,       // conflict check (stat destination), resume detection
    AwaitingPrompt,  // conflict sheet open for this job
    Transferring,
    Paused,          // user pause OR session down (reason recorded)
    Failed,          // terminal until user retries (attempts < max auto-retries first)
    Done { at: DateTime<Utc> },
    Cancelled,
}
```

Legal transitions are enforced in one `fn advance(job, event)` (unit-tested exhaustively);
every transition emits `JobUpdate` and (batched, 250 ms) `QueueStats`.

## 2.2 Scheduler

One global `Scheduler` task (not per-session) owning the job table:

- Wakes on: job added/finished/failed, settings change (concurrency 1–8), pause/resume
  commands, session state changes.
- Pick rule: highest `order`-priority `Queued` job whose session has a free transfer
  slot. Per-session slot cap = min(global concurrency setting, per-server limit
  from P3 for FTP). Global cap = the settings slider (1–8).
- Dispatch: `Preparing` inline (stat via session actor), then hand the job to the
  session's **transfer lane** (see 2.3) with a fresh `CancellationToken` and a progress
  throttle. The scheduler never touches sockets — it only orchestrates.
- Reorder: frontend sends new neighbor ids → recompute `order` with gap strategy
  (renumber batch when gaps exhaust). Drag-to-reorder in the drawer = re-prioritize,
  per the design.
- Pause-all: cancel tokens of `Transferring` jobs (they checkpoint `transferred`
  first — the cancel path flushes and records bytes), set `Paused`.
- Auto-retry: transient `EngineError::{Network, Timeout}` → backoff 2^attempts·1s,
  max 3 attempts, then `Failed` with reason (drawer Failed tab + designed failure line).
  `Auth`/`PermissionDenied`/`TrustRejected` are non-retryable → `Failed` immediately.
- Speed accounting: scheduler aggregates deltas from `JobUpdate`s into a 3s sliding
  window per direction → `QueueStats.speed_bps` (drawer header + per-row speed).

## 2.3 Session transfer lanes

Extend P1's ad-hoc second channel into a structured pool:
- `SessionActor` gains `transfer_lanes: Vec<Lane>` where a Lane = one SFTP channel
  (P3: one FTP data connection) + one running child task. Lanes are created lazily up
  to the session cap and torn down after 60s idle.
- Browse channel is NEVER used for transfers — browsing stays snappy during a full queue.

## 2.4 Conflicts & resume

`Preparing` logic (per direction):
1. `stat` destination. Missing → go. Exists → check job/queue-level `conflict_policy`
   (set by a previous "apply to remaining N"), else settings default
   (Ask/Overwrite/Skip), else prompt.
2. Prompt carries both sides' size+mtime (design shows side-by-side cards with "newer"
   highlight) + `remaining` count of pending conflicts in this batch.
3. Actions:
   - **Overwrite**: offset 0, truncate.
   - **Skip**: state `Done` (skipped flag) — drawer shows "skipped".
   - **KeepBoth**: rename target `name (2).ext` (increment until free — match design copy).
   - **Resume**: downloads — offset = size of existing `.relaypart` (a completed
     plain file offers Overwrite/KeepBoth/Skip only; resume only applies to partials);
     uploads — offset = remote size (SFTP seek-write; FTP REST in P3). After resume
     completes, verify final size == expected; mismatch → `Failed("size mismatch")`.
4. `apply_to_remaining` stores the action on the parent folder job (or a batch id for
   multi-select), so later `Preparing` steps skip the prompt.

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
  id TEXT PRIMARY KEY, server_id TEXT, direction INTEGER,
  remote_path TEXT, local_path TEXT, size INTEGER, transferred INTEGER,
  state INTEGER, ord INTEGER, attempts INTEGER, error TEXT,
  parent TEXT, conflict_policy INTEGER, created_at INTEGER
) STRICT;
```

- Write-behind: dirty jobs flushed every 1s + on state transitions + on exit; progress
  bytes only every 5s (avoid write amplification).
- Startup: `Transferring/Preparing/AwaitingPrompt` → `Paused("app restarted")`; `Done`
  older than 7 days pruned. Drawer shows restored queue; "Resume all" banner.
- One SQLite db, `WAL` mode; all access from the scheduler task (no pool needed).

## 2.7 Reconnect with queue preservation

- Session actor detects connection death (channel EOF / keepalive failure) →
  `SessionState::Reconnecting{attempt}`; its `Transferring` jobs checkpoint → `Paused("connection lost")`.
- Backoff 1s, 2s, 5s, 10s, 30s (cap), max 10 attempts, then `Disconnected` — the
  design's amber banner shows attempt count and a "Reconnect now" button
  (`session_reconnect` command resets the backoff).
- On reconnect: re-auth silently via stored secrets (prompts only if secrets are `Ask`),
  restore cwd, scheduler auto-resumes the session's `Paused("connection lost")` jobs
  (resume offsets — this is where `.relaypart` + checkpointed `transferred` pay off).

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

## 2.9 Tests & exit criteria

Tests:
- Scheduler property tests: never exceeds caps; priority respected; pause-all halts
  within one chunk; no starvation (fuzz 500 fake jobs on MockBackend with injected
  failures — deterministic seed).
- State machine table test: every (state, event) pair.
- Integration: kill the docker sftp container mid-20-file-transfer → reconnect →
  all files complete with correct hashes (sha256 compare); restart the app mid-queue →
  restored + resumable.
- Conflict matrix test: {policy default, apply-to-remaining, each action} × {up, down}.

Exit criteria:
1. Queue a 1,000-file folder download: browsing stays responsive, progress accurate,
   speed/ETA sane, reorder/pause/retry all work.
2. Pull the network cable (or docker kill) → amber banner → auto-reconnect → queue
   finishes; final hashes match.
3. Restart app mid-queue → queue restored, resume completes partial files correctly.
4. Concurrency slider 1–8 visibly changes parallelism live.
