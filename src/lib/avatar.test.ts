import { describe, expect, it } from 'vitest'

import { avatarMark, initialsFor, tintFor } from './avatar'

describe('avatarMark', () => {
  // The bug: "104.197.160.37" came out as "10", which names nothing — every host on
  // the subnet gives the same two characters, and digits are not a monogram.
  it('gives an address the server glyph rather than digits', () => {
    for (const address of ['104.197.160.37', '10.0.0.1', '192.168.1.20', '::1', 'fe80::1']) {
      expect(avatarMark(address), address).toEqual({ kind: 'address' })
    }
  })

  it('keeps initials for a name someone chose', () => {
    expect(avatarMark('Acme Corp')).toEqual({ kind: 'initials', text: 'AC' })
  })

  it('falls back to the glyph rather than rendering nothing', () => {
    expect(avatarMark('')).toEqual({ kind: 'address' })
    expect(avatarMark('   ')).toEqual({ kind: 'address' })
  })
})

describe('initialsFor', () => {
  // The design's own examples, so the rule stays the design's.
  it('matches the design', () => {
    expect(initialsFor('Acme Corp')).toBe('AC')
    expect(initialsFor('Ledger.io')).toBe('LI')
    expect(initialsFor('Rosen Bakery')).toBe('RB')
    expect(initialsFor('Home NAS')).toBe('HN')
    expect(initialsFor('Media Backup')).toBe('MB')
  })

  // A protocol prefix says what the host is, never which one.
  it('skips a protocol prefix and splits a hyphenated host', () => {
    expect(initialsFor('sftp.acme-corp.com')).toBe('AC')
    expect(initialsFor('ssh.deploy.example')).toBe('DE')
  })

  it('handles a single word and a lone letter', () => {
    expect(initialsFor('staging')).toBe('S')
    expect(initialsFor('x')).toBe('X')
  })
})

describe('tintFor', () => {
  it('is stable and always from the design swatches', () => {
    const swatches = ['#2456E6', '#7C4DDB', '#E8A03C', '#2E9E5B', '#0FA3A3', '#D6453C']
    expect(tintFor('Acme Corp')).toBe(tintFor('Acme Corp'))
    for (const name of ['a', 'Acme Corp', '104.197.160.37', 'zzz', '']) {
      expect(swatches, name).toContain(tintFor(name))
    }
  })
})
