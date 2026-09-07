import { create } from 'zustand'

import type { Sort } from '@/components/FileList'
import type { ListingSnapshot, LocalEntry, Session, SessionState } from '@/ipc/gen'
import type { Fault } from '@/lib/errors'
import { emptyHistory, type History } from '@/lib/history'

/** Per-tab view state. Purely local: the engine owns everything else. */
export interface PaneState {
  localPath: string
  localEntries: LocalEntry[]
  localLoading: boolean
  /** The structured fault, not a string: the pane picks a designed state from the
   * kind, which a stringified error throws away. */
  localError: Fault | null
  remoteFilter: string
  localFilter: string
  remoteLoading: boolean
  /** A failed listing belongs to the pane, not to a toast that outlives it. */
  remoteError: Fault | null
  /** Sort is per pane and per tab: the local side is usually a project directory and
   * the remote side a deploy target, and they rarely want the same order. */
  localSort: Sort
  remoteSort: Sort
  /** Whether dotfiles are listed at all. Per pane and per tab, like the sort: the
   * local side is usually a project directory where dotfiles are noise, and the remote
   * side a server where `.htaccess` and `.env` are the whole reason to be looking.
   * Defaults to showing them, which is what the design draws. */
  localShowHidden: boolean
  remoteShowHidden: boolean
  /** Where each pane has been, so back and forward can retrace it. The breadcrumb only
   * goes up; returning to a sibling you were just in has no other route. */
  localHistory: History
  remoteHistory: History
  /** The focused row's name, or null. Single-select for now; the design's multi-select
   * (⌘-click, shift-range) lands with the batch queue in phase 2. */
  localSelected: string | null
  remoteSelected: string | null
}

const emptyPane: PaneState = {
  localPath: '',
  localEntries: [],
  localLoading: false,
  localError: null,
  remoteFilter: '',
  localFilter: '',
  remoteLoading: false,
  remoteError: null,
  localSort: { key: 'name', dir: 1 },
  remoteSort: { key: 'name', dir: 1 },
  localShowHidden: true,
  remoteShowHidden: true,
  localHistory: emptyHistory,
  remoteHistory: emptyHistory,
  localSelected: null,
  remoteSelected: null,
}

interface SessionsState {
  sessions: Record<string, Session>
  /** Tab order, which the engine does not care about. */
  order: string[]
  activeId: string | null
  listings: Record<string, ListingSnapshot>
  panes: Record<string, PaneState>

  replaceAll: (sessions: Session[], listings: ListingSnapshot[]) => void
  upsert: (session: Session) => void
  setState: (id: string, state: SessionState) => void
  setLatency: (id: string, ms: number) => void
  setListing: (listing: ListingSnapshot) => void
  remove: (id: string) => void
  activate: (id: string | null) => void
  patchPane: (id: string, patch: Partial<PaneState>) => void
}

export const useSessionsStore = create<SessionsState>((set) => ({
  sessions: {},
  order: [],
  activeId: null,
  listings: {},
  panes: {},

  replaceAll: (sessions, listings) =>
    set((s) => {
      const byId = Object.fromEntries(sessions.map((session) => [session.id, session]))
      // Keep the tab order we already have; append anything the snapshot adds.
      const kept = s.order.filter((id) => id in byId)
      const added = sessions.map((x) => x.id).filter((id) => !kept.includes(id))
      const order = [...kept, ...added]
      return {
        sessions: byId,
        order,
        activeId: s.activeId && order.includes(s.activeId) ? s.activeId : (order[0] ?? null),
        listings: Object.fromEntries(listings.map((l) => [l.session, l])),
        panes: Object.fromEntries(order.map((id) => [id, s.panes[id] ?? emptyPane])),
      }
    }),

  upsert: (session) =>
    set((s) => ({
      sessions: { ...s.sessions, [session.id]: session },
      order: s.order.includes(session.id) ? s.order : [...s.order, session.id],
      activeId: s.activeId ?? session.id,
      panes: { ...s.panes, [session.id]: s.panes[session.id] ?? emptyPane },
    })),

  setState: (id, state) =>
    set((s) => {
      const session = s.sessions[id]
      if (!session) return s
      return { sessions: { ...s.sessions, [id]: { ...session, state } } }
    }),

  setLatency: (id, ms) =>
    set((s) => {
      const session = s.sessions[id]
      if (!session) return s
      return { sessions: { ...s.sessions, [id]: { ...session, latencyMs: ms } } }
    }),

  setListing: (listing) =>
    set((s) => {
      const current = s.listings[listing.session]
      // The engine already drops stale listings, but the pane must not regress either.
      if (current && current.request > listing.request) return s
      const session = s.sessions[listing.session]
      return {
        listings: { ...s.listings, [listing.session]: listing },
        sessions: session
          ? { ...s.sessions, [listing.session]: { ...session, remotePath: listing.path } }
          : s.sessions,
        panes: {
          ...s.panes,
          [listing.session]: {
            ...(s.panes[listing.session] ?? emptyPane),
            remoteLoading: false,
            // A listing that arrived is the answer to whatever failed before it.
            // Left set, a denied pane would sit over the directory that succeeded.
            remoteError: null,
          },
        },
      }
    }),

  remove: (id) =>
    set((s) => {
      const { [id]: _session, ...sessions } = s.sessions
      const { [id]: _listing, ...listings } = s.listings
      const { [id]: _pane, ...panes } = s.panes
      const order = s.order.filter((x) => x !== id)
      return {
        sessions,
        listings,
        panes,
        order,
        activeId: s.activeId === id ? (order[order.length - 1] ?? null) : s.activeId,
      }
    }),

  activate: (activeId) => set({ activeId }),

  patchPane: (id, patch) =>
    set((s) => ({ panes: { ...s.panes, [id]: { ...(s.panes[id] ?? emptyPane), ...patch } } })),
}))

export { emptyPane }
