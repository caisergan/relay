/** The draggable divider between the local and remote panes.
 *
 * The design fixes the local pane at 37% — "a shelf you pick from" against "the thing
 * you are working in". That ratio is right for the design's window and wrong for a lot
 * of real ones: a deep local tree or a remote pane carrying the extra Perms column both
 * want a different share, and neither could be given one.
 *
 * Not in the design, so it is built from what the design already has: the same 1px
 * `--line` the panes were divided by, widened into a grab strip and tinted `--signal`
 * while held.
 *
 * Pointer events rather than mouse events, so a trackpad, a stylus and a touch screen
 * all drive it, and `setPointerCapture` keeps the drag alive when the cursor outruns
 * the 5px strip — which it always does. */

import { useRef } from 'react'

import { useUiStore } from '@/state/uiStore'

interface Props {
  /** Measured to convert a pointer position into a percentage of the pane area. */
  areaRef: React.RefObject<HTMLDivElement | null>
}

/** How far one arrow key nudges the split. Coarse enough to be worth pressing. */
const KEY_STEP = 2

export function PaneSplitter({ areaRef }: Props) {
  const percent = useUiStore((s) => s.localPanePercent)
  const setPercent = useUiStore((s) => s.setLocalPanePercent)
  const dragging = useRef(false)

  const fromPointer = (clientX: number) => {
    const rect = areaRef.current?.getBoundingClientRect()
    if (!rect || rect.width === 0) return
    setPercent(((clientX - rect.left) / rect.width) * 100)
  }

  return (
    <div
      className="splitter"
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize the panes"
      aria-valuenow={Math.round(percent)}
      aria-valuemin={0}
      aria-valuemax={100}
      tabIndex={0}
      onPointerDown={(event) => {
        // Only the primary button drags; a right-click here should do nothing.
        if (event.button !== 0) return
        dragging.current = true
        event.currentTarget.setPointerCapture(event.pointerId)
        event.preventDefault()
      }}
      onPointerMove={(event) => {
        if (!dragging.current) return
        fromPointer(event.clientX)
      }}
      onPointerUp={(event) => {
        dragging.current = false
        event.currentTarget.releasePointerCapture(event.pointerId)
      }}
      onPointerCancel={() => {
        dragging.current = false
      }}
      // Double-click restores the design's ratio, which is the only way back to it
      // once it has been dragged away.
      onDoubleClick={() => setPercent(37)}
      onKeyDown={(event) => {
        if (event.key === 'ArrowLeft') {
          event.preventDefault()
          setPercent(percent - KEY_STEP)
        }
        if (event.key === 'ArrowRight') {
          event.preventDefault()
          setPercent(percent + KEY_STEP)
        }
      }}
    >
      <div className="splitter__line" aria-hidden />
    </div>
  )
}
