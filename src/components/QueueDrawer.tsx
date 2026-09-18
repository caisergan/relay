import { useVirtualizer } from '@tanstack/react-virtual'
import { useMemo, useRef, useState } from 'react'

import { commands } from '@/ipc/commands'
import type { JobSnapshot, PauseReason, QueueOp } from '@/ipc/gen'
import { faultText } from '@/lib/errors'
import { formatBytes, formatElapsed, formatSpeed, formatStarted } from '@/lib/format'
import { useOrderedJobs, useQueueStore } from '@/state/queueStore'
import { useUiStore, type DrawerTab } from '@/state/uiStore'

import {
  IconArrowDown,
  IconArrowUp,
  IconChevronDown,
  IconClock,
  IconClose,
  IconPause,
  IconPlay,
  IconRetry,
  IconStopwatch,
} from './Icons'

const TABS: { id: DrawerTab; label: string }[] = [
  { id: 'active', label: 'Active' },
  { id: 'failed', label: 'Failed' },
  { id: 'completed', label: 'Completed' },
]

/** A row, before it has been measured. Every row in the drawer is the same height. */
const ROW_HEIGHT = 58

function bucket(job: JobSnapshot): DrawerTab {
  switch (job.state.kind) {
    case 'failed':
      return 'failed'
    case 'done':
    case 'cancelled':
      return 'completed'
    default:
      return 'active'
  }
}

/** The transfer drawer. Collapsed it is a 36px summary bar, which is the point: when
 * nothing is moving it should take a line, not a third of the window. */
export function QueueDrawer() {
  const jobs = useOrderedJobs()
  const stats = useQueueStore((s) => s.stats)
  const open = useUiStore((s) => s.drawerOpen)
  const tab = useUiStore((s) => s.drawerTab)
  const setTab = useUiStore((s) => s.setDrawerTab)
  const toggle = useUiStore((s) => s.toggleDrawer)
  const toast = useUiStore((s) => s.toast)

  /** Whether "Cancel all" has been armed. It throws away everything queued, which on
   * a folder transfer is thousands of files, so it asks once before it does. */
  const [confirming, setConfirming] = useState(false)

  /** Which row is being dragged, and which it is currently hovering over. */
  const [dragging, setDragging] = useState<string | null>(null)
  const [over, setOver] = useState<string | null>(null)
  // Rust owns the order. An optimistic reorder here would have to be reconciled
  // against the `JobUpdate` that follows, and the two would disagree the moment a
  // reorder raced a completion.
  const pending = useRef(false)

  const listRef = useRef<HTMLDivElement>(null)

  const run = (op: QueueOp) => {
    commands.queueControl(op).catch((error: unknown) => toast('error', faultText(error)))
  }

  // Five passes over the queue, so they are made once per change rather than once per
  // subscriber: on a folder of thousands these are the render.
  const { counts, shown, aggregate, anyRunning } = useMemo(() => {
    const counts: Record<DrawerTab, number> = { active: 0, failed: 0, completed: 0 }
    const shown: JobSnapshot[] = []
    let total = 0
    let carried = 0
    for (const job of jobs) {
      const where = bucket(job)
      counts[where] += 1
      if (where === tab) shown.push(job)
      // Aggregate progress across everything still moving, weighted by bytes rather
      // than by job count — one 4 GB image next to nine small files is not 90% done.
      if (job.state.kind === 'transferring') {
        total += job.size ?? 0
        carried += job.transferred
      }
    }
    return {
      counts,
      shown,
      aggregate: total > 0 ? Math.min((carried / total) * 100, 100) : 0,
      anyRunning: counts.active > 0,
    }
  }, [jobs, tab])

  const summary = [
    `${stats.active} active`,
    `${stats.queued} queued`,
    ...(stats.failed > 0 ? [`${stats.failed} failed`] : []),
  ].join(' · ')

  // Only the rows the viewport can reach are drawn. A folder transfer puts thousands
  // of jobs in here, and each one was a DOM subtree rebuilt on every progress tick.
  const virtualizer = useVirtualizer({
    count: shown.length,
    getScrollElement: () => listRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 10,
  })

  return (
    <div className={`drawer${open ? ' drawer--open' : ''}`}>
      <button
        className="drawer__bar"
        onClick={() => toggle()}
        aria-expanded={open}
        aria-label={open ? 'Collapse transfers' : 'Expand transfers'}
      >
        <IconChevronDown size={15} className="drawer__chevron" />
        <span className="drawer__title">Transfers</span>
        <span className="drawer__agg">
          <span className="drawer__agg-fill" style={{ width: `${aggregate}%` }} />
        </span>
        <span className="drawer__speed">{formatSpeed(stats.speedBps || null)}</span>
        <span style={{ flex: 1 }} />
        <span className="drawer__summary">{summary}</span>
      </button>

      {open && (
        <div className="drawer__panel">
          <div className="drawer__tabs">
            {TABS.map((entry) => (
              <button
                key={entry.id}
                className={`drawer__tab${tab === entry.id ? ' drawer__tab--active' : ''}`}
                onClick={() => setTab(entry.id)}
              >
                {entry.label}
                {counts[entry.id] > 0 && (
                  <span
                    className={`drawer__badge${entry.id === 'failed' ? ' drawer__badge--bad' : ''}`}
                  >
                    {counts[entry.id]}
                  </span>
                )}
              </button>
            ))}
            <span style={{ flex: 1 }} />
            {tab === 'active' && anyRunning && (
              <button className="drawer__act" onClick={() => run({ kind: 'pauseAll' })}>
                Pause all
              </button>
            )}
            {tab === 'active' && !anyRunning && jobs.length > 0 && (
              <button className="drawer__act" onClick={() => run({ kind: 'resumeAll' })}>
                Resume all
              </button>
            )}
            {tab === 'active' && counts.active > 0 && (
              <button
                className={`drawer__act${confirming ? ' drawer__act--danger' : ''}`}
                onClick={() => {
                  if (!confirming) {
                    setConfirming(true)
                    return
                  }
                  setConfirming(false)
                  run({ kind: 'cancelAll' })
                }}
                onBlur={() => setConfirming(false)}
              >
                {confirming ? `Cancel ${counts.active}?` : 'Cancel all'}
              </button>
            )}
            {tab === 'completed' && counts.completed > 0 && (
              <button className="drawer__act" onClick={() => run({ kind: 'clearCompleted' })}>
                Clear completed
              </button>
            )}
          </div>
          <div className="drawer__list" ref={listRef}>
            {shown.length === 0 && (
              <div className="drawer__empty">
                Nothing {tab === 'active' ? 'in flight' : tab}.
              </div>
            )}
            <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
              {virtualizer.getVirtualItems().map((item) => {
                const job = shown[item.index]
                if (!job) return null
                const percent =
                  job.size && job.size > 0
                    ? Math.min((job.transferred / job.size) * 100, 100)
                    : 0
                const done = job.state.kind === 'done'
                const failed = job.state.kind === 'failed'
                // A child of a folder job is drawn under it, so the drawer reads as the
                // gesture that made it rather than as a flat list of unrelated files.
                const nested = job.parent !== null
                return (
                  <div
                    className={`job${nested ? ' job--child' : ''}${
                      over === job.id && dragging !== job.id ? ' job--over' : ''
                    }${dragging === job.id ? ' job--dragging' : ''}`}
                    key={job.id}
                    ref={virtualizer.measureElement}
                    data-index={item.index}
                    style={{
                      position: 'absolute',
                      top: 0,
                      left: 0,
                      right: 0,
                      transform: `translateY(${item.start}px)`,
                    }}
                    // Only queued work can be reordered: dragging a finished job would
                    // be moving something that has already happened.
                    draggable={tab === 'active' && !nested}
                    onDragStart={() => setDragging(job.id)}
                    onDragEnd={() => {
                      setDragging(null)
                      setOver(null)
                    }}
                    onDragOver={(e) => {
                      if (dragging === null || dragging === job.id) return
                      e.preventDefault()
                      setOver(job.id)
                    }}
                    onDrop={(e) => {
                      e.preventDefault()
                      const moved = dragging
                      setDragging(null)
                      setOver(null)
                      if (moved === null || moved === job.id || pending.current) return
                      pending.current = true
                      // Dropped *onto* a row means "go before it", which is the row
                      // above's `after`. Rust recomputes the position and the update
                      // comes back through the ordinary event stream.
                      const index = shown.findIndex((row) => row.id === job.id)
                      const previous = shown[index - 1]
                      commands
                        .queueControl({
                          kind: 'reorder',
                          job: moved,
                          after: previous ? previous.id : null,
                        })
                        .catch((error: unknown) => toast('error', faultText(error)))
                        .finally(() => {
                          pending.current = false
                        })
                    }}
                  >
                    <span
                      className={`job__dir${job.direction === 'down' ? ' job__dir--down' : ''}`}
                    >
                      {job.direction === 'down' ? (
                        <IconArrowDown size={13} />
                      ) : (
                        <IconArrowUp size={13} />
                      )}
                    </span>
                    <div className="job__body">
                      <div className="job__line">
                        <span className="job__name" title={job.remotePath}>
                          {job.remotePath.split('/').pop()}
                        </span>
                        <span className={`job__status job__status--${statusTone(job)}`}>
                          {describe(job)}
                        </span>
                      </div>
                      <div className="job__bar">
                        <div
                          className={`job__fill${done ? ' job__fill--done' : ''}${
                            failed ? ' job__fill--failed' : ''
                          }`}
                          style={{ width: `${done ? 100 : percent}%` }}
                        />
                      </div>
                    </div>
                    <Facts job={job} jobs={jobs} />
                    <div className="job__acts">
                      {job.state.kind === 'transferring' && (
                        <IconButton
                          label="Pause this transfer"
                          onClick={() => run({ kind: 'pause', job: job.id })}
                        >
                          <IconPause size={13} />
                        </IconButton>
                      )}
                      {job.state.kind === 'paused' && (
                        <IconButton
                          label="Resume this transfer"
                          onClick={() => run({ kind: 'resume', job: job.id })}
                        >
                          <IconPlay size={13} />
                        </IconButton>
                      )}
                      {failed && (
                        <IconButton
                          label="Try this transfer again"
                          onClick={() => run({ kind: 'retry', job: job.id })}
                        >
                          <IconRetry size={13} />
                        </IconButton>
                      )}
                      {bucket(job) !== 'completed' && (
                        <IconButton
                          label="Cancel this transfer"
                          danger
                          onClick={() => run({ kind: 'cancel', job: job.id })}
                        >
                          <IconClose size={13} />
                        </IconButton>
                      )}
                    </div>
                  </div>
                )
              })}
            </div>
          </div>
        </div>
      )}
    </div>
  )
}

/** The three figures at the end of a row, which depend on where the job is. While it
 * moves: how much has arrived, how fast, and how long is left. Once it has stopped: how
 * much, how long it took, and when it began.
 *
 * A figure that does not apply is left out rather than drawn as a dash. The row used to
 * end in speed and ETA whatever state it was in, so every finished transfer closed on
 * two dashes that said nothing about what had happened. */
function Facts({ job, jobs }: { job: JobSnapshot; jobs: JobSnapshot[] }) {
  const { started, ended } = timing(job, jobs)
  const elapsed = started && ended ? Date.parse(ended) - Date.parse(started) : null
  const size = sizeText(job)

  if (job.state.kind === 'transferring') {
    return (
      <>
        <span
          className="job__meta"
          title={job.size ? `${size} of ${formatBytes(job.size)}` : undefined}
        >
          {size}
        </span>
        <span className="job__meta" title={job.speedBps ? 'Current speed' : undefined}>
          {job.speedBps ? formatSpeed(job.speedBps) : ''}
        </span>
        <span
          className="job__meta job__meta--when"
          title={job.etaSecs !== null ? 'Estimated time left' : undefined}
        >
          {job.etaSecs !== null ? `${formatEta(job.etaSecs)} left` : ''}
        </span>
      </>
    )
  }

  return (
    <>
      <span className="job__meta" title={size ? `${size} transferred` : undefined}>
        {size}
      </span>
      <span
        className="job__meta"
        title={elapsed !== null ? `Took ${formatElapsed(elapsed)}` : undefined}
      >
        {elapsed !== null && (
          <>
            <IconStopwatch size={11} />
            {formatElapsed(elapsed)}
          </>
        )}
      </span>
      <span
        className="job__meta job__meta--when"
        title={started ? `Started ${new Date(started).toLocaleString()}` : undefined}
      >
        {started && (
          <>
            <IconClock size={11} />
            {formatStarted(started)}
          </>
        )}
      </span>
    </>
  )
}

/** When a job started, and when it stopped if it has. A folder never runs itself — its
 * files do — so its start is the first of theirs; it stops when the last of them does,
 * which is the moment the folder is marked done. */
function timing(
  job: JobSnapshot,
  jobs: JobSnapshot[],
): { started: string | null; ended: string | null } {
  const ended =
    job.state.kind === 'done' || job.state.kind === 'cancelled' ? job.state.at : null
  if (job.kind !== 'folder') return { started: job.startedAt, ended }
  const starts = jobs.flatMap((child) =>
    child.parent === job.id && child.startedAt !== null ? [child.startedAt] : [],
  )
  const started = starts.reduce<string | null>(
    (first, at) => (first === null || Date.parse(at) < Date.parse(first) ? at : first),
    null,
  )
  return { started, ended }
}

/** Bytes moved, or nothing while there are none to speak of: a job that has not moved a
 * byte yet — queued, or failed before it began — and a skip, which moved nothing on
 * purpose. An empty file that arrived is still "0 B": that is its size, not an absence. */
function sizeText(job: JobSnapshot): string {
  if (job.state.kind === 'done') return job.state.skipped ? '' : formatBytes(job.transferred)
  return job.transferred === 0 ? '' : formatBytes(job.transferred)
}

function IconButton({
  label,
  danger,
  onClick,
  children,
}: {
  label: string
  danger?: boolean
  onClick: () => void
  children: React.ReactNode
}) {
  return (
    <button
      className={`acts__btn${danger ? ' acts__btn--danger' : ''}`}
      title={label}
      aria-label={label}
      onClick={onClick}
    >
      {children}
    </button>
  )
}

function statusTone(job: JobSnapshot): string {
  switch (job.state.kind) {
    case 'failed':
      return 'bad'
    case 'done':
      // A skipped job succeeded at doing nothing. Drawing it in the success colour
      // would say a file was transferred that deliberately was not.
      return job.state.skipped ? 'idle' : 'ok'
    case 'transferring':
      return 'transit'
    default:
      return 'idle'
  }
}

function describe(job: JobSnapshot): string {
  switch (job.state.kind) {
    case 'failed':
      return job.state.error.kind
    case 'paused':
      return pauseWords(job.state.reason)
    case 'done':
      return job.state.skipped ? 'skipped' : 'done'
    case 'queued':
      // A job waiting out a backoff *is* queued, but saying only "queued" hides the
      // fact that something went wrong and is being tried again.
      return job.retryAt !== null ? `retrying (attempt ${job.attempts + 1})` : 'queued'
    case 'scanning':
      return 'scanning folder'
    case 'awaitingPrompt':
      return 'waiting for you'
    default:
      return job.state.kind
  }
}

/** The pause reasons in words a person can act on, rather than the enum's spelling. */
function pauseWords(reason: PauseReason): string {
  switch (reason) {
    case 'user':
      return 'paused'
    case 'sessionDown':
      return 'waiting for the server'
    case 'throttled':
      return 'waiting for a slot'
    case 'restarted':
      return 'paused (app restarted)'
    default:
      return 'paused'
  }
}

/** Seconds as a short duration. The engine already returns `null` below 5 KB/s rather
 * than dividing by a rate that is almost zero and promising four days. */
function formatEta(seconds: number | null): string {
  if (seconds === null) return '—'
  if (seconds < 60) return `${seconds}s`
  if (seconds < 3600) return `${Math.round(seconds / 60)}m`
  return `${Math.round(seconds / 360) / 10}h`
}
