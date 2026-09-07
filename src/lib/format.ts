/** Formatting shared by both panes and the queue. */

const UNITS = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'] as const

export function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B'
  const exponent = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), UNITS.length - 1)
  const value = bytes / 1024 ** exponent
  const unit = UNITS[exponent] ?? 'B'
  return `${value < 10 && exponent > 0 ? value.toFixed(1) : Math.round(value)} ${unit}`
}

export function formatSpeed(bytesPerSecond: number | null): string {
  if (bytesPerSecond === null || bytesPerSecond === 0) return '—'
  return `${formatBytes(bytesPerSecond)}/s`
}

export function formatDuration(seconds: number | null): string {
  if (seconds === null) return '—'
  if (seconds < 60) return `${Math.round(seconds)}s`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes}m ${Math.round(seconds % 60)}s`
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`
}

/** The design's column is 84px of mono, sized for "Jul 14 22:39" — every date in the
 * design is from the current year. A real listing is full of older files, and adding
 * the year to a string that already carried a clock time overflowed the column and
 * wrapped the cell onto two lines.
 *
 * So the year replaces the time rather than joining it, which is the convention Finder
 * and `ls -l` both use: recent files are worth timing to the minute, a file from two
 * years ago is not. */
export function formatWhen(iso: string | null): string {
  if (!iso) return '—'
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) return '—'
  const now = new Date()
  const sameYear = date.getFullYear() === now.getFullYear()
  return date.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    ...(sameYear ? { hour: '2-digit', minute: '2-digit' } : { year: 'numeric' }),
  })
}

/** Path segments for the breadcrumb, with the root kept as its own crumb. */
export function crumbs(path: string): { label: string; path: string }[] {
  const parts = path.split('/').filter(Boolean)
  const out = [{ label: '/', path: '/' }]
  let current = ''
  for (const part of parts) {
    current += `/${part}`
    out.push({ label: part, path: current })
  }
  return out
}

export function parentPath(path: string): string {
  const trimmed = path.replace(/\/+$/, '')
  const index = trimmed.lastIndexOf('/')
  return index <= 0 ? '/' : trimmed.slice(0, index)
}

export function joinPath(base: string, name: string): string {
  return base === '/' ? `/${name}` : `${base}/${name}`
}
