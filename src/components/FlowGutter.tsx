import type { JobSnapshot } from '@/ipc/gen'

interface Props {
  jobs: JobSnapshot[]
}

/** The signature gutter: one pill per in-flight transfer, travelling in the direction
 * of the bytes. Position comes from real progress, so it is a readout, not decoration. */
export function FlowGutter({ jobs }: Props) {
  const moving = jobs.filter((job) => job.state.kind === 'transferring')

  return (
    <div className={`gutter${moving.length === 0 ? ' gutter--idle' : ''}`} aria-hidden>
      <div className="gutter__track" />
      {moving.map((job) => {
        const percent = job.size && job.size > 0 ? (job.transferred / job.size) * 100 : 0
        // Downloads travel right-to-left in the design: remote is the right pane.
        const top = job.direction === 'down' ? 100 - percent : percent
        return (
          <div
            key={job.id}
            className={`pill${job.direction === 'down' ? ' pill--down' : ''}`}
            style={{ top: `calc(${Math.min(Math.max(top, 2), 94)}% )` }}
          >
            {Math.round(percent)}%
          </div>
        )
      })}
    </div>
  )
}
