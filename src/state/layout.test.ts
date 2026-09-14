import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { InterfaceLayout } from '@/ipc/gen'

import { recordLayout, restoreLayout } from './layout'
import { MAX_PANE_PERCENT, useUiStore } from './uiStore'

const layoutGet = vi.hoisted(() =>
  vi.fn((): Promise<InterfaceLayout | null> => Promise.resolve({})),
)
const layoutSet = vi.hoisted(() => vi.fn((_layout: InterfaceLayout) => Promise.resolve(null)))

vi.mock('@/ipc/commands', () => ({ commands: { layoutGet, layoutSet } }))

beforeEach(() => {
  layoutGet.mockClear()
  layoutSet.mockClear()
  useUiStore.setState({
    localPanePercent: 37,
    drawerOpen: true,
    drawerTab: 'active',
    sidebarCollapsed: false,
  })
})

afterEach(() => {
  vi.useRealTimers()
})

describe('restoreLayout', () => {
  it('puts back the split, the drawer and the sidebar as they were left', async () => {
    layoutGet.mockResolvedValueOnce({
      localPanePercent: 52,
      drawerOpen: false,
      drawerTab: 'completed',
      sidebarCollapsed: true,
    })

    await restoreLayout()

    expect(useUiStore.getState()).toMatchObject({
      localPanePercent: 52,
      drawerOpen: false,
      drawerTab: 'completed',
      sidebarCollapsed: true,
    })
  })

  // A file from before a field existed, or one somebody edited.
  it('keeps the default for anything the file does not have, and clamps the split', async () => {
    layoutGet.mockResolvedValueOnce({ localPanePercent: 99 })

    await restoreLayout()

    expect(useUiStore.getState()).toMatchObject({
      localPanePercent: MAX_PANE_PERCENT,
      drawerOpen: true,
      sidebarCollapsed: false,
    })
  })
})

describe('recordLayout', () => {
  it('records a change once it settles, not every step of it', () => {
    vi.useFakeTimers()
    const stop = recordLayout()

    for (const percent of [40, 45, 50, 55]) {
      useUiStore.getState().setLocalPanePercent(percent)
    }
    expect(layoutSet).not.toHaveBeenCalled()

    vi.advanceTimersByTime(1000)
    expect(layoutSet).toHaveBeenCalledOnce()
    expect(layoutSet.mock.calls[0]?.[0]).toMatchObject({ localPanePercent: 55 })
    stop()
  })

  // Starting to record is not a change: writing the launch's defaults back would be
  // exactly the overwrite the ordering in `App` exists to prevent.
  it('writes nothing when nothing has changed', () => {
    vi.useFakeTimers()
    const stop = recordLayout()

    useUiStore.getState().toggleDrawer(true)
    vi.advanceTimersByTime(1000)

    expect(layoutSet).not.toHaveBeenCalled()
    stop()
  })

  it('keeps a change still settling when recording stops', () => {
    vi.useFakeTimers()
    const stop = recordLayout()

    useUiStore.getState().toggleSidebar(true)
    stop()

    expect(layoutSet).toHaveBeenCalledOnce()
    expect(layoutSet.mock.calls[0]?.[0]).toMatchObject({ sidebarCollapsed: true })
  })
})
