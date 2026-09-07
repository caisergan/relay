import { describe, expect, it } from 'vitest'

import { permsClass, sortRows, type FileRow, type Sort } from './FileList'

function row(name: string, over: Partial<FileRow> = {}): FileRow {
  return {
    key: name,
    name,
    isDir: false,
    hidden: name.startsWith('.'),
    size: 0,
    modified: null,
    perms: null,
    ...over,
  }
}

const byName: Sort = { key: 'name', dir: 1 }

describe('sortRows', () => {
  it('puts directories above files whichever way the sort points', () => {
    const rows = [row('zeta.txt'), row('alpha', { isDir: true }), row('beta.txt')]

    for (const dir of [1, -1] as const) {
      const sorted = sortRows(rows, { key: 'name', dir })
      expect(sorted[0]?.isDir, `dir=${dir}`).toBe(true)
    }
  })

  it('orders names the way a person reads them, not by code point', () => {
    const names = sortRows([row('file10.txt'), row('file9.txt'), row('File2.txt')], byName).map(
      (r) => r.name,
    )

    // Numeric collation, and case-insensitive: `File2` before `file9` before `file10`.
    expect(names).toEqual(['File2.txt', 'file9.txt', 'file10.txt'])
  })

  it('sorts by size and reverses on demand', () => {
    const rows = [
      row('big', { size: 900 }),
      row('small', { size: 3 }),
      row('mid', { size: 40 }),
    ]

    expect(sortRows(rows, { key: 'size', dir: 1 }).map((r) => r.name)).toEqual([
      'small',
      'mid',
      'big',
    ])
    expect(sortRows(rows, { key: 'size', dir: -1 }).map((r) => r.name)).toEqual([
      'big',
      'mid',
      'small',
    ])
  })

  it('sorts by modification time, with unknown times first', () => {
    const rows = [
      row('newer', { modified: '2026-09-07T10:00:00Z' }),
      row('older', { modified: '2024-01-01T10:00:00Z' }),
      row('unknown'),
    ]

    expect(sortRows(rows, { key: 'modified', dir: 1 }).map((r) => r.name)).toEqual([
      'unknown',
      'older',
      'newer',
    ])
  })

  it('does not mutate the listing it was handed', () => {
    // The rows come from a zustand snapshot the engine owns. Sorting in place would
    // reorder the store's array and desynchronise it from the next `ListingSnapshot`.
    const rows = [row('b'), row('a')]
    const before = rows.map((r) => r.name)

    sortRows(rows, byName)

    expect(rows.map((r) => r.name)).toEqual(before)
  })
})

describe('permsClass', () => {
  // The design tints exactly one mode: a file only its owner can even read. Everything
  // else — including `rwx------` on a directory, which is entirely ordinary — stays
  // faint, so the tint keeps meaning something.
  it('tints only an owner-only file', () => {
    expect(permsClass('rw-------')).toContain('row__perms--private')
  })

  it('leaves every other mode faint', () => {
    for (const mode of ['rwx------', 'rw-r--r--', 'rwxr-xr-x', null]) {
      expect(permsClass(mode), mode ?? 'null').toBe('row__perms')
    }
  })
})
