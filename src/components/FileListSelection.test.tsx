/** Choosing rows — a click, a ⌘-click — and what an action on a chosen row reaches.
 *
 * Rendered through a stateful wrapper rather than with a mock `onSelect`, because the
 * rules are about a sequence: a press that selects a row, then the click that ends the
 * same press. A selection that never changes cannot show one step undoing the last. */

import { fireEvent, render, screen } from '@testing-library/react'
import { useState } from 'react'
import { beforeAll, describe, expect, it, vi } from 'vitest'

import { ROW_NAME_ATTR } from '@/lib/rowDrag'
import { giveViewport } from '@/test/viewport'

import { FileList, type FileRow } from './FileList'

beforeAll(giveViewport)

function entry(name: string, isDir = false): FileRow {
  return { key: name, name, isDir, hidden: false, size: 0, modified: null, perms: null }
}

const ROWS = [entry('backups', true), entry('a.bin'), entry('b.bin'), entry('c.bin')]

/** Both keys, so the rule holds on whichever platform the suite runs: ⌘ counts on a
 * Mac and Ctrl elsewhere, and what jsdom claims to be is not this file's business.
 * `addsToSelection` is tested for that directly. */
const ADD = { metaKey: true, ctrlKey: true }

function Listing({
  initial = [],
  ...props
}: Partial<Parameters<typeof FileList>[0]> & { initial?: string[] }) {
  const [selected, setSelected] = useState(initial)
  return (
    <FileList
      pane="remote"
      rows={ROWS}
      loading={false}
      direction="down"
      sort={{ key: 'name', dir: 1 }}
      onSort={vi.fn()}
      onOpen={vi.fn()}
      onAction={vi.fn()}
      {...props}
      selected={selected}
      onSelect={setSelected}
    />
  )
}

function rowFor(name: string): HTMLElement {
  const el = screen.getByText(name).closest('.row')
  if (!(el instanceof HTMLElement)) throw new Error(`no row rendered for "${name}"`)
  return el
}

/** What the listing draws as selected, top to bottom. */
const selectedNames = () =>
  [...document.querySelectorAll('.row[aria-selected="true"]')].map((el) =>
    el.getAttribute(ROW_NAME_ATTR),
  )

const names = (rows: unknown) => (rows as FileRow[]).map((row) => row.name)

/** A click as a pointer makes one: the press, then the click that ends it. */
function clickRow(name: string, keys = {}) {
  fireEvent.pointerDown(rowFor(name), { button: 0, ...keys })
  fireEvent.click(rowFor(name), keys)
}

describe('selecting rows', () => {
  it('selects one row with a plain click, replacing whatever was selected', () => {
    render(<Listing initial={['a.bin', 'b.bin']} onDragStart={vi.fn()} />)
    clickRow('c.bin')
    expect(selectedNames()).toEqual(['c.bin'])
  })

  it('adds files and folders alike with ⌘-click', () => {
    render(<Listing onDragStart={vi.fn()} />)
    clickRow('a.bin')
    clickRow('backups', ADD)
    clickRow('c.bin', ADD)
    expect(selectedNames()).toEqual(['backups', 'a.bin', 'c.bin'])
  })

  // The press adds the row so a drag can carry it. The click that ends that same press
  // must not toggle it straight back out.
  it('takes a row out with a second ⌘-click, and not before', () => {
    render(<Listing initial={['a.bin']} onDragStart={vi.fn()} />)
    clickRow('b.bin', ADD)
    expect(selectedNames()).toEqual(['a.bin', 'b.bin'])
    clickRow('a.bin', ADD)
    expect(selectedNames()).toEqual(['b.bin'])
  })

  // The local pane while nothing is connected: no drag, so the press does nothing and
  // the click has to do all of it.
  it('works the same in a pane rows cannot be dragged from', () => {
    render(<Listing initial={['a.bin']} />)
    clickRow('b.bin', ADD)
    expect(selectedNames()).toEqual(['a.bin', 'b.bin'])
    clickRow('a.bin', ADD)
    expect(selectedNames()).toEqual(['b.bin'])
  })

  it('narrows a selection to the row a plain click lands on', () => {
    render(<Listing initial={['a.bin', 'b.bin']} onDragStart={vi.fn()} />)
    clickRow('b.bin')
    expect(selectedNames()).toEqual(['b.bin'])
  })

  it('keeps the selection when one of its rows is right-clicked, and replaces it otherwise', () => {
    const onInspect = vi.fn()
    render(<Listing initial={['a.bin', 'b.bin']} onInspect={onInspect} />)
    fireEvent.contextMenu(rowFor('b.bin'))
    expect(selectedNames()).toEqual(['a.bin', 'b.bin'])
    fireEvent.contextMenu(rowFor('c.bin'))
    expect(selectedNames()).toEqual(['c.bin'])
    expect(onInspect).toHaveBeenCalledTimes(2)
  })
})

describe('acting on a selection', () => {
  it('lifts the whole selection when one of its rows is pressed, in listing order', () => {
    const onDragStart = vi.fn()
    render(<Listing initial={['c.bin', 'backups']} onDragStart={onDragStart} />)
    fireEvent.pointerDown(rowFor('c.bin'), { button: 0 })
    expect(names(onDragStart.mock.calls[0]?.[0])).toEqual(['backups', 'c.bin'])
    // Only a click narrows it. A press may be the start of carrying all of it.
    expect(selectedNames()).toEqual(['backups', 'c.bin'])
  })

  it('lifts only the row when a row outside the selection is pressed', () => {
    const onDragStart = vi.fn()
    render(<Listing initial={['a.bin', 'b.bin']} onDragStart={onDragStart} />)
    fireEvent.pointerDown(rowFor('c.bin'), { button: 0 })
    expect(names(onDragStart.mock.calls[0]?.[0])).toEqual(['c.bin'])
  })

  it('lifts the selection together with the row a ⌘-press adds to it', () => {
    const onDragStart = vi.fn()
    render(<Listing initial={['c.bin']} onDragStart={onDragStart} />)
    fireEvent.pointerDown(rowFor('a.bin'), { button: 0, ...ADD })
    expect(names(onDragStart.mock.calls[0]?.[0])).toEqual(['a.bin', 'c.bin'])
  })

  it("sends the selection from a selected row's transfer button, and one row from any other", () => {
    const onAction = vi.fn()
    render(<Listing initial={['a.bin', 'b.bin']} onAction={onAction} />)
    const [first] = screen.getAllByLabelText('Download 2 items')
    if (first) fireEvent.click(first)
    expect(names(onAction.mock.calls[0]?.[0])).toEqual(['a.bin', 'b.bin'])
    fireEvent.click(screen.getByLabelText('Download c.bin'))
    expect(names(onAction.mock.calls[1]?.[0])).toEqual(['c.bin'])
    // A button is not a row: pressing one changes nothing about what is selected.
    expect(selectedNames()).toEqual(['a.bin', 'b.bin'])
  })

  it('deletes the selection from the keyboard', () => {
    const onDelete = vi.fn()
    render(<Listing initial={['a.bin', 'c.bin']} onDelete={onDelete} />)
    fireEvent.keyDown(rowFor('c.bin'), { key: 'Delete' })
    expect(names(onDelete.mock.calls[0]?.[0])).toEqual(['a.bin', 'c.bin'])
  })

  // An action reaches what can be seen. A name a filter is hiding stays selected, but it
  // is not carried off with the rows on show.
  it('leaves out selected names the listing is not showing', () => {
    const onDelete = vi.fn()
    render(<Listing initial={['a.bin', 'hidden-by-filter.bin']} onDelete={onDelete} />)
    fireEvent.keyDown(rowFor('a.bin'), { key: 'Delete' })
    expect(names(onDelete.mock.calls[0]?.[0])).toEqual(['a.bin'])
  })
})
