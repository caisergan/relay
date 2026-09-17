/** Keeps a pane true to what a transfer just did to the directory it is showing.
 *
 * Nothing tells us a directory changed. SFTP has no notifications, and the local pane
 * lists only when it navigates — so a download finished, and the folder it landed in
 * went on showing itself as it was before, until the person left it and came back. A
 * transfer stopping is the one moment we *know* something in a directory may be
 * different, so that is when the pane showing it lists again.
 *
 * Fed from the engine stream rather than from the session view, which is mounted for the
 * active tab only: a download that finishes behind another tab should be there when that
 * tab is brought forward. And quiet — no loading state, no skeleton — because the person
 * did not ask for it, and a listing that blinked on every file of a batch would be worse
 * than a stale one. */

import { commands } from '@/ipc/commands'
import type { JobSnapshot, JobState } from '@/ipc/gen'

import { useSessionsStore } from './sessionsStore'

type Side = 'local' | 'remote'

/** How long arrivals are gathered before the pane lists. Short enough that one file is
 * on screen before anyone reaches for the refresh button; long enough that files
 * finishing together — a batch of small ones on four lanes — are one listing, not four. */
export const SETTLE_MS = 300

/** Whether a job that has just reached `state` may have changed its destination's
 * directory. A skip left it alone by definition. A failure or a cancellation may not
 * have: a folder's directory is made before its files, and a partial that an earlier
 * refresh put on screen is removed when its transfer is cancelled. */
export function mayHaveChanged(state: JobState): boolean {
  switch (state.kind) {
    case 'done':
      return !state.skipped
    case 'failed':
    case 'cancelled':
      return true
    default:
      return false
  }
}

/** A path's names, on either platform: a local path can be a Windows one. */
function segments(path: string): string[] {
  return path.split(/[/\\]+/).filter(Boolean)
}

/** The names between `dir` and `path`, or null when `path` is not inside `dir`. */
function below(dir: string, path: string): string[] | null {
  const base = segments(dir)
  const full = segments(path)
  if (full.length < base.length || base.some((name, i) => name !== full[i])) return null
  return full.slice(base.length)
}

/** Whether something arriving at `dest` can have changed the listing of `dir`, a
 * directory whose rows are named `names`.
 *
 * - Directly inside it: a row appeared, or changed its size and time.
 * - Deeper: only when the folder it came through is not a row yet. A folder of a
 *   thousand files is one row in the directory above it, and listing that directory once
 *   per file would change nothing but the row's time.
 * - A folder transfer that finishes changes any directory within it as well, because its
 *   walk creates the empty directories no file job ever reports. */
export function changesListing(
  dir: string,
  names: readonly string[],
  dest: string,
  isFolder: boolean,
): boolean {
  if (isFolder && below(dest, dir) !== null) return true
  const [first, ...deeper] = below(dir, dest) ?? []
  if (first === undefined) return false
  return deeper.length === 0 || !names.includes(first)
}

interface Arrival {
  dest: string
  isFolder: boolean
}

interface View {
  dir: string
  names: string[]
  /** A navigation is under way. What it lists may predate an arrival, and the directory
   * it lands on is the one to judge the arrival against. */
  busy: boolean
}

/** What a pane is showing, or null when it is showing no listing to keep current: not
 * listed yet, a fault that is the pane's answer until the person moves, or a server that
 * is not connected. */
function view(session: string, side: Side): View | null {
  const state = useSessionsStore.getState()
  const pane = state.panes[session]
  if (!pane) return null
  if (side === 'local') {
    if (!pane.localPath || pane.localError) return null
    return {
      dir: pane.localPath,
      names: pane.localEntries.map((entry) => entry.name),
      busy: pane.localLoading,
    }
  }
  const listing = state.listings[session]
  if (!listing || pane.remoteError || state.sessions[session]?.state.kind !== 'connected') {
    return null
  }
  return {
    dir: listing.path,
    names: listing.entries.map((entry) => entry.name),
    busy: pane.remoteLoading,
  }
}

interface Lane {
  /** Judged when the pane lists rather than when they arrive, against wherever the pane
   * is by then: an arrival that came in mid-navigation belongs to where it was going. */
  arrivals: Arrival[]
  timer: ReturnType<typeof setTimeout> | null
  /** A listing is on its way. Arrivals meanwhile wait for it rather than racing it. */
  listing: boolean
}

const lanes = new Map<string, Lane>()

const keyOf = (session: string, side: Side) => `${session}\n${side}`

/** Note a job that has stopped, and list the pane its destination is in if that changes
 * what the pane shows. Call it on the transition, not on every update of a stopped job. */
export function noteArrival(job: JobSnapshot): void {
  const side: Side = job.direction === 'down' ? 'local' : 'remote'
  const key = keyOf(job.session, side)
  const lane = lanes.get(key) ?? { arrivals: [], timer: null, listing: false }
  lanes.set(key, lane)
  lane.arrivals.push({
    dest: side === 'local' ? job.localPath : job.remotePath,
    isFolder: job.kind === 'folder',
  })
  settle(job.session, side, lane)
}

function settle(session: string, side: Side, lane: Lane): void {
  if (lane.timer !== null || lane.listing) return
  lane.timer = setTimeout(() => {
    lane.timer = null
    void flush(session, side, lane)
  }, SETTLE_MS)
}

async function flush(session: string, side: Side, lane: Lane): Promise<void> {
  const shown = view(session, side)
  if (shown?.busy) {
    settle(session, side, lane)
    return
  }
  const arrivals = lane.arrivals
  lane.arrivals = []
  if (
    shown &&
    arrivals.some((a) => changesListing(shown.dir, shown.names, a.dest, a.isFolder))
  ) {
    lane.listing = true
    try {
      await (side === 'local'
        ? relistLocal(session, shown.dir)
        : relistRemote(session, shown.dir))
    } finally {
      lane.listing = false
    }
  }
  if (lane.arrivals.length > 0) settle(session, side, lane)
  else lanes.delete(keyOf(session, side))
}

async function relistLocal(session: string, dir: string): Promise<void> {
  const before = useSessionsStore.getState().panes[session]?.localEntries
  // A directory that cannot be read now is the next navigation's to report. Nobody
  // asked for this listing, so nobody is waiting for its error.
  const entries = await commands.localListDir(dir).catch(() => null)
  const pane = useSessionsStore.getState().panes[session]
  // Anything that listed or moved the pane while this was reading knows better: it
  // started later, or it is taking the pane somewhere else.
  if (
    !entries ||
    !pane ||
    pane.localPath !== dir ||
    pane.localLoading ||
    pane.localEntries !== before
  ) {
    return
  }
  useSessionsStore.getState().patchPane(session, { localEntries: entries })
}

/** Through the session, which lists only if the pane has not moved on by the time the
 * request reaches the front of its queue; the listing itself arrives on the engine
 * stream like any other. */
async function relistRemote(session: string, dir: string): Promise<void> {
  await commands.sessionRelist(session, dir).catch(() => false)
}
