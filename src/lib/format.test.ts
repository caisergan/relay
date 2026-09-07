import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { formatBytes, formatWhen } from './format'

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
