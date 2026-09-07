/** The hit test behind a drop.
 *
 * Pure DOM in, target out: no React, no pointer. What matters is the precedence — the
 * landing area over the listing, a folder row over the listing it sits in, and the
 * source pane over nothing at all. */

import { describe, expect, it } from 'vitest'

import {
  PANE_ATTR,
  resolveTarget,
  ROW_DIR_ATTR,
  ROW_NAME_ATTR,
  sameTarget,
  ZONE_ATTR,
} from './rowDrag'

function mount(html: string): Document {
  document.body.innerHTML = html
  return document
}

const REMOTE = `
  <div class="pane">
    <div class="rows" ${PANE_ATTR}="remote" id="listing">
      <div class="row" ${ROW_NAME_ATTR}="backups" ${ROW_DIR_ATTR}="true" id="folder">
        <span id="folder-name">backups</span>
      </div>
      <div class="row" ${ROW_NAME_ATTR}="a.bin" ${ROW_DIR_ATTR}="false" id="file">a.bin</div>
    </div>
    <div class="dropzone" ${ZONE_ATTR}="remote" id="zone"><span id="zone-label">Drop in</span></div>
  </div>
  <div class="sidebar" id="elsewhere"></div>
`

const at = (id: string) => document.getElementById(id)

describe('resolveTarget', () => {
  it('finds nothing for no element', () => {
    expect(resolveTarget(null, 'local')).toBeNull()
  })

  it('finds nothing outside any listing', () => {
    mount(REMOTE)
    expect(resolveTarget(at('elsewhere'), 'local')).toBeNull()
  })

  it('lands in the open directory when the pointer is over the listing itself', () => {
    mount(REMOTE)
    expect(resolveTarget(at('listing'), 'local')).toEqual({
      pane: 'remote',
      folder: null,
      zone: false,
    })
  })

  it('names the folder when the pointer is over a folder row, however deep', () => {
    mount(REMOTE)
    expect(resolveTarget(at('folder-name'), 'local')).toEqual({
      pane: 'remote',
      folder: 'backups',
      zone: false,
    })
  })

  /** A file is not a destination. A drop on one falls through to the listing. */
  it('does not treat a file row as a destination', () => {
    mount(REMOTE)
    expect(resolveTarget(at('file'), 'local')).toEqual({
      pane: 'remote',
      folder: null,
      zone: false,
    })
  })

  it('prefers the landing area, which floats over the listing', () => {
    mount(REMOTE)
    expect(resolveTarget(at('zone-label'), 'local')).toEqual({
      pane: 'remote',
      folder: null,
      zone: true,
    })
  })

  /** Dragging within one pane is not a transfer of a file onto itself. */
  it('offers nothing in the pane the drag started in', () => {
    mount(REMOTE)
    expect(resolveTarget(at('folder'), 'remote')).toBeNull()
    expect(resolveTarget(at('zone'), 'remote')).toBeNull()
    expect(resolveTarget(at('listing'), 'remote')).toBeNull()
  })

  it('ignores a listing that is not marked as receiving', () => {
    mount(REMOTE)
    at('listing')?.removeAttribute(PANE_ATTR)
    expect(resolveTarget(at('folder'), 'local')).toBeNull()
  })
})

describe('sameTarget', () => {
  it('treats two nulls as the same and a null and a target as different', () => {
    expect(sameTarget(null, null)).toBe(true)
    expect(sameTarget(null, { pane: 'remote', folder: null, zone: false })).toBe(false)
  })

  it('compares field by field', () => {
    const a = { pane: 'remote', folder: 'x', zone: false } as const
    expect(sameTarget(a, { ...a })).toBe(true)
    expect(sameTarget(a, { ...a, folder: null })).toBe(false)
    expect(sameTarget(a, { ...a, zone: true })).toBe(false)
    expect(sameTarget(a, { ...a, pane: 'local' })).toBe(false)
  })
})
