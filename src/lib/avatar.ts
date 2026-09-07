/** What a server's avatar shows, and what colour it is.
 *
 * Lived in `TitleBar` and was imported by two other components, which made a component
 * module the home of shared logic. It is here so all three can have it, and so the
 * initials rule can be tested without mounting anything. */

/** The design's swatch row. A generated tint and a chosen tint come from the same six
 * colours, so an auto-assigned server cannot end up a shade the editor would never
 * offer. */
const PALETTE = ['#2456E6', '#7C4DDB', '#E8A03C', '#2E9E5B', '#0FA3A3', '#D6453C']

export function tintFor(name: string): string {
  let hash = 0
  for (const char of name) hash = (hash * 31 + char.charCodeAt(0)) >>> 0
  return PALETTE[hash % PALETTE.length] ?? '#2456E6'
}

/** An IPv4 address, an IPv6 address, or something close enough to one that initials
 * would be digits. */
function isAddress(name: string): boolean {
  const trimmed = name.trim()
  if (/^\d{1,3}(\.\d{1,3}){3}$/.test(trimmed)) return true
  // IPv6: hex groups and colons, and at least one colon to tell it from a word.
  if (/^[0-9a-f:]+$/i.test(trimmed) && trimmed.includes(':')) return true
  // A name that carries no letters at all cannot produce a readable monogram.
  return !/[a-z]/i.test(trimmed)
}

/** What to draw in the avatar.
 *
 * `104.197.160.37` used to come out as "10", which names nothing — every host on that
 * subnet gives the same two characters, and the digits are not a monogram in the first
 * place. An address gets the server glyph instead; a name people chose keeps its
 * initials, which is what the design draws. */
export type AvatarMark = { kind: 'initials'; text: string } | { kind: 'address' }

export function avatarMark(name: string): AvatarMark {
  if (name.trim() === '' || isAddress(name)) return { kind: 'address' }
  return { kind: 'initials', text: initialsFor(name) }
}

/** The design's rule: split on whitespace and dots, take the first letter of the first
 * two parts. "Acme Corp" → AC, "Ledger.io" → LI, "Home NAS" → HN.
 *
 * Parts without a letter are skipped, so `sftp.acme-corp.com` gives SA rather than
 * letting a protocol prefix and a TLD decide the monogram. */
export function initialsFor(name: string): string {
  const parts = name
    .split(/[\s.\-_/]+/)
    .filter((part) => /[a-z0-9]/i.test(part))
    .filter((part) => !IGNORED.has(part.toLowerCase()))
  const usable = parts.length > 0 ? parts : [name.trim()]
  const first = usable[0]?.[0] ?? '?'
  const second = usable[1]?.[0] ?? ''
  return (first + second).toUpperCase()
}

/** Only the parts that say what a host *is* rather than which one. TLDs deliberately
 * stay: the design draws `Ledger.io` as LI, and dropping the `io` would leave a bare
 * `L` that collides with every other server beginning in L. */
const IGNORED = new Set(['sftp', 'ftp', 'ftps', 'ssh', 'www'])
