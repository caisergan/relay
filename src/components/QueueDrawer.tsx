import { commands } from '@/ipc/commands'
import type { JobSnapshot } from '@/ipc/gen'
import { formatBytes, formatSpeed } from '@/lib/format'
import { useOrderedJobs } from '@/state/queueStore'
import { useUiStore, type DrawerTab } from '@/state/uiStore'

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

export function QueueDrawer() {
  const jobs = useOrderedJobs()
  const open = useUiStore((s) => s.drawerOpen)
  const tab = useUiStore((s) => s.drawerTab)
  const setTab = useUiStore((s) => s.setDrawerTab)
  const toggle = useUiStore((s) => s.toggleDrawer)
  const toast = useUiStore((s) => s.toast)

  const counts: Record<DrawerTab, number> = { active: 0, failed: 0, completed: 0 }
  for (const job of jobs) counts[bucket(job)] += 1
  const shown = jobs.filter((job) => bucket(job) === tab)

  return (
    <div className="drawer" style={open ? undefined : { maxHeight: 34 }}>
      <div className="drawer__tabs">
        {TABS.map((entry) => (
          <button
            key={entry.id}
            className={`drawer__tab${tab === entry.id ? ' drawer__tab--active' : ''}`}
            onClick={() => setTab(entry.id)}
          >
            {entry.label} {counts[entry.id] > 0 && <span>({counts[entry.id]})</span>}
          </button>
        ))}
        <button
          className="iconbtn"
          style={{ marginLeft: 'auto' }}
          onClick={() => toggle()}
          aria-label={open ? 'Collapse queue' : 'Expand queue'}
        >
          {open ? '▾' : '▴'}
        </button>
      </div>
      {open && (
        <div className="drawer__list">
          {shown.length === 0 && (
            <div className="empty" style={{ padding: 20 }}>
              <span>Nothing {tab === 'active' ? 'in flight' : tab}.</span>
            </div>
          )}
          {shown.map((job) => {
            const percent =
              job.size && job.size > 0 ? Math.min((job.transferred / job.size) * 100, 100) : 0
            const fill =
              job.state.kind === 'done'
                ? ' job__fill--done'
                : job.state.kind === 'failed'
                  ? ' job__fill--failed'
                  : ''
            return (
              <div className="job" key={job.id}>
                <span aria-hidden>{job.direction === 'down' ? '↓' : '↑'}</span>
                <span className="row__name" title={job.remotePath}>
                  {job.remotePath.split('/').pop()}
                </span>
                <div className="job__bar">
                  <div
                    className={`job__fill${fill}`}
                    style={{ width: `${job.state.kind === 'done' ? 100 : percent}%` }}
                  />
                </div>
                <span className="row__meta" style={{ width: 88 }}>
                  {job.size === null ? '—' : formatBytes(job.transferred)}
                </span>
                <span className="row__meta" style={{ width: 78 }}>
                  {formatSpeed(job.speedBps)}
                </span>
                <span className="row__meta" style={{ width: 90 }}>
                  {describe(job)}
                </span>
                {job.state.kind === 'transferring' && (
                  <button
                    className="iconbtn"
                    onClick={() => {
                      commands
                        .queueControl({ kind: 'cancel', job: job.id })
                        .catch((error: unknown) => toast('error', String(error)))
                    }}
                  >
                    Cancel
                  </button>
                )}
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}

function describe(job: JobSnapshot): string {
  switch (job.state.kind) {
    case 'failed':
      return job.state.error.kind
    case 'paused':
      return `paused (${job.state.reason})`
    case 'done':
      return 'done'
    default:
      return job.state.kind
  }
}
