import { create } from 'zustand'

import type { Density, PromptRequest, Theme } from '@/ipc/gen'

export type DrawerTab = 'active' | 'failed' | 'completed'

export interface Toast {
  id: string
  kind: 'info' | 'ok' | 'error'
  text: string
}

interface UiState {
  theme: Theme
  density: Density
  paletteOpen: boolean
  drawerOpen: boolean
  drawerTab: DrawerTab
  activityOpen: boolean
  /** Keyed by prompt id so a remount re-renders the same sheet rather than a new one. */
  prompts: Record<string, PromptRequest>
  toasts: Toast[]
  /** False while the engine stream is down; the shell shows a reconnecting strip. */
  connected: boolean

  setTheme: (theme: Theme) => void
  setDensity: (density: Density) => void
  togglePalette: (open?: boolean) => void
  toggleDrawer: (open?: boolean) => void
  setDrawerTab: (tab: DrawerTab) => void
  toggleActivity: (open?: boolean) => void
  setPrompts: (prompts: PromptRequest[]) => void
  openPrompt: (prompt: PromptRequest) => void
  closePrompt: (id: string) => void
  toast: (kind: Toast['kind'], text: string) => void
  dismissToast: (id: string) => void
  setConnected: (connected: boolean) => void
}

export const useUiStore = create<UiState>((set) => ({
  theme: 'system',
  density: 'comfortable',
  paletteOpen: false,
  drawerOpen: true,
  drawerTab: 'active',
  activityOpen: false,
  prompts: {},
  toasts: [],
  connected: false,

  setTheme: (theme) => set({ theme }),
  setDensity: (density) => set({ density }),
  togglePalette: (open) => set((s) => ({ paletteOpen: open ?? !s.paletteOpen })),
  toggleDrawer: (open) => set((s) => ({ drawerOpen: open ?? !s.drawerOpen })),
  setDrawerTab: (drawerTab) => set({ drawerTab }),
  toggleActivity: (open) => set((s) => ({ activityOpen: open ?? !s.activityOpen })),

  setPrompts: (prompts) => set({ prompts: Object.fromEntries(prompts.map((p) => [p.id, p])) }),
  openPrompt: (prompt) => set((s) => ({ prompts: { ...s.prompts, [prompt.id]: prompt } })),
  closePrompt: (id) =>
    set((s) => {
      const { [id]: _removed, ...rest } = s.prompts
      return { prompts: rest }
    }),

  toast: (kind, text) =>
    set((s) => ({ toasts: [...s.toasts, { id: crypto.randomUUID(), kind, text }] })),
  dismissToast: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
  setConnected: (connected) => set({ connected }),
}))
