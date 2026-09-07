import { create } from 'zustand'

import type { JobSnapshot, QueueStats } from '@/ipc/gen'

const emptyStats: QueueStats = { active: 0, queued: 0, failed: 0, done: 0, speedBps: 0 }

interface QueueState {
  jobs: Record<string, JobSnapshot>
  stats: QueueStats
  replaceAll: (jobs: JobSnapshot[], stats: QueueStats) => void
  upsert: (job: JobSnapshot) => void
}

/** Rust owns every transition. This store only reflects what it was told, which is why
 * there is no `advance` or `markDone` here. */
export const useQueueStore = create<QueueState>((set) => ({
  jobs: {},
  stats: emptyStats,
  replaceAll: (jobs, stats) =>
    set({ jobs: Object.fromEntries(jobs.map((job) => [job.id, job])), stats }),
  upsert: (job) => set((s) => ({ jobs: { ...s.jobs, [job.id]: job } })),
}))

/** Jobs in queue order, which is what both the drawer and the gutter render. */
export function selectOrderedJobs(state: QueueState): JobSnapshot[] {
  return Object.values(state.jobs).sort((a, b) => a.order - b.order)
}

export { emptyStats }
