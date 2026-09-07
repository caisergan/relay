import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'

import { resolveTarget, sameTarget, type DropTarget, type RowDrag } from '@/lib/rowDrag'

/** How far the pointer has to travel before a press becomes a drag. Under this, it is
 * a click, and clicks have to keep selecting and double-clicks keep opening. */
const THRESHOLD = 4

/** The ghost sits down and to the right of the pointer, clear of the hit test and of
 * the cursor itself. */
const GHOST_OFFSET = 14

/** Classes on `<body>` while a drag is in flight, so the cursor can say whether a drop
 * would land. A captured pointer does not take its cursor from the element it is
 * over, so the rule has to be global for the whole gesture. */
const DRAGGING = 'dragging'
const DRAGGING_OK = 'dragging--ok'

export interface RowDragHandle {
  /** What is being dragged, or `null` between gestures. */
  drag: RowDrag | null
  /** Where a drop would land right now, or `null` when it would land nowhere. */
  over: DropTarget | null
  /** Attach to the ghost element. The hook moves it directly on every pointer event
   * rather than re-rendering two virtualised listings sixty times a second. */
  ghostRef: React.RefObject<HTMLDivElement | null>
  /** A row's `pointerdown`. Arms a drag; nothing shows until the pointer moves. */
  begin: (drag: RowDrag, e: React.PointerEvent) => void
}

/** Drive a drag between panes from pointer events. See `lib/rowDrag.ts` for why it is
 * not the HTML drag-and-drop model. */
export function useRowDrag(onDrop: (drag: RowDrag, target: DropTarget) => void): RowDragHandle {
  const [drag, setDrag] = useState<RowDrag | null>(null)
  const [over, setOver] = useState<DropTarget | null>(null)

  /** A press that has not yet travelled far enough to be a drag. */
  const pending = useRef<{ drag: RowDrag; x: number; y: number } | null>(null)
  /** The drag in flight, readable from a listener without a stale closure. */
  const active = useRef<RowDrag | null>(null)
  const overRef = useRef<DropTarget | null>(null)
  const point = useRef({ x: 0, y: 0 })
  const ghostRef = useRef<HTMLDivElement | null>(null)
  const detach = useRef<(() => void) | null>(null)

  // Always the latest callback, so the drop lands on the current listing and paths
  // rather than whatever the hook was first rendered with.
  const dropRef = useRef(onDrop)
  useEffect(() => {
    dropRef.current = onDrop
  }, [onDrop])

  const placeGhost = () => {
    const el = ghostRef.current
    if (!el) return
    const { x, y } = point.current
    el.style.transform = `translate(${x + GHOST_OFFSET}px, ${y + GHOST_OFFSET}px)`
  }

  // The ghost mounts only once `drag` is set, which is after the first qualifying
  // move — so it has to be placed on mount, not only on the moves that follow.
  useLayoutEffect(() => {
    if (drag) placeGhost()
  }, [drag])

  const clearSelection = () => {
    const selection = window.getSelection()
    if (selection && !selection.isCollapsed) selection.removeAllRanges()
  }

  const setTarget = (target: DropTarget | null) => {
    if (sameTarget(overRef.current, target)) return
    overRef.current = target
    setOver(target)
    document.body.classList.toggle(DRAGGING_OK, target !== null)
  }

  const finish = () => {
    clearSelection()
    pending.current = null
    active.current = null
    overRef.current = null
    setDrag(null)
    setOver(null)
    document.body.classList.remove(DRAGGING, DRAGGING_OK)
    detach.current?.()
    detach.current = null
  }

  const targetAt = (x: number, y: number): DropTarget | null => {
    const from = active.current?.from
    if (!from) return null
    return resolveTarget(document.elementFromPoint(x, y), from)
  }

  const onMove = (e: PointerEvent) => {
    point.current = { x: e.clientX, y: e.clientY }
    const press = pending.current
    if (press) {
      if (Math.hypot(e.clientX - press.x, e.clientY - press.y) < THRESHOLD) return
      pending.current = null
      active.current = press.drag
      document.body.classList.add(DRAGGING)
      setDrag(press.drag)
    }
    if (!active.current) return
    // A press is also the start of a text selection, and WebKit keeps growing it as
    // the pointer travels, whatever `user-select` says once the gesture is under way.
    // Cleared on every move rather than once, because it comes back on every move.
    clearSelection()
    placeGhost()
    setTarget(targetAt(e.clientX, e.clientY))
  }

  const onUp = (e: PointerEvent) => {
    const carried = active.current
    const target = carried ? targetAt(e.clientX, e.clientY) : null
    finish()
    if (carried && target) dropRef.current(carried, target)
  }

  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Escape') finish()
  }

  const begin = useCallback((next: RowDrag, e: React.PointerEvent) => {
    // The primary button only, and not from the row's own action buttons: a press on
    // "upload" that wobbles a few pixels is a click on "upload", not a drag.
    if (e.button !== 0) return
    if (e.target instanceof Element && e.target.closest('button')) return
    detach.current?.()
    pending.current = { drag: next, x: e.clientX, y: e.clientY }
    point.current = { x: e.clientX, y: e.clientY }
    const cancel = () => finish()
    window.addEventListener('pointermove', onMove)
    window.addEventListener('pointerup', onUp)
    window.addEventListener('pointercancel', cancel)
    window.addEventListener('blur', cancel)
    window.addEventListener('keydown', onKey)
    detach.current = () => {
      window.removeEventListener('pointermove', onMove)
      window.removeEventListener('pointerup', onUp)
      window.removeEventListener('pointercancel', cancel)
      window.removeEventListener('blur', cancel)
      window.removeEventListener('keydown', onKey)
    }
    // Listeners are the only closures here and they read refs, so the hook's own
    // setters are the only thing this depends on, and those are stable.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // An unmount mid-gesture must not leave listeners on the window or a class on the
  // body. Only those two: state on an unmounted component needs no clearing, and
  // keeping `finish` out of it keeps the effect free of anything that changes.
  useEffect(() => {
    const listeners = detach
    return () => {
      listeners.current?.()
      listeners.current = null
      document.body.classList.remove(DRAGGING, DRAGGING_OK)
    }
  }, [])

  return { drag, over, ghostRef, begin }
}
