import { beforeEach, describe, expect, it } from 'vitest'

import { MAX_TOASTS, useUiStore } from './uiStore'

describe('toasts', () => {
  beforeEach(() => {
    useUiStore.setState({ toasts: [] })
  })

  /// A queue of thousands meeting a server that refuses a channel raised one toast per
  /// failure, until they covered the window — including the drawer that says which
  /// files they were.
  it('keeps the newest few rather than stacking one per failure', () => {
    const { toast } = useUiStore.getState()
    for (let i = 0; i < MAX_TOASTS + 6; i += 1) toast('error', `failure ${i}`)

    const shown = useUiStore.getState().toasts
    expect(shown).toHaveLength(MAX_TOASTS)
    expect(shown.at(-1)?.text, 'the newest is the one still on screen').toBe(
      `failure ${MAX_TOASTS + 5}`,
    )
  })

  it('leaves a handful alone', () => {
    const { toast } = useUiStore.getState()
    toast('ok', 'one')
    toast('ok', 'two')
    expect(useUiStore.getState().toasts.map((t) => t.text)).toEqual(['one', 'two'])
  })
})
