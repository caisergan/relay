import { create } from 'zustand'

import type { JobSnapshot, QueueStats } from '@/ipc/gen'

const emptyStats: QueueStats = { active: 0, queued: 0, failed: 0, done: 0, speedBps: 0 }

interface QueueState {
  jobs: Record<string, JobSnapshot>
  /** The same jobs in queue order.
   *
   * Kept in the store rather than derived per render. As a selector it re-sorted the
   * whole queue on every progress tick, for every subscriber; a folder transfer puts
   * thousands of jobs in here, and that sort was most of a frame. */
  ordered: JobSnapshot[]
  stats: QueueStats
  replaceAll: (jobs: JobSnapshot[], stats: QueueStats) => void
  upsert: (job: JobSnapshot) => void
}

const byOrder = (a: JobSnapshot, b: JobSnapshot) => a.order - b.order

/** Rust owns every transition. This store only reflects what it was told, which is why
 * there is no `advance` or `markDone` here. */
export const useQueueStore = create<QueueState>((set) => ({
  jobs: {},
  ordered: [],
  stats: emptyStats,
  replaceAll: (jobs, stats) =>
    set({
      jobs: Object.fromEntries(jobs.map((job) => [job.id, job])),
      ordered: [...jobs].sort(byOrder),
      stats,
    }),
  upsert: (job) => {
    pending.set(job.id, job)
    if (frame === null) frame = schedule(flushJobUpdates)
  },
}))

/** Updates waiting for the next frame.
 *
 * Progress arrives about ten times a second per transfer, and each one used to be its
 * own store write and its own render of the drawer. Coalescing them costs at most a
 * frame of staleness on a number that is changing anyway. */
const pending = new Map<string, JobSnapshot>()
let frame: number | null = null

const schedule = (run: () => void): number =>
  typeof requestAnimationFrame === 'function'
    ? requestAnimationFrame(run)
    : Number(setTimeout(run, 16))

/** Apply everything buffered, now. Exported for tests, which cannot wait for a frame. */
export function flushJobUpdates(): void {
  frame = null
  if (pending.size === 0) return
  const batch = new Map(pending)
  pending.clear()
  useQueueStore.setState((state) => {
    const jobs = { ...state.jobs }
    // A job that is new, or that a reorder has moved, is the only reason to sort
    // again. Everything else is a job changing in place.
    let resort = false
    for (const [id, job] of batch) {
      const before = jobs[id]
      if (before === undefined || before.order !== job.order) resort = true
      jobs[id] = job
    }
    return {
      jobs,
      ordered: resort
        ? Object.values(jobs).sort(byOrder)
        : state.ordered.map((job) => batch.get(job.id) ?? job),
    }
  })
}

/** What the queue last held for a job, buffer included.
 *
 * Read by anything distinguishing a transition from a repeat. It has to see the update
 * immediately before this one rather than the one that last reached the store, or a
 * whole frame of progress ticks each look like a fresh arrival. */
export function lastStateOf(id: string): JobSnapshot['state']['kind'] | undefined {
  return (pending.get(id) ?? useQueueStore.getState().jobs[id])?.state.kind
}

export function useOrderedJobs(): JobSnapshot[] {
  return useQueueStore((s) => s.ordered)
}

export { emptyStats }
