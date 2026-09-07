/** What the inspector says about one entry, and the words it says it in.
 *
 * Separate from the panel that draws it, and pure, because the two halves fail in
 * different ways and only one of them can be tested. The panel is a floating box
 * measured against the pointer, which jsdom has no layout to answer for. But "what
 * does a symlink whose target could not be stat'd call itself", "what is mode 0o644
 * written the way `ls` writes it", and "does a 512-byte file really need to say
 * 512 bytes twice" are all decided here, in functions that take values and return
 * strings.
 *
 * The two panes carry different facts about their entries — the server knows an owner
 * and a mode, the local disk knows whether the file is writable — so rather than
 * flatten both into one lowest-common-denominator shape, each side builds its own list
 * and the panel renders whatever list it is handed. */

import type { DirSize, FileKind, LocalEntry, RemoteEntry } from '@/ipc/gen'

import { formatBytes, parentPath } from './format'

/** One label/value line in the panel. */
export interface Fact {
  label: string
  value: string
  /** Data rather than prose: paths, sizes, modes, owners. Set for the mono face. */
  mono?: boolean
  /** The value is an admission that nothing is known, not a value. Drawn faintly and
   * in italics, the way the conflict sheet already draws an unknown timestamp, so a
   * missing fact cannot be misread as a real one. */
  unknown?: boolean
}

export interface Properties {
  name: string
  isDir: boolean
  /** "Folder", "OVPN file", "Broken symbolic link". */
  kind: string
  facts: Fact[]
}

/** Said where a fact is missing. The conflict sheet already uses this exact phrase for
 * a timestamp the server would not give up, and the two should agree. */
const UNKNOWN = 'not known'

const unknown = (label: string): Fact => ({ label, value: UNKNOWN, unknown: true })

/** An extension long enough to be a word rather than a type. `archive.tar.gz` is a GZ
 * file; `notes.somethingverylong` is just a file, because "SOMETHINGVERYLONG file" is
 * a worse answer than "File". */
const MAX_EXT = 8

/** The entry's type, in words.
 *
 * A leading dot is a hidden file and not an extension: `.gitignore` is a file, not a
 * "GITIGNORE file", which is why the search starts at index 1. */
export function kindLabel(name: string, kind: FileKind, targetKind: FileKind | null): string {
  if (kind === 'dir') return 'Folder'
  if (kind === 'symlink') {
    // `null` means the target could not be stat'd — a link to nowhere, or to somewhere
    // this account may not look. Either way it is worth saying, because it explains a
    // row that will not open.
    if (targetKind === 'dir') return 'Symbolic link to a folder'
    if (targetKind === 'file') return 'Symbolic link to a file'
    return 'Broken symbolic link'
  }
  const dot = name.lastIndexOf('.')
  if (dot < 1 || dot === name.length - 1) return 'File'
  const ext = name.slice(dot + 1)
  return ext.length <= MAX_EXT ? `${ext.toUpperCase()} file` : 'File'
}

/** The size, as both the reading and the number.
 *
 * A directory has no size worth showing: what the server reports is the size of the
 * directory record, not of anything in it, and printing it invites the reading that a
 * folder holds 4 KB. Below a kilobyte the two forms are the same number, so only one
 * of them is printed. */
export function sizeLabel(bytes: number, isDir: boolean): string {
  if (isDir) return '—'
  if (bytes < 1024) return `${bytes.toLocaleString()} ${bytes === 1 ? 'byte' : 'bytes'}`
  return `${formatBytes(bytes)} (${bytes.toLocaleString()} bytes)`
}

/** The full timestamp, not the listing's abbreviated one.
 *
 * The column in the listing drops the year to fit 84px and drops the clock when it
 * keeps the year. The inspector is the place someone comes when that was not enough,
 * so it shows all of it. */
export function whenLabel(iso: string | null): string | null {
  if (!iso) return null
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) return null
  return date.toLocaleString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  })
}

/** The permission bits the way `ls -l` and `chmod` write them.
 *
 * Masked to twelve bits so the file-type bits the server packs into the same integer
 * do not turn 0o644 into 0o100644. Padded to three digits, and left at four when a
 * setuid, setgid or sticky bit is set, because those are exactly the cases where the
 * leading digit is the interesting one. */
export function octalMode(mode: number | null): string | null {
  if (mode === null || !Number.isFinite(mode)) return null
  const bits = Math.trunc(mode) & 0o7777
  return bits.toString(8).padStart(3, '0')
}

/** Permissions as one line: the letters people read and the number people type. */
function permsFact(perms: string | null, mode: number | null): Fact {
  const octal = octalMode(mode)
  if (perms && octal) return { label: 'Permissions', value: `${perms} (${octal})`, mono: true }
  if (perms) return { label: 'Permissions', value: perms, mono: true }
  if (octal) return { label: 'Permissions', value: octal, mono: true }
  return unknown('Permissions')
}

function whenFact(iso: string | null): Fact {
  const when = whenLabel(iso)
  return when ? { label: 'Modified', value: when, mono: true } : unknown('Modified')
}

/** An entry on this Mac. Its path is absolute already, so the folder it sits in is the
 * parent of that. */
export function localProperties(entry: LocalEntry): Properties {
  const isDir = entry.kind === 'dir' || entry.targetKind === 'dir'
  return {
    name: entry.name,
    isDir,
    kind: kindLabel(entry.name, entry.kind, entry.targetKind),
    facts: [
      { label: 'Size', value: sizeLabel(entry.size, isDir), mono: true },
      whenFact(entry.modified),
      { label: 'Where', value: parentPath(entry.path), mono: true },
      // Not "Permissions": the local side reports one bit, not a mode, and calling it
      // permissions would imply the panel is hiding the other eight.
      { label: 'Access', value: entry.readonly ? 'Read only' : 'Read and write' },
      ...(entry.hidden ? [{ label: 'Hidden', value: 'Yes' }] : []),
    ],
  }
}

/** An entry on the server. A remote listing names entries but not their paths, so the
 * directory it came from has to be handed in. */
export function remoteProperties(entry: RemoteEntry, dir: string): Properties {
  const isDir = entry.kind === 'dir' || entry.targetKind === 'dir'
  return {
    name: entry.name,
    isDir,
    kind: kindLabel(entry.name, entry.kind, entry.targetKind),
    facts: [
      { label: 'Size', value: sizeLabel(entry.size, isDir), mono: true },
      whenFact(entry.modified),
      { label: 'Where', value: dir, mono: true },
      permsFact(entry.perms, entry.mode),
      entry.owner ? { label: 'Owner', value: entry.owner, mono: true } : unknown('Owner'),
      entry.group ? { label: 'Group', value: entry.group, mono: true } : unknown('Group'),
    ],
  }
}

/** A folder's size while it is being worked out, and once it is.
 *
 * A folder has no size until something walks it, and walking a server takes one round
 * trip per directory — long enough that the panel has to open without an answer and
 * fill it in. So the size is a small state machine rather than a value. */
export type Measurement =
  { state: 'measuring' } | { state: 'failed' } | { state: 'done'; size: DirSize }

/** How many things are in there.
 *
 * Counted as well as totalled because the two answer different questions: "1.2 GB"
 * says whether it will fit, "348 files in 12 folders" says how long it will take —
 * and over SFTP, which opens a connection per file, the count is often the number
 * that decides. */
export function countLabel(size: DirSize): string {
  const files = `${size.files.toLocaleString()} ${size.files === 1 ? 'file' : 'files'}`
  const folders = `${size.folders.toLocaleString()} ${size.folders === 1 ? 'folder' : 'folders'}`
  const body = size.folders === 0 ? files : `${files} in ${folders}`
  if (size.truncated) return `at least ${body}`
  return size.files === 0 && size.folders === 0 ? 'Nothing' : body
}

/** The size line for a folder that has been measured.
 *
 * A truncated walk drops the exact byte count. Printing "at least 4.8 GB
 * (5,153,960,755 bytes)" puts a number accurate to the byte next to an admission that
 * it is not the total, and the precision is the more convincing of the two. */
export function measuredSizeLabel(size: DirSize): string {
  if (size.truncated) return `at least ${formatBytes(size.bytes)}`
  return sizeLabel(size.bytes, false)
}

/** Fold a folder's measurement into the facts the panel draws.
 *
 * The `Size` row is replaced rather than added to — a folder's row says `—` until
 * there is something better to put there — and the count follows it, so the two
 * numbers about the same thing sit together. */
export function withMeasurement(facts: Fact[], measurement: Measurement | null): Fact[] {
  if (!measurement) return facts
  const size: Fact =
    measurement.state === 'measuring'
      ? { label: 'Size', value: 'measuring…', unknown: true }
      : measurement.state === 'failed'
        ? { label: 'Size', value: 'could not be measured', unknown: true }
        : { label: 'Size', value: measuredSizeLabel(measurement.size), mono: true }

  // Any previous count is dropped before the new one goes in, so folding a second
  // measurement into an already-folded list replaces the row rather than adding a
  // second. The panel keys its rows by label, so a duplicate would be a React key
  // collision as well as two different answers to the same question.
  const out = facts
    .filter((fact) => fact.label !== 'Contains')
    .map((fact) => (fact.label === 'Size' ? size : fact))
  if (measurement.state !== 'done') return out
  const contains: Fact = { label: 'Contains', value: countLabel(measurement.size), mono: true }
  const at = out.findIndex((fact) => fact.label === 'Size')
  out.splice(at + 1, 0, contains)
  return out
}
