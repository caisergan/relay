import { describe, expect, it } from 'vitest'

import { addsToSelection, clickSelect } from './selection'

describe('clickSelect', () => {
  it('makes a plain click the whole selection', () => {
    expect(clickSelect(['a.bin', 'b.bin'], 'c.bin', false)).toEqual(['c.bin'])
    expect(clickSelect(['a.bin', 'b.bin'], 'a.bin', false)).toEqual(['a.bin'])
  })

  it('adds a row with the modifier, keeping the order rows were picked in', () => {
    expect(clickSelect(['b.bin'], 'a.bin', true)).toEqual(['b.bin', 'a.bin'])
    expect(clickSelect([], 'backups', true)).toEqual(['backups'])
  })

  it('takes a row out with the modifier, down to nothing at all', () => {
    expect(clickSelect(['a.bin', 'b.bin'], 'a.bin', true)).toEqual(['b.bin'])
    expect(clickSelect(['a.bin'], 'a.bin', true)).toEqual([])
  })
})

describe('addsToSelection', () => {
  // Ctrl-click there opens the inspector, and must not also add the row.
  it('is ⌘ on a Mac', () => {
    expect(addsToSelection({ metaKey: true, ctrlKey: false }, true)).toBe(true)
    expect(addsToSelection({ metaKey: false, ctrlKey: true }, true)).toBe(false)
  })

  it('is Ctrl everywhere else', () => {
    expect(addsToSelection({ metaKey: false, ctrlKey: true }, false)).toBe(true)
    expect(addsToSelection({ metaKey: true, ctrlKey: false }, false)).toBe(false)
  })
})
