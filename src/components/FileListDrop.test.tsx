/** What a pane shows while a drag is in flight, and what it puts in the DOM for the
 * hit test to find.
 *
 * The gesture itself lives in `useRowDrag` and is tested there; the hit test in
 * `lib/rowDrag`. What is left for the pane is presentational — a landing area that
 * appears in the right pane at the right moment, a folder that lights up, a listing
 * that marks itself as a target — and the markers the other two rely on.
 *
 * The zone tests go through the empty-listing path on purpose. A populated listing is
 * virtualised, and @tanstack/react-virtual measures its scroll container, which jsdom
 * reports as zero pixels tall — so no row is ever mounted and nothing about a row can
 * be asserted without the stubbing below. The empty directory is not a workaround for
 * that: it is the case most worth covering, because a folder with nothing in it is a
 * perfectly good destination and the one where a landing area is the only thing on
 * screen saying so.
 *
 * Named for the drop rather than for the component because `FileList.test.ts` already
 * exists beside it, covering sorting and permissions. Two files whose names differ only
 * by the `x` both resolve to the module `./FileList.test`, and TypeScript's project
 * service answers with one of them and reports the other as missing — a parse error
 * from ESLint and nothing from `tsc`. */

import { fireEvent, render, screen } from '@testing-library/react'
import { beforeAll, describe, expect, it, vi } from 'vitest'

import { PANE_ATTR, ROW_DIR_ATTR, ROW_NAME_ATTR, ZONE_ATTR, type RowDrag } from '@/lib/rowDrag'

import { FileList, type FileRow } from './FileList'

const NONE: FileRow[] = []

/** Give the virtualiser a viewport, so rows actually mount.
 *
 * jsdom reports every element as zero by zero, and @tanstack/react-virtual computes
 * its window from the scroll element's size — so with real dimensions it decides no
 * row is visible and mounts none of them. */
beforeAll(() => {
  // Assigned rather than defaulted: the DOM types say `ResizeObserver` always exists,
  // so `??=` reads as dead code to the linter, and jsdom does not actually ship one.
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  for (const [prop, value] of [
    ['clientHeight', 800],
    ['clientWidth', 600],
    ['offsetHeight', 800],
    ['offsetWidth', 600],
  ] as const) {
    Object.defineProperty(HTMLElement.prototype, prop, { configurable: true, value })
  }
})

function entry(name: string, isDir: boolean): FileRow {
  return { key: name, name, isDir, hidden: false, size: 0, modified: null, perms: null }
}

const LISTING: FileRow[] = [entry('backups', true), entry('a.bin', false)]

/** A drag that started in the remote pane, carrying one file. */
const FROM_REMOTE: RowDrag = { from: 'remote', entries: [{ name: 'x.txt', isDir: false }] }
const FROM_LOCAL: RowDrag = { from: 'local', entries: [{ name: 'x.txt', isDir: false }] }

/** A row element, by the name it shows. */
function rowFor(name: string): HTMLElement {
  const el = screen.getByText(name).closest('.row')
  if (!(el instanceof HTMLElement)) throw new Error(`no row rendered for "${name}"`)
  return el
}

/** `receives` is a separate flag rather than `canReceive: false` in `props` so the
 * default case is exercised as the default: a pane that was never told it can receive
 * must behave exactly like one told it cannot. */
function paint(props: Partial<Parameters<typeof FileList>[0]> = {}, receives = true) {
  return render(
    <FileList
      pane="local"
      rows={NONE}
      loading={false}
      direction="up"
      sort={{ key: 'name', dir: 1 }}
      onSort={vi.fn()}
      selected={null}
      onSelect={vi.fn()}
      onOpen={vi.fn()}
      onAction={vi.fn()}
      dropLabel="/Users/ada/Documents"
      {...(receives ? { canReceive: true } : {})}
      {...props}
    />,
  )
}

const zone = () => screen.queryByText(/Drop in/)

/** The drop zone element itself, rather than the label inside it. */
function target(): HTMLElement {
  const el = zone()?.closest('.dropzone')
  if (!(el instanceof HTMLElement)) throw new Error('the drop zone was not rendered')
  return el
}

const listing = (container: HTMLElement): HTMLElement => {
  const el = container.querySelector('.rows')
  if (!(el instanceof HTMLElement)) throw new Error('no listing rendered')
  return el
}

describe('the drop zone', () => {
  it('stays hidden while nothing is being dragged', () => {
    paint({ drag: null })
    expect(zone()).toBeNull()
  })

  it('appears the moment a drag starts in the other pane', () => {
    paint({ drag: FROM_REMOTE })
    expect(zone()).not.toBeNull()
  })

  /** A drag within one pane is not a transfer of a file onto itself, so the pane it
   * started in must not offer to receive it. */
  it('does not appear in the pane the drag started in', () => {
    paint({ drag: FROM_LOCAL })
    expect(zone()).toBeNull()
  })

  /** Without a destination on it the zone is just a rectangle, and the whole reason it
   * exists is that "drop anywhere" never said where anywhere was. */
  it('names the directory the drop will land in', () => {
    paint({ drag: FROM_REMOTE })
    expect(screen.getByText('/Users/ada/Documents')).toBeTruthy()
  })

  it('is absent when the pane cannot receive at all', () => {
    paint({ drag: FROM_REMOTE }, false)
    expect(zone()).toBeNull()
  })

  it('is marked for the hit test with the pane it belongs to', () => {
    paint({ drag: FROM_REMOTE })
    expect(target().getAttribute(ZONE_ATTR)).toBe('local')
  })

  it('lights up while the pointer is over it, and only then', () => {
    const { rerender } = paint({ drag: FROM_REMOTE })
    expect(target().className).not.toContain('dropzone--over')
    rerender(
      <FileList
        pane="local"
        rows={NONE}
        loading={false}
        direction="up"
        sort={{ key: 'name', dir: 1 }}
        onSort={vi.fn()}
        selected={null}
        onSelect={vi.fn()}
        onOpen={vi.fn()}
        onAction={vi.fn()}
        canReceive
        drag={FROM_REMOTE}
        over={{ pane: 'local', folder: null, zone: true }}
      />,
    )
    expect(target().className).toContain('dropzone--over')
  })
})

describe('the listing as a target', () => {
  it('marks itself for the hit test while it can receive', () => {
    const { container } = paint({ drag: FROM_REMOTE, rows: LISTING })
    expect(listing(container).getAttribute(PANE_ATTR)).toBe('local')
  })

  it('marks the empty listing too', () => {
    const { container } = paint({ drag: FROM_REMOTE })
    expect(listing(container).getAttribute(PANE_ATTR)).toBe('local')
  })

  it('carries no mark while nothing is dragged', () => {
    const { container } = paint({ drag: null, rows: LISTING })
    expect(listing(container).hasAttribute(PANE_ATTR)).toBe(false)
  })

  it('carries no mark in the pane the drag started in', () => {
    const { container } = paint({ drag: FROM_LOCAL, rows: LISTING })
    expect(listing(container).hasAttribute(PANE_ATTR)).toBe(false)
  })

  it('carries no mark when it cannot receive', () => {
    const { container } = paint({ drag: FROM_REMOTE, rows: LISTING }, false)
    expect(listing(container).hasAttribute(PANE_ATTR)).toBe(false)
  })

  it('shows the pane-wide highlight when the drop would land in the open directory', () => {
    const { container } = paint({
      drag: FROM_REMOTE,
      rows: LISTING,
      over: { pane: 'local', folder: null, zone: false },
    })
    expect(listing(container).className).toContain('rows--dropping')
  })

  it('does not show it while the pointer is over a folder or the zone', () => {
    const a = paint({
      drag: FROM_REMOTE,
      rows: LISTING,
      over: { pane: 'local', folder: 'backups', zone: false },
    })
    expect(listing(a.container).className).not.toContain('rows--dropping')
    a.unmount()
    const b = paint({
      drag: FROM_REMOTE,
      rows: LISTING,
      over: { pane: 'local', folder: null, zone: true },
    })
    expect(listing(b.container).className).not.toContain('rows--dropping')
  })

  it('ignores a target in the other pane', () => {
    const { container } = paint({
      drag: FROM_REMOTE,
      rows: LISTING,
      over: { pane: 'remote', folder: null, zone: false },
    })
    expect(listing(container).className).not.toContain('rows--dropping')
  })
})

describe('rows', () => {
  it('carry their name and kind, so a hit test can name the folder under the pointer', () => {
    paint({ rows: LISTING })
    expect(rowFor('backups').getAttribute(ROW_NAME_ATTR)).toBe('backups')
    expect(rowFor('backups').getAttribute(ROW_DIR_ATTR)).toBe('true')
    expect(rowFor('a.bin').getAttribute(ROW_NAME_ATTR)).toBe('a.bin')
    expect(rowFor('a.bin').getAttribute(ROW_DIR_ATTR)).toBe('false')
  })

  it('light up the folder the drop would land inside', () => {
    paint({
      drag: FROM_REMOTE,
      rows: LISTING,
      over: { pane: 'local', folder: 'backups', zone: false },
    })
    expect(rowFor('backups').className).toContain('row--into')
    expect(rowFor('a.bin').className).not.toContain('row--into')
  })

  it('hand a press to the drag, with the row and the event', () => {
    const onDragStart = vi.fn()
    const onSelect = vi.fn()
    paint({ rows: LISTING, onDragStart, onSelect })
    fireEvent.pointerDown(rowFor('a.bin'), { button: 0, clientX: 10, clientY: 10 })
    expect(onDragStart).toHaveBeenCalledOnce()
    expect(onDragStart.mock.calls[0]?.[0]).toEqual(LISTING[1])
    // Picking something up selects it, as it always did.
    expect(onSelect).toHaveBeenCalledWith('a.bin')
  })

  it('do not start a drag from a secondary button', () => {
    const onDragStart = vi.fn()
    paint({ rows: LISTING, onDragStart })
    fireEvent.pointerDown(rowFor('a.bin'), { button: 2, clientX: 10, clientY: 10 })
    expect(onDragStart).not.toHaveBeenCalled()
  })

  /** A press on the row's upload button is a press on the button. */
  it("do not start a drag from the row's own buttons", () => {
    const onDragStart = vi.fn()
    paint({ rows: LISTING, onDragStart })
    fireEvent.pointerDown(screen.getByLabelText('Upload a.bin'), { button: 0 })
    expect(onDragStart).not.toHaveBeenCalled()
  })

  it('cannot be picked up when there is nowhere to drop them', () => {
    const onSelect = vi.fn()
    paint({ rows: LISTING, onSelect })
    fireEvent.pointerDown(rowFor('a.bin'), { button: 0 })
    expect(onSelect).not.toHaveBeenCalled()
  })
})
