/** The gesture: a press, a threshold, a hit test on every move, a drop or a cancel.
 *
 * jsdom has no layout, so `elementFromPoint` is stubbed to answer from a table of
 * points. That is not a shortcut around the real thing — the real thing is the
 * browser's, and what is ours is everything on either side of that one call. */

import { act, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { PANE_ATTR, ROW_DIR_ATTR, ROW_NAME_ATTR, ZONE_ATTR, type RowDrag } from '@/lib/rowDrag'

import { useRowDrag } from './useRowDrag'

const CARRIED: RowDrag = { from: 'local', entries: [{ name: 'notes.txt', isDir: false }] }

/** Somewhere in the remote listing, on its `backups` folder, on its landing area, and
 * nowhere. Points are arbitrary and only have to be distinct. */
const OVER_LISTING = { clientX: 300, clientY: 100 }
const OVER_FOLDER = { clientX: 300, clientY: 140 }
const OVER_ZONE = { clientX: 300, clientY: 400 }
const NOWHERE = { clientX: 10, clientY: 10 }

function Harness({ onDrop }: { onDrop: (drag: RowDrag, target: unknown) => void }) {
  const { drag, over, ghostRef, begin } = useRowDrag(onDrop)
  return (
    <>
      <div className="rows" {...{ [PANE_ATTR]: 'local' }}>
        <div
          className="row"
          {...{ [ROW_NAME_ATTR]: 'notes.txt', [ROW_DIR_ATTR]: 'false' }}
          onPointerDown={(e) => begin(CARRIED, e)}
        >
          notes.txt
          <button>upload</button>
        </div>
      </div>
      <div className="rows" {...{ [PANE_ATTR]: 'remote' }} data-testid="listing">
        <div
          className="row"
          {...{ [ROW_NAME_ATTR]: 'backups', [ROW_DIR_ATTR]: 'true' }}
          data-testid="folder"
        >
          backups
        </div>
      </div>
      <div className="dropzone" {...{ [ZONE_ATTR]: 'remote' }} data-testid="zone" />
      <output data-testid="state">{JSON.stringify({ drag, over })}</output>
      {drag && <div className="dragghost" ref={ghostRef} data-testid="ghost" />}
    </>
  )
}

const state = () => JSON.parse(screen.getByTestId('state').textContent) as unknown

beforeEach(() => {
  document.elementFromPoint = (x: number, y: number) => {
    const hit = (p: { clientX: number; clientY: number }) => p.clientX === x && p.clientY === y
    if (hit(OVER_LISTING)) return screen.getByTestId('listing')
    if (hit(OVER_FOLDER)) return screen.getByTestId('folder')
    if (hit(OVER_ZONE)) return screen.getByTestId('zone')
    return null
  }
})

afterEach(() => {
  document.body.className = ''
})

/** Press on the source row and move far enough to lift it. */
function lift(from = { clientX: 100, clientY: 100 }) {
  fireEvent.pointerDown(screen.getByText('notes.txt'), { button: 0, ...from })
  fireEvent.pointerMove(window, { clientX: from.clientX + 20, clientY: from.clientY })
}

describe('lifting a row', () => {
  it('is not a drag until the pointer has travelled', () => {
    render(<Harness onDrop={vi.fn()} />)
    fireEvent.pointerDown(screen.getByText('notes.txt'), {
      button: 0,
      clientX: 100,
      clientY: 100,
    })
    fireEvent.pointerMove(window, { clientX: 102, clientY: 101 })
    expect(state()).toEqual({ drag: null, over: null })
    expect(document.body.classList.contains('dragging')).toBe(false)
  })

  it('becomes one past the threshold, and shows a ghost', () => {
    render(<Harness onDrop={vi.fn()} />)
    lift()
    expect(state()).toEqual({ drag: CARRIED, over: null })
    expect(document.body.classList.contains('dragging')).toBe(true)
    expect(screen.getByTestId('ghost').style.transform).toMatch(/^translate\(/)
  })

  it('ignores a press with any button but the primary one', () => {
    render(<Harness onDrop={vi.fn()} />)
    fireEvent.pointerDown(screen.getByText('notes.txt'), {
      button: 2,
      clientX: 100,
      clientY: 100,
    })
    fireEvent.pointerMove(window, { clientX: 200, clientY: 100 })
    expect(state()).toEqual({ drag: null, over: null })
  })

  /** A press on "upload" that wobbles is a click on "upload". */
  it("ignores a press on the row's own buttons", () => {
    render(<Harness onDrop={vi.fn()} />)
    fireEvent.pointerDown(screen.getByText('upload'), { button: 0, clientX: 100, clientY: 100 })
    fireEvent.pointerMove(window, { clientX: 200, clientY: 100 })
    expect(state()).toEqual({ drag: null, over: null })
  })
})

describe('moving over the other pane', () => {
  it('reports the listing, a folder in it, and the landing area as the pointer crosses them', () => {
    render(<Harness onDrop={vi.fn()} />)
    lift()
    fireEvent.pointerMove(window, OVER_LISTING)
    expect(state()).toEqual({
      drag: CARRIED,
      over: { pane: 'remote', folder: null, zone: false },
    })
    fireEvent.pointerMove(window, OVER_FOLDER)
    expect(state()).toEqual({
      drag: CARRIED,
      over: { pane: 'remote', folder: 'backups', zone: false },
    })
    fireEvent.pointerMove(window, OVER_ZONE)
    expect(state()).toEqual({
      drag: CARRIED,
      over: { pane: 'remote', folder: null, zone: true },
    })
    fireEvent.pointerMove(window, NOWHERE)
    expect(state()).toEqual({ drag: CARRIED, over: null })
  })

  it('says through the body class whether a drop would land', () => {
    render(<Harness onDrop={vi.fn()} />)
    lift()
    fireEvent.pointerMove(window, OVER_FOLDER)
    expect(document.body.classList.contains('dragging--ok')).toBe(true)
    fireEvent.pointerMove(window, NOWHERE)
    expect(document.body.classList.contains('dragging--ok')).toBe(false)
  })
})

describe('letting go', () => {
  it('drops on the target under the pointer, then clears everything', () => {
    const onDrop = vi.fn()
    render(<Harness onDrop={onDrop} />)
    lift()
    fireEvent.pointerMove(window, OVER_FOLDER)
    fireEvent.pointerUp(window, OVER_FOLDER)
    expect(onDrop).toHaveBeenCalledWith(CARRIED, {
      pane: 'remote',
      folder: 'backups',
      zone: false,
    })
    expect(state()).toEqual({ drag: null, over: null })
    expect(document.body.className).toBe('')
  })

  /** The target is read at the drop, not remembered from the last move: a pointer
   * that jumped straight to the folder still lands in it. */
  it('hit-tests the release point itself', () => {
    const onDrop = vi.fn()
    render(<Harness onDrop={onDrop} />)
    lift()
    fireEvent.pointerUp(window, OVER_ZONE)
    expect(onDrop).toHaveBeenCalledWith(CARRIED, { pane: 'remote', folder: null, zone: true })
  })

  it('drops nothing when released over nothing', () => {
    const onDrop = vi.fn()
    render(<Harness onDrop={onDrop} />)
    lift()
    fireEvent.pointerUp(window, NOWHERE)
    expect(onDrop).not.toHaveBeenCalled()
    expect(state()).toEqual({ drag: null, over: null })
  })

  it('drops nothing when the press never became a drag', () => {
    const onDrop = vi.fn()
    render(<Harness onDrop={onDrop} />)
    fireEvent.pointerDown(screen.getByText('notes.txt'), {
      button: 0,
      clientX: 100,
      clientY: 100,
    })
    fireEvent.pointerUp(window, { clientX: 100, clientY: 100 })
    expect(onDrop).not.toHaveBeenCalled()
    expect(state()).toEqual({ drag: null, over: null })
  })

  it('is cancelled by Escape', () => {
    const onDrop = vi.fn()
    render(<Harness onDrop={onDrop} />)
    lift()
    fireEvent.pointerMove(window, OVER_FOLDER)
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(state()).toEqual({ drag: null, over: null })
    expect(document.body.className).toBe('')
    fireEvent.pointerUp(window, OVER_FOLDER)
    expect(onDrop).not.toHaveBeenCalled()
  })

  it('is cancelled when the window loses focus', () => {
    const onDrop = vi.fn()
    render(<Harness onDrop={onDrop} />)
    lift()
    fireEvent.blur(window)
    expect(state()).toEqual({ drag: null, over: null })
  })

  /** The drop calls whatever the hook was last rendered with, not the callback from
   * the render the gesture began in. */
  it('lands on the latest callback', () => {
    const first = vi.fn()
    const second = vi.fn()
    const { rerender } = render(<Harness onDrop={first} />)
    lift()
    rerender(<Harness onDrop={second} />)
    fireEvent.pointerUp(window, OVER_LISTING)
    expect(first).not.toHaveBeenCalled()
    expect(second).toHaveBeenCalledOnce()
  })

  it('leaves nothing behind on unmount', () => {
    const { unmount } = render(<Harness onDrop={vi.fn()} />)
    lift()
    act(() => unmount())
    expect(document.body.className).toBe('')
  })
})
