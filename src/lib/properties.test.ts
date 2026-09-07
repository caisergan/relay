/** The words the inspector puts on an entry.
 *
 * Sizes and timestamps go through `toLocaleString`, so their exact punctuation belongs
 * to whatever locale the suite runs under — `4,900` here, `4.900` in Istanbul. These
 * assert the shape and the parts that are ours, the way `format.test.ts` already does
 * for the listing's date column. Everything else is locale-free and asserted exactly. */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { LocalEntry, RemoteEntry } from '@/ipc/gen'

import type { DirSize } from '@/ipc/gen'

import {
  countLabel,
  kindLabel,
  localProperties,
  octalMode,
  remoteProperties,
  measuredSizeLabel,
  sizeLabel,
  whenLabel,
  withMeasurement,
} from './properties'

describe('kindLabel', () => {
  it('names a directory', () => {
    expect(kindLabel('src', 'dir', null)).toBe('Folder')
  })

  it('names a file by its extension', () => {
    expect(kindLabel('us-server.ovpn', 'file', null)).toBe('OVPN file')
    expect(kindLabel('archive.tar.gz', 'file', null)).toBe('GZ file')
  })

  it('calls a file with no extension a file', () => {
    expect(kindLabel('README', 'file', null)).toBe('File')
    expect(kindLabel('notes.', 'file', null)).toBe('File')
  })

  /** A leading dot makes a file hidden, it does not make it a "GITIGNORE file". */
  it('does not read a dotfile’s name as an extension', () => {
    expect(kindLabel('.gitignore', 'file', null)).toBe('File')
  })

  /** "SOMETHINGVERYLONG file" is a worse answer than "File". */
  it('gives up on an extension long enough to be a word', () => {
    expect(kindLabel('notes.somethingverylong', 'file', null)).toBe('File')
  })

  it('says what a symlink points at', () => {
    expect(kindLabel('www', 'symlink', 'dir')).toBe('Symbolic link to a folder')
    expect(kindLabel('latest.log', 'symlink', 'file')).toBe('Symbolic link to a file')
  })

  /** A null target means the server could not stat it: a link to nowhere, or to
   * somewhere this account may not look. It explains a row that will not open. */
  it('calls a link with no reachable target broken', () => {
    expect(kindLabel('dangling', 'symlink', null)).toBe('Broken symbolic link')
  })
})

describe('sizeLabel', () => {
  /** What the server reports for a directory is the size of its record, not of what is
   * in it, and printing it invites the reading that a folder holds 4 KB. */
  it('refuses to give a folder a size', () => {
    expect(sizeLabel(4096, true)).toBe('—')
  })

  it('gives both the reading and the count for a file', () => {
    expect(sizeLabel(4900, false)).toMatch(/^4\.8 KB \(4.?900 bytes\)$/u)
  })

  /** Under a kilobyte the two forms are the same number twice. */
  it('prints one number when both would be the same', () => {
    expect(sizeLabel(512, false)).toBe('512 bytes')
    expect(sizeLabel(0, false)).toBe('0 bytes')
  })

  it('counts a single byte in the singular', () => {
    expect(sizeLabel(1, false)).toBe('1 byte')
  })
})

describe('whenLabel', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-09-08T12:00:00Z'))
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  /** The listing's column drops the year to fit 84px, and drops the clock when it keeps
   * the year. The inspector is where someone goes when that was not enough. */
  it('keeps the year and the clock, down to the second', () => {
    const text = whenLabel('2025-07-14T22:39:07Z')
    expect(text).toContain('2025')
    expect(text).toMatch(/\d{2}:\d{2}:\d{2}/)
  })

  it('has nothing to say about a missing or unparseable date', () => {
    expect(whenLabel(null)).toBeNull()
    expect(whenLabel('not a date')).toBeNull()
  })
})

describe('octalMode', () => {
  it('writes the bits the way ls and chmod write them', () => {
    expect(octalMode(0o644)).toBe('644')
    expect(octalMode(0o755)).toBe('755')
  })

  /** The regression this masking exists for: a server packs the file-type bits into the
   * same integer, so an unmasked 0o100644 would be shown as "100644". */
  it('masks off the file-type bits the mode is packed with', () => {
    expect(octalMode(0o100644)).toBe('644')
    expect(octalMode(0o40755)).toBe('755')
  })

  /** Setuid, setgid and sticky are exactly the cases where the leading digit matters. */
  it('keeps a fourth digit when one is set', () => {
    expect(octalMode(0o4755)).toBe('4755')
    expect(octalMode(0o1777)).toBe('1777')
  })

  it('pads a short mode to three digits', () => {
    expect(octalMode(0o7)).toBe('007')
  })

  it('has nothing to say about a mode the server withheld', () => {
    expect(octalMode(null)).toBeNull()
    expect(octalMode(Number.NaN)).toBeNull()
  })
})

const local = (over: Partial<LocalEntry> = {}): LocalEntry => ({
  name: 'us-server.ovpn',
  path: '/Users/ada/Downloads/us-server.ovpn',
  kind: 'file',
  targetKind: null,
  size: 4900,
  modified: '2026-09-08T00:58:00Z',
  hidden: false,
  readonly: false,
  ...over,
})

const remote = (over: Partial<RemoteEntry> = {}): RemoteEntry => ({
  name: 'us-server.ovpn',
  kind: 'file',
  targetKind: null,
  size: 4900,
  modified: '2026-09-08T00:58:00Z',
  perms: 'rw-r--r--',
  mode: 0o100644,
  owner: 'ada',
  group: 'staff',
  ...over,
})

/** The value against a label, or `undefined` when the panel would not show that row. */
const factFor = (facts: { label: string; value: string }[], label: string) =>
  facts.find((fact) => fact.label === label)?.value

describe('localProperties', () => {
  it('answers where the file is with the folder holding it, not with its own path', () => {
    const it = localProperties(local())
    expect(factFor(it.facts, 'Where')).toBe('/Users/ada/Downloads')
    expect(it.name).toBe('us-server.ovpn')
    expect(it.kind).toBe('OVPN file')
    expect(it.isDir).toBe(false)
  })

  /** The local side reports one bit, not a mode. Calling it "Permissions" would imply
   * the panel was hiding the other eight. */
  it('reports the one access bit the local disk gives, under its own name', () => {
    expect(factFor(localProperties(local()).facts, 'Access')).toBe('Read and write')
    expect(factFor(localProperties(local({ readonly: true })).facts, 'Access')).toBe(
      'Read only',
    )
    expect(factFor(localProperties(local()).facts, 'Permissions')).toBeUndefined()
  })

  /** A row that is not hidden should not carry a line saying so. */
  it('mentions hidden only when the entry is', () => {
    expect(factFor(localProperties(local()).facts, 'Hidden')).toBeUndefined()
    expect(factFor(localProperties(local({ hidden: true })).facts, 'Hidden')).toBe('Yes')
  })

  it('treats a link to a directory as a directory', () => {
    const it = localProperties(local({ name: 'www', kind: 'symlink', targetKind: 'dir' }))
    expect(it.isDir).toBe(true)
    expect(factFor(it.facts, 'Size')).toBe('—')
  })
})

describe('remoteProperties', () => {
  /** A remote listing names entries but not their paths, so the directory has to be
   * handed in — getting this wrong would put every remote file in the same folder. */
  it('answers where the file is with the directory it was listed from', () => {
    expect(factFor(remoteProperties(remote(), '/srv/vpn').facts, 'Where')).toBe('/srv/vpn')
  })

  it('shows the permissions as both the letters and the number', () => {
    expect(factFor(remoteProperties(remote(), '/srv').facts, 'Permissions')).toBe(
      'rw-r--r-- (644)',
    )
  })

  it('falls back to whichever half of the permissions the server gave', () => {
    expect(factFor(remoteProperties(remote({ mode: null }), '/srv').facts, 'Permissions')).toBe(
      'rw-r--r--',
    )
    expect(
      factFor(remoteProperties(remote({ perms: null }), '/srv').facts, 'Permissions'),
    ).toBe('644')
  })

  it('carries the owner and group the local side has no answer for', () => {
    const facts = remoteProperties(remote(), '/srv').facts
    expect(factFor(facts, 'Owner')).toBe('ada')
    expect(factFor(facts, 'Group')).toBe('staff')
  })

  /** A fact nothing reported has to be marked as such, so the panel can draw it as an
   * admission rather than as a value. */
  it('marks a withheld fact rather than inventing or dropping it', () => {
    const facts = remoteProperties(
      remote({ owner: null, group: null, perms: null, mode: null, modified: null }),
      '/srv',
    ).facts
    for (const label of ['Owner', 'Group', 'Permissions', 'Modified']) {
      const fact = facts.find((item) => item.label === label)
      expect(fact?.unknown, `${label} should be marked unknown`).toBe(true)
      expect(fact?.value).toBe('not known')
    }
  })
})

const measured = (over: Partial<DirSize> = {}): DirSize => ({
  bytes: 1288490188,
  files: 348,
  folders: 12,
  truncated: false,
  ...over,
})

describe('countLabel', () => {
  /** The total says whether it will fit; the count says how long it will take. Over
   * SFTP, which opens a connection per file, the count is often the deciding number. */
  it('counts the files and the folders holding them', () => {
    expect(countLabel(measured())).toBe('348 files in 12 folders')
  })

  it('drops the folders when the tree is flat', () => {
    expect(countLabel(measured({ folders: 0 }))).toBe('348 files')
  })

  it('counts one of each in the singular', () => {
    expect(countLabel(measured({ files: 1, folders: 1 }))).toBe('1 file in 1 folder')
  })

  it('says so when there is nothing in there at all', () => {
    expect(countLabel(measured({ files: 0, folders: 0 }))).toBe('Nothing')
  })

  /** A walk that stopped at a limit produces a floor. Presenting it as a total is the
   * one thing this must not do. */
  it('marks a count the walk did not finish as a floor', () => {
    expect(countLabel(measured({ truncated: true }))).toBe('at least 348 files in 12 folders')
  })
})

describe('measuredSizeLabel', () => {
  it('gives the reading and the exact count when the walk finished', () => {
    expect(measuredSizeLabel(measured())).toMatch(/^1\.2 GB \(1.288.490.188 bytes\)$/u)
  })

  /** "at least 1.2 GB (1,288,490,188 bytes)" puts a byte-exact number beside an
   * admission that it is not the total, and the precision is the more convincing of
   * the two. */
  it('drops the byte-exact number when the total is only a floor', () => {
    expect(measuredSizeLabel(measured({ truncated: true }))).toBe('at least 1.2 GB')
  })
})

describe('withMeasurement', () => {
  const facts = () => remoteProperties(remote({ kind: 'dir', size: 4096 }), '/srv').facts

  it('leaves the facts alone when there is nothing to measure', () => {
    const base = facts()
    expect(withMeasurement(base, null)).toEqual(base)
  })

  /** A folder's size row reads "—" until there is something better to put there, so
   * the measurement replaces it rather than being appended somewhere else. */
  it('replaces the size row while the walk is running', () => {
    const out = withMeasurement(facts(), { state: 'measuring' })
    const size = out.find((fact) => fact.label === 'Size')
    expect(size?.value).toBe('measuring…')
    expect(size?.unknown).toBe(true)
    expect(out.filter((fact) => fact.label === 'Size')).toHaveLength(1)
  })

  it('says so when the walk could not be done at all', () => {
    const size = withMeasurement(facts(), { state: 'failed' }).find((f) => f.label === 'Size')
    expect(size?.value).toBe('could not be measured')
    expect(size?.unknown).toBe(true)
  })

  /** The two numbers about the same thing belong next to each other. */
  it('puts the count directly after the size once the walk lands', () => {
    const out = withMeasurement(facts(), { state: 'done', size: measured() })
    const labels = out.map((fact) => fact.label)
    expect(labels.indexOf('Contains')).toBe(labels.indexOf('Size') + 1)
    expect(out[labels.indexOf('Contains')]?.value).toBe('348 files in 12 folders')
    expect(out[labels.indexOf('Size')]?.value).toMatch(/^1\.2 GB/)
  })

  it('does not disturb the other facts', () => {
    const before = facts()
    const after = withMeasurement(before, { state: 'done', size: measured() })
    for (const label of ['Modified', 'Where', 'Permissions', 'Owner', 'Group']) {
      expect(after.find((f) => f.label === label)).toEqual(
        before.find((f) => f.label === label),
      )
    }
  })

  /** The panel renders one row per fact keyed by label, so a duplicate would be a
   * React key collision as well as a nonsense reading. */
  it('never leaves two size rows behind', () => {
    const once = withMeasurement(facts(), { state: 'done', size: measured() })
    const twice = withMeasurement(once, { state: 'done', size: measured() })
    expect(twice.filter((fact) => fact.label === 'Size')).toHaveLength(1)
    expect(twice.filter((fact) => fact.label === 'Contains')).toHaveLength(1)
  })
})
