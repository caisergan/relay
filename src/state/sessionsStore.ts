import { create } from 'zustand'

import type { ListingSnapshot, LocalEntry, Session, SessionState } from '@/ipc/gen'

/** Per-tab view state. Purely local: the engine owns everything else. */
export interface PaneState {
  localPath: string
  localEntries: LocalEntry[]
  localLoading: boolean
  localError: string | null
  remoteFilter: string
  localFilter: string
  remoteLoading: boolean
}

const emptyPane: PaneState = {
  localPath: '',
  localEntries: [],
  localLoading: false,
  localError: null,
  remoteFilter: '',
  localFilter: '',
  remoteLoading: false,
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
