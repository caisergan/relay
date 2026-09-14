/** How the interface was arranged — the split between the panes, the transfers drawer,
 * the sidebar — put back on launch and recorded as it changes. The window's own size and
 * place are the shell's to keep; see `relay_core::layout`. */

import { commands } from '@/ipc/commands'
import type { InterfaceLayout } from '@/ipc/gen'

import { useUiStore } from './uiStore'

type UiState = ReturnType<typeof useUiStore.getState>

/** How long the arrangement has to hold still before it is recorded. Dragging the
 * splitter changes the split on every frame, and one write at the end says the same. */
const SETTLE_MS = 400

export function currentLayout(state: UiState): InterfaceLayout {
  return {
    // A tenth of a percent is finer than a pixel on any display this runs on.
    localPanePercent: Math.round(state.localPanePercent * 10) / 10,
    drawerOpen: state.drawerOpen,
    drawerTab: state.drawerTab,
    sidebarCollapsed: state.sidebarCollapsed,
  }
}

/** Put back what was recorded. A field the file does not have keeps its default, and the
 * split goes through the same clamp a drag does, so no file can hide a pane. */
export async function restoreLayout(): Promise<void> {
  const saved = await commands.layoutGet().catch(() => null)
  if (!saved) return
  const ui = useUiStore.getState()
  if (typeof saved.localPanePercent === 'number') ui.setLocalPanePercent(saved.localPanePercent)
  if (typeof saved.drawerOpen === 'boolean') ui.toggleDrawer(saved.drawerOpen)
  if (typeof saved.drawerTab === 'string') ui.setDrawerTab(saved.drawerTab)
  if (typeof saved.sidebarCollapsed === 'boolean') ui.toggleSidebar(saved.sidebarCollapsed)
}

/** Keep the recorded layout current, until the returned function is called.
 *
 * Started after `restoreLayout`, so the defaults every launch begins with are never
 * written over the layout about to be put back. */
export function recordLayout(): () => void {
  let last = JSON.stringify(currentLayout(useUiStore.getState()))
  let timer: ReturnType<typeof setTimeout> | null = null

  const send = () => {
    timer = null
    const layout = currentLayout(useUiStore.getState())
    const json = JSON.stringify(layout)
    if (json === last) return
    last = json
    commands.layoutSet(layout).catch(() => undefined)
  }

  const unsubscribe = useUiStore.subscribe((state) => {
    if (JSON.stringify(currentLayout(state)) === last) return
    if (timer !== null) clearTimeout(timer)
    timer = setTimeout(send, SETTLE_MS)
  })

  return () => {
    unsubscribe()
    // Whatever was waiting to settle is still the arrangement to keep.
    if (timer !== null) {
      clearTimeout(timer)
      send()
    }
  }
}
