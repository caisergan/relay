import { describe, expect, it } from 'vitest'

import { formatTarget, parseTarget } from './url'

describe('parseTarget', () => {
  it('reads a full sftp url', () => {
    const target = parseTarget('sftp://deploy@staging.example:2222/var/www')
    expect(target).toMatchObject({
      proto: 'sftp',
      protoInferred: false,
      username: 'deploy',
      host: 'staging.example',
      port: 2222,
      path: '/var/www',
      error: null,
      unavailable: null,
    })
  })

  it('defaults a bare host to sftp on 22', () => {
    expect(parseTarget('staging.example')).toMatchObject({
      proto: 'sftp',
      port: 22,
      portExplicit: false,
      username: null,
    })
  })

  it('infers ftp from a typed port 21 and says so', () => {
    const target = parseTarget('example.com:21')
    expect(target?.proto).toBe('ftp')
    expect(target?.protoInferred).toBe(true)
    expect(target?.unavailable).toMatch(/not available/)
  })

  it('does not infer over an explicit scheme', () => {
    const target = parseTarget('sftp://example.com:21')
    expect(target?.proto).toBe('sftp')
    expect(target?.protoInferred).toBe(false)
  })

  it('marks ftps as unavailable in this release', () => {
    expect(parseTarget('ftps://example.com')?.unavailable).toMatch(/FTPS/)
  })

  it('handles bracketed ipv6 with a port', () => {
    expect(parseTarget('sftp://root@[2001:db8::1]:2200')).toMatchObject({
      host: '2001:db8::1',
      port: 2200,
      username: 'root',
    })
  })

  it('rejects nonsense rather than guessing', () => {
    expect(parseTarget('example.com:ssh')?.error).toMatch(/not a port/)
    expect(parseTarget('http://example.com')?.error).toMatch(/Unknown protocol/)
    expect(parseTarget('sftp://example.com:99999')?.error).toMatch(/1–65535/)
    expect(parseTarget('   ')).toBeNull()
  })

  it('round-trips through formatTarget', () => {
    const target = parseTarget('deploy@example.com:2222/srv')
    expect(target && formatTarget(target)).toBe('sftp://deploy@example.com:2222/srv')
  })
})
