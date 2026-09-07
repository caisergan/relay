import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { baseName, formatBytes, formatWhen } from './format'

describe('formatWhen', () => {
  // Fixed so "this year" is not whatever year the suite happens to run in.
  beforeEach(() => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-09-07T12:00:00Z'))
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  it('times a file from this year to the minute', () => {
    const text = formatWhen('2026-07-14T22:39:00Z')
    expect(text).toMatch(/\d{2}:\d{2}/)
    expect(text).not.toContain('2026')
  })

  // The regression: the year used to be added *alongside* the clock time, and the
  // resulting string overflowed the design's 84px column and wrapped onto two lines.
  it('replaces the time with the year for an older file, rather than adding it', () => {
    const text = formatWhen('2025-09-07T01:07:00Z')
    expect(text).toContain('2025')
    expect(text).not.toMatch(/\d{2}:\d{2}/)
  })

  it('renders a missing or unparseable date as a dash', () => {
    expect(formatWhen(null)).toBe('—')
    expect(formatWhen('not a date')).toBe('—')
  })
})

describe('formatBytes', () => {
  it('keeps a decimal only where it carries information', () => {
    expect(formatBytes(0)).toBe('0 B')
    expect(formatBytes(420)).toBe('420 B')
    expect(formatBytes(3789)).toBe('3.7 KB')
    expect(formatBytes(1024 * 1024 * 25)).toBe('25 MB')
  })
})

describe('baseName', () => {
  it('takes the last segment of a posix path', () => {
    expect(baseName('/var/www/app.js')).toBe('app.js')
    expect(baseName('app.js')).toBe('app.js')
    expect(baseName('/')).toBe('')
  })

  /// The operating system hands dragged-in paths over in its own spelling, and a
  /// Windows path has no forward slash in it at all.
  it('takes the last segment of a windows path', () => {
    expect(baseName('C:\\Users\\ada\\notes.md')).toBe('notes.md')
    expect(baseName('\\\\server\\share\\report.pdf')).toBe('report.pdf')
  })

  /// A dragged folder arrives with a trailing separator on some platforms, and an
  /// empty name would upload it to the directory itself.
  it('ignores a trailing separator so a folder keeps its name', () => {
    expect(baseName('/var/www/assets/')).toBe('assets')
    expect(baseName('C:\\Users\\ada\\')).toBe('ada')
  })
})
