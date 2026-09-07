import type { JobSnapshot } from '@/ipc/gen'

interface Props {
  jobs: JobSnapshot[]
}

/** The signature gutter: one pill per in-flight transfer, travelling in the direction
 * of the bytes. Position comes from real progress, so it is a readout, not decoration.
 *
 * The mark is a single dashed vertical spine between the panes, inset from both ends —
 * a channel, not a field. Drawn across the full width it read as a third empty pane,
 * which is the opposite of what a 52px gap is for. */
export function FlowGutter({ jobs }: Props) {
  const moving = jobs.filter((job) => job.state.kind === 'transferring')

  return (
    <div className={`gutter${moving.length === 0 ? ' gutter--idle' : ''}`}>
      <div className="gutter__spine" aria-hidden />
      <div className="gutter__count" aria-label={`${moving.length} transfers in flight`}>
        {moving.length}
      </div>
      <div className="gutter__pills" aria-hidden>
        {moving.slice(0, 5).map((job) => {
          const percent = job.size && job.size > 0 ? (job.transferred / job.size) * 100 : 0
          // Downloads travel right-to-left in the design: remote is the right pane.
          const top = job.direction === 'down' ? 100 - percent : percent
          return (
            <div
              key={job.id}
              className={`pill${job.direction === 'down' ? ' pill--down' : ''}`}
              style={{ top: `${Math.min(Math.max(top, 4), 92)}%` }}
            >
              {Math.round(percent)}%
            </div>
          )
        })}
      </div>
    </div>
  )
}
