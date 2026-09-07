import { commands } from '@/ipc/commands'
import type { JobSnapshot } from '@/ipc/gen'
import { faultText } from '@/lib/errors'
import { formatBytes, formatSpeed } from '@/lib/format'
import { useOrderedJobs, useQueueStore } from '@/state/queueStore'
import { useUiStore, type DrawerTab } from '@/state/uiStore'

import { IconArrowDown, IconArrowUp, IconChevronDown, IconClose } from './Icons'

const TABS: { id: DrawerTab; label: string }[] = [
  { id: 'active', label: 'Active' },
  { id: 'failed', label: 'Failed' },
  { id: 'completed', label: 'Completed' },
]

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

  const counts: Record<DrawerTab, number> = { active: 0, failed: 0, completed: 0 }
  for (const job of jobs) counts[bucket(job)] += 1
  const shown = jobs.filter((job) => bucket(job) === tab)

  // Aggregate progress across everything still moving, weighted by bytes rather than
  // by job count — one 4 GB image next to nine small files is not 90% done.
  const moving = jobs.filter((job) => job.state.kind === 'transferring')
  const total = moving.reduce((sum, job) => sum + (job.size ?? 0), 0)
  const carried = moving.reduce((sum, job) => sum + job.transferred, 0)
  const aggregate = total > 0 ? Math.min((carried / total) * 100, 100) : 0

  const summary = [
    `${stats.active} active`,
    `${stats.queued} queued`,
    ...(stats.failed > 0 ? [`${stats.failed} failed`] : []),
  ].join(' · ')

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
          </div>
          <div className="drawer__list">
            {shown.length === 0 && (
              <div className="drawer__empty">
                Nothing {tab === 'active' ? 'in flight' : tab}.
              </div>
            )}
            {shown.map((job) => {
              const percent =
                job.size && job.size > 0 ? Math.min((job.transferred / job.size) * 100, 100) : 0
              const done = job.state.kind === 'done'
              const failed = job.state.kind === 'failed'
              return (
                <div className="job" key={job.id}>
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
                  <span className="job__meta">
                    {job.size === null ? '—' : formatBytes(job.transferred)}
                  </span>
                  <span className="job__meta">{formatSpeed(job.speedBps)}</span>
                  {job.state.kind === 'transferring' && (
                    <button
                      className="acts__btn acts__btn--danger"
                      title="Cancel this transfer"
                      aria-label="Cancel this transfer"
                      onClick={() => {
                        commands
                          .queueControl({ kind: 'cancel', job: job.id })
                          .catch((error: unknown) => toast('error', faultText(error)))
                      }}
                    >
                      <IconClose size={13} />
                    </button>
                  )}
                </div>
              )
            })}
          </div>
        </div>
      )}
    </div>
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
      return `paused (${job.state.reason})`
    case 'done':
      return job.state.skipped ? 'skipped' : 'done'
    default:
      return job.state.kind
  }
}
