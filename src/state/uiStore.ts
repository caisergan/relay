import { create } from 'zustand'

import type { Density, PromptRequest, Theme } from '@/ipc/gen'

export type DrawerTab = 'active' | 'failed' | 'completed'

/** A path to go to, sent from outside the pane that goes there — the command palette. */
export interface GoRequest {
  sessionId: string
  side: 'local' | 'remote'
  text: string
}

/** A server file to open in an application on this computer; see `state/previews`. */
export interface PreviewRequest {
  session: string
  serverId: string
  /** Where the file is on the server. */
  remotePath: string
  name: string
}

export interface Toast {
  id: string
  kind: 'info' | 'ok' | 'error'
  text: string
  /** One thing the toast lets you do about what it just said. A transfer that failed
   * is worth retrying from where you are told about it, rather than by opening the
   * drawer and finding the row again. */
  action?: { label: string; run: () => void }
}

interface UiState {
  theme: Theme
  /** `theme` with `system` already resolved against the OS. What the icons read. */
  resolved: 'light' | 'dark'
  density: Density
  paletteOpen: boolean
  /** Waiting for the session's view to take it; see `GoRequest`. */
  pendingGo: GoRequest | null
  drawerOpen: boolean
  drawerTab: DrawerTab
  activityOpen: boolean
  settingsOpen: boolean
  /** A file waiting for someone to say which application it opens in. */
  previewAsk: PreviewRequest | null
  /** Keyed by prompt id so a remount re-renders the same sheet rather than a new one. */
  prompts: Record<string, PromptRequest>
  toasts: Toast[]
  /** False while the engine stream is down; the shell shows a reconnecting strip. */
  connected: boolean
  /** The design's `sidebarCollapsed`: the server list folds away to a floating rail. */
  sidebarCollapsed: boolean
  /** The local pane's share of the pane area, as a percentage. The design fixes this
   * at 37%, but a fixed split cannot suit both a laptop and a wide display, so it is
   * a starting value rather than a constant. */
  localPanePercent: number

  setTheme: (theme: Theme) => void
  setResolved: (resolved: 'light' | 'dark') => void
  setDensity: (density: Density) => void
  togglePalette: (open?: boolean) => void
  requestGo: (request: GoRequest) => void
  clearGo: () => void
  toggleDrawer: (open?: boolean) => void
  setDrawerTab: (tab: DrawerTab) => void
  toggleActivity: (open?: boolean) => void
  toggleSettings: (open?: boolean) => void
  askPreview: (request: PreviewRequest | null) => void
  setPrompts: (prompts: PromptRequest[]) => void
  openPrompt: (prompt: PromptRequest) => void
  closePrompt: (id: string) => void
  toast: (kind: Toast['kind'], text: string, action?: Toast['action']) => void
  dismissToast: (id: string) => void
  setConnected: (connected: boolean) => void
  toggleSidebar: (collapsed?: boolean) => void
  setLocalPanePercent: (percent: number) => void
}

/** Neither pane may be dragged away entirely: a pane too narrow to show a filename is
 * not a smaller pane, it is a broken one, and there would be no handle left to drag
 * back. Collapsing is what the sidebar's toggle is for. */
const MIN_PANE_PERCENT = 18
const MAX_PANE_PERCENT = 78

/** How many toasts may be on screen at once.
 *
 * Without a cap, a burst — a queue of thousands meeting a server that refuses a
 * channel — stacks one per failure until they cover the window, including the pane the
 * person needs to see what went wrong. The oldest give way to the newest, and nothing
 * is lost by it: every failure is a row in the drawer's Failed tab, with the same
 * Retry button on it. */
const MAX_TOASTS = 4

export const useUiStore = create<UiState>((set) => ({
  theme: 'system',
  resolved: 'light',
  density: 'comfortable',
  paletteOpen: false,
  pendingGo: null,
  drawerOpen: true,
  drawerTab: 'active',
  activityOpen: false,
  settingsOpen: false,
  previewAsk: null,
  prompts: {},
  toasts: [],
  connected: false,
  sidebarCollapsed: false,
  localPanePercent: 37,

  setTheme: (theme) => set({ theme }),
  setResolved: (resolved) => set({ resolved }),
  setDensity: (density) => set({ density }),
  togglePalette: (open) => set((s) => ({ paletteOpen: open ?? !s.paletteOpen })),
  requestGo: (pendingGo) => set({ pendingGo }),
  clearGo: () => set({ pendingGo: null }),
  toggleDrawer: (open) => set((s) => ({ drawerOpen: open ?? !s.drawerOpen })),
  setDrawerTab: (drawerTab) => set({ drawerTab }),
  toggleActivity: (open) => set((s) => ({ activityOpen: open ?? !s.activityOpen })),
  toggleSettings: (open) => set((s) => ({ settingsOpen: open ?? !s.settingsOpen })),
  askPreview: (previewAsk) => set({ previewAsk }),

  setPrompts: (prompts) => set({ prompts: Object.fromEntries(prompts.map((p) => [p.id, p])) }),
  openPrompt: (prompt) => set((s) => ({ prompts: { ...s.prompts, [prompt.id]: prompt } })),
  closePrompt: (id) =>
    set((s) => {
      const { [id]: _removed, ...rest } = s.prompts
      return { prompts: rest }
    }),

  // The key is omitted rather than set to `undefined`: `exactOptionalPropertyTypes`
  // draws a distinction between "absent" and "present and undefined", and this is the
  // former.
  toast: (kind, text, action) =>
    set((s) => ({
      toasts: [
        ...s.toasts,
        { id: crypto.randomUUID(), kind, text, ...(action ? { action } : {}) },
      ].slice(-MAX_TOASTS),
    })),
  dismissToast: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
  setConnected: (connected) => set({ connected }),
  toggleSidebar: (collapsed) =>
    set((s) => ({ sidebarCollapsed: collapsed ?? !s.sidebarCollapsed })),
  setLocalPanePercent: (percent) =>
    set({
      localPanePercent: Math.min(Math.max(percent, MIN_PANE_PERCENT), MAX_PANE_PERCENT),
    }),
}))

export { MAX_PANE_PERCENT, MAX_TOASTS, MIN_PANE_PERCENT }
