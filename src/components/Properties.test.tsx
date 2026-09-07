/** Where the inspector lands.
 *
 * Only the placement rule is tested, and only as arithmetic. The panel places itself
 * from its own measured size, and jsdom reports every element as zero by zero — a test
 * that rendered it would prove that a zero-by-zero box fits anywhere, which is true and
 * worth nothing. Handing the sizes in is what makes the edges testable at all. */

import { describe, expect, it } from 'vitest'

import { placePanel } from './Properties'

const VIEWPORT = { width: 1000, height: 800 }
const PANEL = { width: 300, height: 200 }

describe('placePanel', () => {
  /** Where a menu goes, and so where the eye already is. */
  it('sits just below and right of the pointer when there is room', () => {
    expect(placePanel({ x: 100, y: 100 }, PANEL, VIEWPORT)).toEqual({ x: 108, y: 108 })
  })

  /** Flipping rather than sliding keeps a corner at the pointer. A panel that slid to
   * the window edge while the pointer stayed behind has lost what it refers to. */
  it('flips to the left of the pointer rather than running off the right edge', () => {
    expect(placePanel({ x: 950, y: 100 }, PANEL, VIEWPORT)).toEqual({ x: 642, y: 108 })
  })

  it('flips above the pointer rather than running off the bottom', () => {
    expect(placePanel({ x: 100, y: 750 }, PANEL, VIEWPORT)).toEqual({ x: 108, y: 542 })
  })

  it('flips both ways in the far corner', () => {
    expect(placePanel({ x: 950, y: 750 }, PANEL, VIEWPORT)).toEqual({ x: 642, y: 542 })
  })

  /** The last row of a full-height listing is right against the bottom edge, and a
   * right-click there must not put the panel half off screen. */
  it('keeps the panel inside the window when the pointer is in the very corner', () => {
    const at = { x: VIEWPORT.width, y: VIEWPORT.height }
    const { x, y } = placePanel(at, PANEL, VIEWPORT)
    expect(x).toBeGreaterThanOrEqual(10)
    expect(y).toBeGreaterThanOrEqual(10)
    expect(x + PANEL.width).toBeLessThanOrEqual(VIEWPORT.width - 10)
    expect(y + PANEL.height).toBeLessThanOrEqual(VIEWPORT.height - 10)
  })

  /** Flipping cannot solve a panel taller than the window. Starting at the top edge and
   * letting it run is the only answer that keeps the beginning of it readable — the
   * alternative clamps to a negative coordinate and cuts off the name. */
  it('starts at the near edge when the panel is larger than the window', () => {
    const tall = { width: 300, height: 900 }
    expect(placePanel({ x: 100, y: 400 }, tall, VIEWPORT)).toEqual({ x: 108, y: 10 })
  })

  it('never places the panel outside the margin, wherever the pointer is', () => {
    for (const x of [0, 1, 500, 999, 1000]) {
      for (const y of [0, 1, 400, 799, 800]) {
        const spot = placePanel({ x, y }, PANEL, VIEWPORT)
        expect(spot.x, `x at ${x},${y}`).toBeGreaterThanOrEqual(10)
        expect(spot.y, `y at ${x},${y}`).toBeGreaterThanOrEqual(10)
      }
    }
  })
})
