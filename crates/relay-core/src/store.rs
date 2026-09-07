//! The queue on disk.
//!
//! A queue that forgets everything when the app closes is not a queue; it is a list of
//! things that were happening. Relay's has to survive a quit, a crash, and a power cut,
//! and come back knowing not just what was queued but what was *proven* — which bytes
//! reached the disk, which rename was attempted, which partial file is ours.
//!
//! **One thread owns the connection.** SQLite calls block, and a blocking call inside
//! the scheduler's loop would stall every session at once. So the connection lives on a
//! dedicated worker thread and everything reaches it as a closure over a channel; the
//! async side awaits a oneshot and never touches the database itself. Work is applied
//! in the order it was submitted, which is what makes "the checkpoint was committed
//! before the transfer continued" a fact rather than a hope.
//!
//! **What is stored, and how.** Anything the queue queries lives in its own column.
//! `JobState` lives in one JSON column instead of an integer plus a scatter of nullable
//! fields, because the state machine's shape belongs to [`crate::queue`] and a second
//! encoding of it in SQL would drift the first time a variant gains a field. The
//! resume record is the exception: its safety-critical scalars — the checkpoint, the
//! digests, how far finalisation got — are real columns, so they are inspectable in a
//! `sqlite3` shell when someone has to work out what happened to a file.
//!
//! **Acknowledged means committed.** Every write returns a `Result` and the caller may
//! not report success until it comes back `Ok`. A full disk is a failure, and a queue
//! that says "saved" when the write failed is worse than one that says nothing.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use chrono::{DateTime, TimeDelta, Utc};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::error::{EngineError, Result};
use crate::job::{JobState, PauseReason};
use crate::model::{FileFacts, JobId};
use crate::queue::{Finalization, Job, ResumeRecord};
use crate::wire::Bytes;

/// How long a finished job stays in the drawer's Completed tab before it is pruned.
pub const KEEP_COMPLETED: TimeDelta = TimeDelta::days(7);

/// The schema this build writes. A database from a *newer* version is refused rather
/// than opened: a downgraded Relay reading a queue it does not understand would drop
/// the fields it cannot see on the next write, which is data loss disguised as a
/// successful start.
pub const SCHEMA_VERSION: i64 = 1;

/// Applied in order, each inside its own transaction, each recorded on success.
const MIGRATIONS: &[(i64, &str)] = &[(
    1,
    r#"
    CREATE TABLE jobs (
      id              TEXT PRIMARY KEY,
      -- Which gesture created this job, and which item of it. Together they are
      -- unique, so an enqueue re-sent after an uncertain IPC delivery finds the job
      -- it already made instead of queueing the same file twice.
      batch           TEXT NOT NULL,
      item            TEXT NOT NULL,
      session         TEXT NOT NULL,
      server_id       TEXT NOT NULL,
      kind            TEXT NOT NULL,
      direction       TEXT NOT NULL,
      remote_path     TEXT NOT NULL,
      local_path      TEXT NOT NULL,
      size            INTEGER,
      -- Displayed progress. Written in batches and never trusted by a resume; the
      -- durable number is resume_records.checkpoint_offset.
      transferred     INTEGER NOT NULL,
      state           TEXT NOT NULL,
      ord             INTEGER NOT NULL,
      attempts        INTEGER NOT NULL,
      retry_at        TEXT,
      conflict_policy TEXT,
      chosen          TEXT,
      parent          TEXT,
      created_at      TEXT NOT NULL,
      started_at      TEXT,
      UNIQUE (batch, item)
    ) STRICT;

    CREATE INDEX jobs_by_order ON jobs (ord);
    CREATE INDEX jobs_by_parent ON jobs (parent);

    CREATE TABLE resume_records (
      job_id            TEXT PRIMARY KEY REFERENCES jobs(id) ON DELETE CASCADE,
      source_facts_json TEXT NOT NULL,
      temporary_path    TEXT NOT NULL,
      checkpoint_offset INTEGER NOT NULL,
      prefix_sha256     TEXT NOT NULL,
      final_sha256      TEXT,
      finalization_state TEXT NOT NULL
    ) STRICT;
    "#,
)];

/// A handle to the queue's database. Cheap to clone; every clone talks to the same
/// worker thread and therefore to the same ordered stream of work.
#[derive(Clone)]
pub struct QueueStore {
    tx: mpsc::Sender<Task>,
}

impl std::fmt::Debug for QueueStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("QueueStore")
    }
}

type Task = Box<dyn FnOnce(&mut Connection) + Send>;

impl QueueStore {
    /// Open (or create) the queue database and bring its schema up to date.
    ///
    /// Returns once migrations have run, so a caller that gets a `QueueStore` back has
    /// one it can write to.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let (tx, rx) = mpsc::channel::<Task>();
        let (ready_tx, ready_rx) = oneshot::channel();

        thread::Builder::new()
            .name("relay-queue-db".into())
            .spawn(move || worker(path, rx, ready_tx))
            .map_err(|err| EngineError::LocalIo {
                path: "queue database".into(),
                message: format!("could not start the database worker: {err}"),
            })?;

        // The worker reports the outcome of opening and migrating before it takes any
        // work, so a broken database is an error from `open` rather than a store that
        // fails every call later on.
        match ready_rx.await {
            Ok(Ok(())) => Ok(Self { tx }),
            Ok(Err(err)) => Err(err),
            Err(_) => Err(gone()),
        }
    }

    /// An in-memory database, for tests and for a build that has nowhere to write.
    pub async fn in_memory() -> Result<Self> {
        Self::open(":memory:").await
    }

    async fn call<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(Box::new(move |conn| {
                let _ = reply_tx.send(work(conn));
            }))
            .map_err(|_| gone())?;
        reply_rx.await.map_err(|_| gone())?
    }

    /// Insert a batch of jobs, returning what the queue now holds for it.
    ///
    /// Idempotent on `(batch, item)`: if a request arrives twice — a retried command,
    /// an uncertain IPC delivery, a reconnect that replays the gesture — the second
    /// insert finds the first one's rows and returns those. The alternative is a user
    /// who clicks once and watches the same folder download twice.
    ///
    /// One transaction, so a batch is entirely queued or not queued at all.
    pub async fn insert_batch(&self, jobs: Vec<Job>) -> Result<Vec<Job>> {
        self.call(move |conn| {
            let tx = conn.transaction().map_err(db)?;
            let mut out = Vec::with_capacity(jobs.len());
            for job in jobs {
                let existing: Option<String> = tx
                    .query_row(
                        "SELECT id FROM jobs WHERE batch = ?1 AND item = ?2",
                        params![job.batch.to_string(), job.item],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(db)?;
                match existing {
                    Some(id) => {
                        let id: JobId = id.parse().map_err(|_| corrupt("job id"))?;
                        out.push(read_job(&tx, id)?.ok_or_else(|| corrupt("job row"))?);
                    }
                    None => {
                        insert_job(&tx, &job)?;
                        out.push(job);
                    }
                }
            }
            tx.commit().map_err(db)?;
            Ok(out)
        })
        .await
    }

    /// Persist a job without waiting for the write.
    ///
    /// For transitions whose exact value recovery does not depend on — `Preparing`,
    /// `Transferring` — because `recover` maps every one of them to
    /// `Paused(Restarted)` regardless. Awaiting an fsync for a state that will be
    /// discarded on the way back in would put the disk in the scheduler's loop for
    /// nothing. Enqueue, terminal states and checkpoints all use [`Self::save`]
    /// instead, and wait.
    ///
    /// Still ordered behind everything already submitted; only the acknowledgement is
    /// dropped. A failure is logged rather than returned, since there is no caller
    /// left to tell.
    pub fn save_detached(&self, job: Job) {
        let id = job.id;
        let _ = self.tx.send(Box::new(move |conn| {
            let outcome = conn
                .transaction()
                .map_err(db)
                .and_then(|tx| write_job(&tx, &job).and_then(|()| tx.commit().map_err(db)));
            if let Err(err) = outcome {
                tracing::warn!(%id, %err, "could not persist a job's progress state");
            }
        }));
    }

    /// Persist a job's current state, and its resume record if it has one.
    ///
    /// The two are written in one transaction. A checkpoint that survives without the
    /// state that explains it, or a state that claims a resume the record does not
    /// support, is exactly the inconsistency recovery cannot untangle.
    pub async fn save(&self, job: Job) -> Result<()> {
        self.call(move |conn| {
            let tx = conn.transaction().map_err(db)?;
            write_job(&tx, &job)?;
            tx.commit().map_err(db)
        })
        .await
    }

    /// Record displayed progress only.
    ///
    /// Deliberately cheap and deliberately not durable-making: this moves the number
    /// the progress bar reads, nothing else. The caller batches these (the scheduler
    /// does it every few seconds) because a write per progress tick would put the
    /// database in the transfer's hot path for a number that does not have to be
    /// exact. A resume never reads this column.
    pub async fn save_progress(&self, updates: Vec<(JobId, Bytes)>) -> Result<()> {
        self.call(move |conn| {
            let tx = conn.transaction().map_err(db)?;
            for (id, transferred) in updates {
                tx.execute(
                    "UPDATE jobs SET transferred = ?2 WHERE id = ?1",
                    params![id.to_string(), transferred.get() as i64],
                )
                .map_err(db)?;
            }
            tx.commit().map_err(db)
        })
        .await
    }

    /// Commit a verified checkpoint before the transfer is allowed to rely on it.
    ///
    /// This is the write that must be durable: everything a resume trusts comes from
    /// here, so a caller advances its in-memory checkpoint only after this returns
    /// `Ok`. Reversing that order is how a crash produces a partial file whose recorded
    /// digest describes bytes that are not in it.
    pub async fn checkpoint(&self, job: JobId, record: ResumeRecord) -> Result<()> {
        self.call(move |conn| {
            let tx = conn.transaction().map_err(db)?;
            write_resume(&tx, job, &record)?;
            tx.commit().map_err(db)
        })
        .await
    }

    /// Everything in the queue, in queue order.
    pub async fn load_all(&self) -> Result<Vec<Job>> {
        self.call(|conn| read_all(conn)).await
    }

    /// The startup pass: reconcile what was running, prune what is stale, hand back
    /// the queue.
    ///
    /// Anything the process was in the middle of becomes `Paused(Restarted)`. It is
    /// not requeued, because nothing here knows whether the bytes in flight reached
    /// the disk — that is a question for the resume verification, and this pass exists
    /// to make sure it gets asked rather than skipped. Finished jobs older than
    /// [`KEEP_COMPLETED`] are dropped so the Completed tab is a recent history rather
    /// than a permanent one.
    ///
    /// The prune reads `$.at`, which is why `Done` and `Cancelled` both carry one. A
    /// bare unit variant would compare as NULL, match nothing, and quietly keep every
    /// cancelled job for the life of the installation.
    pub async fn recover(&self) -> Result<Vec<Job>> {
        self.call(|conn| {
            let tx = conn.transaction().map_err(db)?;
            let cutoff = (Utc::now() - KEEP_COMPLETED).to_rfc3339();
            tx.execute(
                "DELETE FROM jobs
                 WHERE json_extract(state, '$.kind') IN ('done', 'cancelled')
                   AND json_extract(state, '$.at') < ?1",
                params![cutoff],
            )
            .map_err(db)?;

            let interrupted = select(
                &tx,
                "json_extract(j.state, '$.kind')
                 IN ('preparing', 'awaitingPrompt', 'transferring', 'verifying', 'scanning')",
                [],
            )?;
            for mut job in interrupted {
                job.state = JobState::Paused {
                    reason: PauseReason::Restarted,
                };
                job.speed_bps = None;
                write_job(&tx, &job)?;
            }
            tx.commit().map_err(db)?;
            read_all(conn)
        })
        .await
    }

    /// Drop finished jobs — the drawer's "Clear completed".
    pub async fn clear_completed(&self) -> Result<()> {
        self.call(|conn| {
            conn.execute(
                "DELETE FROM jobs WHERE json_extract(state, '$.kind') IN ('done', 'cancelled')",
                [],
            )
            .map(|_| ())
            .map_err(db)
        })
        .await
    }

    /// Forget one job and its resume record.
    pub async fn remove(&self, job: JobId) -> Result<()> {
        self.call(move |conn| {
            conn.execute("DELETE FROM jobs WHERE id = ?1", params![job.to_string()])
                .map(|_| ())
                .map_err(db)
        })
        .await
    }
}

/// The worker thread: opens the database, migrates it, then applies work in order.
fn worker(path: PathBuf, rx: mpsc::Receiver<Task>, ready: oneshot::Sender<Result<()>>) {
    let mut conn = match prepare(&path) {
        Ok(conn) => conn,
        Err(err) => {
            let _ = ready.send(Err(err));
            return;
        }
    };
    if ready.send(Ok(())).is_err() {
        return;
    }
    // Ends when every `QueueStore` clone has been dropped.
    while let Ok(task) = rx.recv() {
        task(&mut conn);
    }
}

fn prepare(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path).map_err(db)?;
    // Foreign keys are off by default in SQLite, which would leave the resume record's
    // cascade silently inert and orphan a checkpoint behind a deleted job.
    conn.pragma_update(None, "foreign_keys", "ON").map_err(db)?;
    // WAL lets the reader that draws the drawer run while a checkpoint is committing.
    // An in-memory database has no journal to speak of and refuses this; that is fine.
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    // The default (NORMAL under WAL) can lose the last commits to a power cut. The
    // whole point of the checkpoint column is that it is true after one.
    conn.pragma_update(None, "synchronous", "FULL")
        .map_err(db)?;
    migrate(&conn)?;
    Ok(conn)
}

/// Bring the schema to [`SCHEMA_VERSION`], or explain why it cannot.
fn migrate(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
           version INTEGER PRIMARY KEY,
           applied_at TEXT NOT NULL
         ) STRICT",
        [],
    )
    .map_err(db)?;

    let current: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(db)?;

    if current > SCHEMA_VERSION {
        return Err(EngineError::Unsupported {
            operation: format!(
                "the transfer queue was written by a newer version of Relay \
                 (schema {current}, this build understands {SCHEMA_VERSION})"
            ),
        });
    }

    for (version, sql) in MIGRATIONS.iter().filter(|(v, _)| *v > current) {
        conn.execute_batch(&format!("BEGIN; {sql} COMMIT;"))
            .map_err(db)?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            params![version, Utc::now().to_rfc3339()],
        )
        .map_err(db)?;
    }
    Ok(())
}

// ------------------------------------------------------------------ row mapping

/// The job columns as `j.<name>`, in the order [`map_job`] reads them. The two must
/// agree; they are next to each other so that stays visible.
const JOB_COLUMNS_PREFIXED: &str = "id, j.batch, j.item, j.session, j.server_id, j.kind,
    j.direction, j.remote_path, j.local_path, j.size, j.transferred, j.state, j.ord,
    j.attempts, j.retry_at, j.conflict_policy, j.chosen, j.parent, j.created_at,
    j.started_at";

const INSERT_JOB: &str = "INSERT INTO jobs (
    id, batch, item, session, server_id, kind, direction, remote_path, local_path,
    size, transferred, state, ord, attempts, retry_at, conflict_policy, chosen,
    parent, created_at, started_at
  ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
            ?17, ?18, ?19, ?20)";

/// The columns a job may change after it is queued.
///
/// Its identity, its paths and its place in a batch are deliberately absent: those are
/// what the row *is*, and an update that could rewrite them would let a save aimed at
/// one job land on another.
const UPSERT_JOB: &str = " ON CONFLICT(id) DO UPDATE SET
    size = excluded.size,
    transferred = excluded.transferred,
    state = excluded.state,
    ord = excluded.ord,
    attempts = excluded.attempts,
    retry_at = excluded.retry_at,
    conflict_policy = excluded.conflict_policy,
    chosen = excluded.chosen,
    started_at = excluded.started_at";

/// Add a job that does not exist yet.
///
/// A plain insert, deliberately: an id already in the table is an error here rather
/// than an update. Only [`write_job`] may overwrite a row, and only for a caller
/// holding the job it is overwriting.
fn insert_job(conn: &Connection, job: &Job) -> Result<()> {
    conn.execute(INSERT_JOB, params_from_iter(job_params(job)?))
        .map_err(db)?;
    write_resume_for(conn, job)
}

fn write_job(conn: &Connection, job: &Job) -> Result<()> {
    conn.execute(
        &format!("{INSERT_JOB}{UPSERT_JOB}"),
        params_from_iter(job_params(job)?),
    )
    .map_err(db)?;
    write_resume_for(conn, job)
}

fn write_resume_for(conn: &Connection, job: &Job) -> Result<()> {
    match &job.resume {
        Some(record) => write_resume(conn, job.id, record),
        // Only when the job genuinely has none. A job that has written no bytes owns no
        // partial, and a record left behind would offer a resume against a file this
        // job did not create.
        None => conn
            .execute(
                "DELETE FROM resume_records WHERE job_id = ?1",
                params![job.id.to_string()],
            )
            .map(|_| ())
            .map_err(db),
    }
}

/// Bound values rather than a `params!` slice: `params!` borrows temporaries, so it
/// cannot cross a function boundary, and both write paths need the same twenty values.
fn job_params(job: &Job) -> Result<Vec<Value>> {
    Ok(vec![
        text(job.id),
        text(job.batch),
        Value::Text(job.item.clone()),
        text(job.session),
        text(job.server_id),
        Value::Text(tag(&job.kind)?),
        Value::Text(tag(&job.direction)?),
        Value::Text(job.remote_path.clone()),
        Value::Text(job.local_path.to_string_lossy().into_owned()),
        job.size
            .map_or(Value::Null, |s| Value::Integer(s.get() as i64)),
        Value::Integer(job.transferred.get() as i64),
        Value::Text(as_json(&job.state)?),
        Value::Integer(job.order.get()),
        Value::Integer(i64::from(job.attempts)),
        job.retry_at.map_or(Value::Null, when),
        job.conflict_policy
            .map(|p| tag(&p))
            .transpose()?
            .map_or(Value::Null, Value::Text),
        job.chosen
            .map(|c| tag(&c))
            .transpose()?
            .map_or(Value::Null, Value::Text),
        job.parent.map_or(Value::Null, text),
        when(job.created_at),
        job.started_at.map_or(Value::Null, when),
    ])
}

fn text(id: Uuid) -> Value {
    Value::Text(id.to_string())
}

fn when(at: DateTime<Utc>) -> Value {
    Value::Text(at.to_rfc3339())
}

fn write_resume(conn: &Connection, job: JobId, record: &ResumeRecord) -> Result<()> {
    conn.execute(
        "INSERT INTO resume_records (
           job_id, source_facts_json, temporary_path, checkpoint_offset,
           prefix_sha256, final_sha256, finalization_state
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(job_id) DO UPDATE SET
           source_facts_json = excluded.source_facts_json,
           temporary_path = excluded.temporary_path,
           checkpoint_offset = excluded.checkpoint_offset,
           prefix_sha256 = excluded.prefix_sha256,
           final_sha256 = excluded.final_sha256,
           finalization_state = excluded.finalization_state",
        params![
            job.to_string(),
            as_json(&record.source)?,
            record.temporary_path,
            record.checkpoint.get() as i64,
            record.prefix_sha256,
            record.final_sha256,
            tag(&record.finalization)?,
        ],
    )
    .map(|_| ())
    .map_err(db)
}

/// A job with its resume record joined on, always in queue order.
///
/// `predicate` is only ever one of the literals in this module — never a value, and
/// never anything that reached the engine from outside it. Values are bound.
fn select(conn: &Connection, predicate: &str, args: impl rusqlite::Params) -> Result<Vec<Job>> {
    let sql = format!(
        "SELECT j.{JOB_COLUMNS_PREFIXED},
                r.source_facts_json, r.temporary_path, r.checkpoint_offset,
                r.prefix_sha256, r.final_sha256, r.finalization_state
         FROM jobs j LEFT JOIN resume_records r ON r.job_id = j.id
         WHERE {predicate} ORDER BY j.ord"
    );
    let mut stmt = conn.prepare(&sql).map_err(db)?;
    let rows = stmt
        .query_map(args, |row| Ok(map_job(row)))
        .map_err(db)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(db)?;
    rows.into_iter().collect()
}

fn read_all(conn: &Connection) -> Result<Vec<Job>> {
    select(conn, "1 = 1", [])
}

fn read_job(conn: &Connection, id: JobId) -> Result<Option<Job>> {
    Ok(select(conn, "j.id = ?1", params![id.to_string()])?.pop())
}

fn map_job(row: &rusqlite::Row<'_>) -> Result<Job> {
    let resume = match row.get::<_, Option<String>>(20).map_err(db)? {
        Some(source) => Some(ResumeRecord {
            source: from_json::<FileFacts>(&source)?,
            temporary_path: row.get(21).map_err(db)?,
            checkpoint: Bytes(row.get::<_, i64>(22).map_err(db)? as u64),
            prefix_sha256: row.get(23).map_err(db)?,
            final_sha256: row.get(24).map_err(db)?,
            finalization: untag::<Finalization>(&row.get::<_, String>(25).map_err(db)?)?,
        }),
        None => None,
    };

    Ok(Job {
        id: uuid(row, 0)?,
        batch: uuid(row, 1)?,
        item: row.get(2).map_err(db)?,
        session: uuid(row, 3)?,
        server_id: uuid(row, 4)?,
        kind: untag(&row.get::<_, String>(5).map_err(db)?)?,
        direction: untag(&row.get::<_, String>(6).map_err(db)?)?,
        remote_path: row.get(7).map_err(db)?,
        local_path: PathBuf::from(row.get::<_, String>(8).map_err(db)?),
        size: row
            .get::<_, Option<i64>>(9)
            .map_err(db)?
            .map(|s| Bytes(s as u64)),
        transferred: Bytes(row.get::<_, i64>(10).map_err(db)? as u64),
        state: from_json(&row.get::<_, String>(11).map_err(db)?)?,
        order: crate::wire::Order(row.get(12).map_err(db)?),
        attempts: row.get(13).map_err(db)?,
        retry_at: timestamp(row, 14)?,
        conflict_policy: row
            .get::<_, Option<String>>(15)
            .map_err(db)?
            .map(|p| untag(&p))
            .transpose()?,
        chosen: row
            .get::<_, Option<String>>(16)
            .map_err(db)?
            .map(|c| untag(&c))
            .transpose()?,
        parent: row
            .get::<_, Option<String>>(17)
            .map_err(db)?
            .map(|p| p.parse().map_err(|_| corrupt("parent id")))
            .transpose()?,
        created_at: timestamp(row, 18)?.ok_or_else(|| corrupt("created_at"))?,
        started_at: timestamp(row, 19)?,
        // Recent throughput is a live measurement; a stored one would be a lie about
        // a transfer that is not running.
        speed_bps: None,
        resume,
    })
}

fn uuid(row: &rusqlite::Row<'_>, index: usize) -> Result<Uuid> {
    row.get::<_, String>(index)
        .map_err(db)?
        .parse()
        .map_err(|_| corrupt("uuid"))
}

fn timestamp(row: &rusqlite::Row<'_>, index: usize) -> Result<Option<DateTime<Utc>>> {
    match row.get::<_, Option<String>>(index).map_err(db)? {
        Some(text) => Ok(Some(
            DateTime::parse_from_rfc3339(&text)
                .map_err(|_| corrupt("timestamp"))?
                .with_timezone(&Utc),
        )),
        None => Ok(None),
    }
}

/// A unit-variant enum as bare text.
///
/// `serde_json` would write `"down"` with the quotes, which is correct JSON and makes
/// every one of these columns read as a quoted string in a `sqlite3` shell. Going
/// through serde rather than a hand-written match keeps the spelling the same one the
/// IPC layer uses, so a column and a wire value can never disagree.
fn tag<T: serde::Serialize>(value: &T) -> Result<String> {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => Ok(text),
        _ => Err(EngineError::LocalIo {
            path: "queue database".into(),
            message: "a queue enum that should be a plain tag is not one".into(),
        }),
    }
}

fn untag<T: serde::de::DeserializeOwned>(text: &str) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(text.to_string())).map_err(|err| {
        EngineError::LocalIo {
            path: "queue database".into(),
            message: format!("could not decode a queue tag: {err}"),
        }
    })
}

fn as_json<T: serde::Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|err| EngineError::LocalIo {
        path: "queue database".into(),
        message: format!("could not encode a queue value: {err}"),
    })
}

fn from_json<T: serde::de::DeserializeOwned>(text: &str) -> Result<T> {
    serde_json::from_str(text).map_err(|err| EngineError::LocalIo {
        path: "queue database".into(),
        message: format!("could not decode a queue value: {err}"),
    })
}

fn db(err: rusqlite::Error) -> EngineError {
    EngineError::LocalIo {
        path: "queue database".into(),
        message: err.to_string(),
    }
}

fn corrupt(what: &str) -> EngineError {
    EngineError::LocalIo {
        path: "queue database".into(),
        message: format!("the queue database holds an unreadable {what}"),
    }
}

fn gone() -> EngineError {
    EngineError::LocalIo {
        path: "queue database".into(),
        message: "the queue database worker is no longer running".into(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::interact::ConflictAction;
    use crate::job::JobKind;
    use crate::model::Direction;
    use crate::queue::{JobSpec, ORDER_STRIDE};
    use crate::wire::Order;

    use super::*;

    fn spec(item: &str) -> JobSpec {
        JobSpec {
            session: Uuid::new_v4(),
            server_id: Uuid::new_v4(),
            kind: JobKind::File,
            direction: Direction::Down,
            remote_path: format!("/remote/{item}"),
            local_path: PathBuf::from(format!("/local/{item}")),
            size: Some(Bytes(4096)),
            parent: None,
            item: item.to_string(),
        }
    }

    fn job(batch: Uuid, item: &str, position: i64) -> Job {
        Job::new(
            Uuid::new_v4(),
            Order(position * ORDER_STRIDE),
            batch,
            spec(item),
        )
    }

    #[tokio::test]
    async fn a_job_survives_the_round_trip_with_every_field_intact() {
        let store = QueueStore::in_memory().await.unwrap();
        let batch = Uuid::new_v4();
        let mut original = job(batch, "app.tar", 1);
        original.chosen = Some(ConflictAction::Overwrite);
        original.conflict_policy = Some(ConflictAction::Overwrite);
        original.attempts = 2;
        original.state = JobState::Paused {
            reason: PauseReason::User,
        };

        store.save(original.clone()).await.unwrap();
        let loaded = store.load_all().await.unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], original);
    }

    /// The bug this prevents: a user clicks Download once, the command is re-sent after
    /// an uncertain delivery, and the file transfers twice.
    #[tokio::test]
    async fn enqueueing_the_same_request_twice_returns_the_first_jobs() {
        let store = QueueStore::in_memory().await.unwrap();
        let batch = Uuid::new_v4();

        let first = store
            .insert_batch(vec![job(batch, "a.txt", 1), job(batch, "b.txt", 2)])
            .await
            .unwrap();
        // Same batch, same items, fresh job ids — as a replayed command would arrive.
        let second = store
            .insert_batch(vec![job(batch, "a.txt", 1), job(batch, "b.txt", 2)])
            .await
            .unwrap();

        assert_eq!(
            first.iter().map(|j| j.id).collect::<Vec<_>>(),
            second.iter().map(|j| j.id).collect::<Vec<_>>(),
            "the replay must find the existing jobs, not make new ones"
        );
        assert_eq!(store.load_all().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_different_gesture_for_the_same_file_is_a_different_job() {
        let store = QueueStore::in_memory().await.unwrap();
        store
            .insert_batch(vec![job(Uuid::new_v4(), "a.txt", 1)])
            .await
            .unwrap();
        store
            .insert_batch(vec![job(Uuid::new_v4(), "a.txt", 2)])
            .await
            .unwrap();

        assert_eq!(
            store.load_all().await.unwrap().len(),
            2,
            "asking for the same file again is a real second request"
        );
    }

    #[tokio::test]
    async fn a_batch_is_queued_whole_or_not_at_all() {
        let store = QueueStore::in_memory().await.unwrap();
        let batch = Uuid::new_v4();
        let mut clash = job(batch, "a.txt", 2);
        clash.id = Uuid::new_v4();

        // Two rows claiming the same primary key inside one batch: the insert fails
        // partway, and the transaction must take the first row back out with it.
        let mut duplicate = job(batch, "b.txt", 1);
        duplicate.id = clash.id;
        let result = store.insert_batch(vec![clash, duplicate]).await;

        assert!(result.is_err());
        assert!(
            store.load_all().await.unwrap().is_empty(),
            "a failed batch must leave nothing behind"
        );
    }

    #[tokio::test]
    async fn the_queue_comes_back_in_queue_order() {
        let store = QueueStore::in_memory().await.unwrap();
        let batch = Uuid::new_v4();
        for (item, position) in [("c", 3), ("a", 1), ("b", 2)] {
            store.save(job(batch, item, position)).await.unwrap();
        }
        let order: Vec<String> = store
            .load_all()
            .await
            .unwrap()
            .into_iter()
            .map(|j| j.item)
            .collect();
        assert_eq!(order, ["a", "b", "c"]);
    }

    #[tokio::test]
    async fn what_was_running_comes_back_paused_rather_than_queued_or_finished() {
        let store = QueueStore::in_memory().await.unwrap();
        let batch = Uuid::new_v4();
        for (index, state) in [
            JobState::Transferring,
            JobState::Preparing,
            JobState::Verifying,
            JobState::AwaitingPrompt {
                prompt: Uuid::new_v4(),
            },
            JobState::Scanning,
        ]
        .into_iter()
        .enumerate()
        {
            let mut running = job(batch, &format!("f{index}"), index as i64);
            running.state = state;
            store.save(running).await.unwrap();
        }

        let recovered = store.recover().await.unwrap();

        assert_eq!(recovered.len(), 5);
        for job in recovered {
            assert!(
                matches!(
                    job.state,
                    JobState::Paused {
                        reason: PauseReason::Restarted
                    }
                ),
                "{:?} should have been paused for verification, not restarted",
                job.state
            );
        }
    }

    #[tokio::test]
    async fn recovery_keeps_recent_history_and_drops_stale_history() {
        let store = QueueStore::in_memory().await.unwrap();
        let batch = Uuid::new_v4();

        let mut recent = job(batch, "recent", 1);
        recent.state = JobState::Done {
            at: Utc::now() - TimeDelta::days(1),
            skipped: false,
        };
        let mut stale = job(batch, "stale", 2);
        stale.state = JobState::Done {
            at: Utc::now() - KEEP_COMPLETED - TimeDelta::days(1),
            skipped: false,
        };
        // The bug this catches: `Cancelled` used to be a unit variant, so the prune's
        // `json_extract(state, '$.at')` came back NULL, the comparison was NULL, and
        // every cancelled job ever made stayed in the database for good.
        let mut abandoned = job(batch, "abandoned", 3);
        abandoned.state = JobState::Cancelled {
            at: Utc::now() - KEEP_COMPLETED - TimeDelta::days(1),
        };
        let mut queued = job(batch, "queued", 4);
        queued.state = JobState::Queued;

        for j in [recent, stale, abandoned, queued] {
            store.save(j).await.unwrap();
        }

        let items: Vec<String> = store
            .recover()
            .await
            .unwrap()
            .into_iter()
            .map(|j| j.item)
            .collect();
        assert_eq!(items, ["recent", "queued"]);
    }

    #[tokio::test]
    async fn a_checkpoint_is_written_with_the_job_and_removed_with_it() {
        let store = QueueStore::in_memory().await.unwrap();
        let batch = Uuid::new_v4();
        let mut owner = job(batch, "big.iso", 1);
        owner.resume = Some(ResumeRecord {
            checkpoint: Bytes(1024),
            prefix_sha256: "abc123".into(),
            finalization: Finalization::Intended,
            ..ResumeRecord::new(
                FileFacts {
                    path: "/remote/big.iso".into(),
                    size: Bytes(4096),
                    modified: Some(Utc::now()),
                    digest: None,
                },
                "/local/.big.iso.relaypart",
            )
        });
        let id = owner.id;
        store.save(owner.clone()).await.unwrap();

        let loaded = store.load_all().await.unwrap();
        assert_eq!(loaded[0].resume, owner.resume);

        store.remove(id).await.unwrap();
        let orphans: i64 = store
            .call(|conn| {
                conn.query_row("SELECT COUNT(*) FROM resume_records", [], |row| row.get(0))
                    .map_err(db)
            })
            .await
            .unwrap();
        assert_eq!(orphans, 0, "the cascade must take the checkpoint with it");
    }

    /// Displayed progress and durable progress are different numbers, and only one of
    /// them authorises a resume. Writing the cheap one must not move the other.
    #[tokio::test]
    async fn batched_progress_never_touches_the_checkpoint() {
        let store = QueueStore::in_memory().await.unwrap();
        let batch = Uuid::new_v4();
        let mut owner = job(batch, "big.iso", 1);
        owner.resume = Some(ResumeRecord::new(
            FileFacts {
                path: "/remote/big.iso".into(),
                size: Bytes(4096),
                modified: None,
                digest: None,
            },
            "/local/.big.iso.relaypart",
        ));
        let id = owner.id;
        store.save(owner).await.unwrap();

        store.save_progress(vec![(id, Bytes(3000))]).await.unwrap();

        let loaded = store.load_all().await.unwrap();
        assert_eq!(loaded[0].transferred, Bytes(3000));
        assert_eq!(
            loaded[0].resume.as_ref().unwrap().checkpoint,
            Bytes::ZERO,
            "progress is not proof"
        );
    }

    #[tokio::test]
    async fn clearing_completed_leaves_the_work_alone() {
        let store = QueueStore::in_memory().await.unwrap();
        let batch = Uuid::new_v4();
        let mut done = job(batch, "done", 1);
        done.state = JobState::Done {
            at: Utc::now(),
            skipped: false,
        };
        let mut cancelled = job(batch, "cancelled", 2);
        cancelled.state = JobState::Cancelled { at: Utc::now() };
        let mut failed = job(batch, "failed", 3);
        failed.state = JobState::Failed {
            error: EngineError::network("reset"),
            attempts: 3,
        };
        for j in [done, cancelled, failed, job(batch, "queued", 4)] {
            store.save(j).await.unwrap();
        }

        store.clear_completed().await.unwrap();

        let items: Vec<String> = store
            .load_all()
            .await
            .unwrap()
            .into_iter()
            .map(|j| j.item)
            .collect();
        assert_eq!(
            items,
            ["failed", "queued"],
            "a failed job is not completed; it is waiting for a person"
        );
    }

    #[tokio::test]
    async fn a_database_is_migrated_once_and_reopening_it_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay.sqlite");
        let batch = Uuid::new_v4();

        let store = QueueStore::open(&path).await.unwrap();
        store.save(job(batch, "a.txt", 1)).await.unwrap();
        drop(store);

        let store = QueueStore::open(&path).await.unwrap();
        assert_eq!(store.load_all().await.unwrap().len(), 1);
        let applied: i64 = store
            .call(|conn| {
                conn.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                    row.get(0)
                })
                .map_err(db)
            })
            .await
            .unwrap();
        assert_eq!(applied, SCHEMA_VERSION, "migrations must not run twice");
    }

    /// A downgraded Relay opening a newer queue would drop the columns it cannot see on
    /// the next write. Refusing to open is the only safe answer.
    #[tokio::test]
    async fn a_queue_from_a_newer_relay_is_refused_rather_than_damaged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay.sqlite");

        let store = QueueStore::open(&path).await.unwrap();
        store
            .call(|conn| {
                conn.execute(
                    "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                    params![SCHEMA_VERSION + 1, Utc::now().to_rfc3339()],
                )
                .map(|_| ())
                .map_err(db)
            })
            .await
            .unwrap();
        drop(store);

        let err = QueueStore::open(&path).await.unwrap_err();
        assert!(
            matches!(err, EngineError::Unsupported { .. }),
            "expected a refusal, got {err:?}"
        );
    }

    #[tokio::test]
    async fn a_database_that_cannot_be_opened_fails_at_open_not_at_every_call() {
        let dir = tempfile::tempdir().unwrap();
        // A directory is not a database file, and SQLite says so.
        let err = QueueStore::open(dir.path()).await.unwrap_err();
        assert!(matches!(err, EngineError::LocalIo { .. }), "got {err:?}");
    }
}
