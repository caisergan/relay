import { create } from 'zustand'

import type { Density, PromptRequest, Theme } from '@/ipc/gen'

export type DrawerTab = 'active' | 'failed' | 'completed'

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
  drawerOpen: boolean
  drawerTab: DrawerTab
  activityOpen: boolean
  settingsOpen: boolean
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
  toggleDrawer: (open?: boolean) => void
  setDrawerTab: (tab: DrawerTab) => void
  toggleActivity: (open?: boolean) => void
  toggleSettings: (open?: boolean) => void
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

export const useUiStore = create<UiState>((set) => ({
  theme: 'system',
  resolved: 'light',
  density: 'comfortable',
  paletteOpen: false,
  drawerOpen: true,
  drawerTab: 'active',
  activityOpen: false,
  settingsOpen: false,
  prompts: {},
  toasts: [],
  connected: false,
  sidebarCollapsed: false,
  localPanePercent: 37,

  setTheme: (theme) => set({ theme }),
  setResolved: (resolved) => set({ resolved }),
  setDensity: (density) => set({ density }),
  togglePalette: (open) => set((s) => ({ paletteOpen: open ?? !s.paletteOpen })),
  toggleDrawer: (open) => set((s) => ({ drawerOpen: open ?? !s.drawerOpen })),
  setDrawerTab: (drawerTab) => set({ drawerTab }),
  toggleActivity: (open) => set((s) => ({ activityOpen: open ?? !s.activityOpen })),
  toggleSettings: (open) => set((s) => ({ settingsOpen: open ?? !s.settingsOpen })),

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
      ],
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

export { MAX_PANE_PERCENT, MIN_PANE_PERCENT }
