import { Fragment, useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'

import type { DirSize } from '@/ipc/gen'
import { withMeasurement, type Measurement, type Properties as Facts } from '@/lib/properties'

import { RowIcon } from './FileList'

export interface Point {
  x: number
  y: number
}

/** Walks a folder and totals it up. Handed in rather than called from here, because
 * which side of the transfer the folder is on decides which command does the walking,
 * and the panel does not know or need to. */
export type Measure = () => Promise<DirSize>

interface Box {
  width: number
  height: number
}

/** How far the panel sits from the pointer, and how close it may come to the window's
 * edge. Small enough that the panel reads as attached to what was clicked. */
const OFFSET = 8
const MARGIN = 10

/** Where the panel goes, given where the pointer was and how big the panel turned out.
 *
 * Exported so the rule can be tested directly. The panel itself cannot be: it is
 * placed from its own measured size, and jsdom reports every element as zero by zero,
 * so a test of the component would only ever prove that zero fits anywhere.
 *
 * Below and to the right of the pointer, which is where a menu goes and so where the
 * eye already is. It flips rather than slides when that would run off an edge — a
 * panel pinned to the right edge of the window while the pointer is elsewhere has lost
 * the connection to what was clicked, and flipping keeps a corner at the pointer. The
 * final clamp is for the case flipping cannot solve, a panel taller than the window,
 * where the only useful answer is to start at the top and let it run. */
export function placePanel(at: Point, panel: Box, viewport: Box): Point {
  let x = at.x + OFFSET
  if (x + panel.width > viewport.width - MARGIN) x = at.x - OFFSET - panel.width
  let y = at.y + OFFSET
  if (y + panel.height > viewport.height - MARGIN) y = at.y - OFFSET - panel.height

  // `Math.max` on the upper bound too, so a panel larger than the window clamps to the
  // near edge instead of to a negative coordinate off the opposite one.
  const limit = (value: number, extent: number, size: number) =>
    Math.min(Math.max(MARGIN, value), Math.max(MARGIN, extent - MARGIN - size))
  return {
    x: limit(x, viewport.width, panel.width),
    y: limit(y, viewport.height, panel.height),
  }
}

/** The inspector: everything Relay knows about one entry, next to the pointer that
 * asked.
 *
 * A floating panel rather than a sheet, because a sheet is a question — it dims the
 * window and waits for an answer — and this is a glance. It closes on the next thing
 * you do: a click anywhere, Escape, a scroll of the listing behind it, or the window
 * losing focus.
 *
 * Portalled to the body so no pane's `overflow` can clip it and no ancestor's
 * `transform` can turn its `fixed` into "relative to that ancestor" — the same reason
 * the drag ghost lives there. */
export function Properties({
  facts,
  at,
  measure,
  onClose,
}: {
  facts: Facts
  at: Point
  measure: Measure | null
  onClose: () => void
}) {
  const ref = useRef<HTMLDivElement>(null)
  const [pos, setPos] = useState<Point | null>(null)
  // Initialised rather than assigned from the effect: every opening is a fresh panel
  // (`SessionView` keys it), so the first render already knows whether a walk is
  // coming, and the alternative — setting state inside the effect — renders the folder
  // once with a stale size before correcting itself.
  const [measurement, setMeasurement] = useState<Measurement | null>(
    measure ? { state: 'measuring' } : null,
  )

  /** A folder is walked as soon as the panel opens, because a size you have to ask for
   * twice is not much of an inspector — but the walk is one round trip per directory,
   * so the panel opens saying so and fills the number in when it has one. Closing the
   * panel unmounts this and `live` drops the answer on the floor. */
  useEffect(() => {
    if (!measure) return
    let live = true
    void measure().then(
      (size) => {
        if (live) setMeasurement({ state: 'done', size })
      },
      () => {
        if (live) setMeasurement({ state: 'failed' })
      },
    )
    return () => {
      live = false
    }
  }, [measure])

  // Before paint, not after: the panel is rendered once at the raw pointer position to
  // be measured, and moved to its real one in the same frame. In an effect this would
  // be a visible jump.
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const box = el.getBoundingClientRect()
    setPos(
      placePanel(
        at,
        { width: box.width, height: box.height },
        { width: window.innerWidth, height: window.innerHeight },
      ),
    )
    // `measurement` too: the panel grows by a row when a folder's total lands, and a
    // panel that was already flipped above the pointer would otherwise grow off the
    // top of the window.
  }, [at, facts, measurement])

  useEffect(() => {
    const away = (e: PointerEvent) => {
      if (e.target instanceof Node && ref.current?.contains(e.target)) return
      onClose()
    }
    const key = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    // `pointerdown` in the capture phase, so a right-click on another row closes this
    // panel before that row's `contextmenu` opens the next one. Both land in the same
    // tick and React keeps the later state, which is the new panel.
    window.addEventListener('pointerdown', away, true)
    window.addEventListener('keydown', key)
    window.addEventListener('blur', onClose)
    // Also capture: the listing is a nested scroller and `scroll` does not bubble, so
    // a panel anchored to a row would otherwise stay put while the row slid away.
    window.addEventListener('scroll', onClose, true)
    window.addEventListener('resize', onClose)
    return () => {
      window.removeEventListener('pointerdown', away, true)
      window.removeEventListener('keydown', key)
      window.removeEventListener('blur', onClose)
      window.removeEventListener('scroll', onClose, true)
      window.removeEventListener('resize', onClose)
    }
  }, [onClose])

  return createPortal(
    <div
      className="props"
      ref={ref}
      role="dialog"
      aria-label={`Properties of ${facts.name}`}
      style={{ left: pos?.x ?? at.x, top: pos?.y ?? at.y }}
      // A second right-click inside the panel would otherwise reach the engine's own
      // menu, which is the thing this feature exists to replace.
      onContextMenu={(e) => e.preventDefault()}
    >
      <div className="props__head">
        <span className="props__icon">
          <RowIcon name={facts.name} isDir={facts.isDir} />
        </span>
        <div className="props__ident">
          <div className="props__name" title={facts.name}>
            {facts.name}
          </div>
          <div className="props__kind">{facts.kind}</div>
        </div>
      </div>
      <dl className="props__facts">
        {withMeasurement(facts.facts, measurement).map((fact) => (
          <Fragment key={fact.label}>
            <dt>{fact.label}</dt>
            <dd
              className={`props__val${fact.mono ? ' props__val--mono' : ''}${
                fact.unknown ? ' props__val--unknown' : ''
              }`}
            >
              {fact.value}
            </dd>
          </Fragment>
        ))}
      </dl>
    </div>,
    document.body,
  )
}

/** The panel's state, and the two things that open and close it.
 *
 * Here rather than in `SessionView` because the close callback has to keep its
 * identity — the panel resubscribes its window listeners whenever it changes — and
 * because "a right-click opens it, anything else shuts it" is the whole feature. */
export function useProperties() {
  const [open, setOpen] = useState<{
    /** Rises with every opening, so the panel can be keyed on it. Two right-clicks in
     * a row land in one React batch — the first closes, the second opens — so the
     * component is never unmounted between them, and without this it would be reused
     * and would keep the previous row's measurement. */
    seq: number
    facts: Facts
    at: Point
    measure: Measure | null
  } | null>(null)
  const close = useCallback(() => setOpen(null), [])
  // `measure` is stored rather than rebuilt on render, so the panel's effect sees one
  // stable function per opening and walks the folder once.
  const show = useCallback(
    (facts: Facts, at: Point, measure: Measure | null = null) =>
      setOpen((was) => ({ seq: (was?.seq ?? 0) + 1, facts, at, measure })),
    [],
  )
  return { open, show, close }
}
