/** The connect field's live URL parser.
 *
 * A pure function so it can be tested without a webview, per phase 1 §1.6. It is
 * deliberately forgiving about what a person types and precise about what it claims
 * to have understood — a port typed without a scheme infers the protocol, and that
 * inference is reported rather than assumed. */

import type { Proto } from '@/ipc/gen'

export interface ParsedTarget {
  proto: Proto
  /** True when the protocol came from the port number rather than the text. */
  protoInferred: boolean
  username: string | null
  host: string
  port: number
  portExplicit: boolean
  path: string | null
  /** Set when the input cannot be connected to at all. */
  error: string | null
  /** Set when the target parses but this release cannot open it. */
  unavailable: string | null
}

const DEFAULT_PORTS: Record<Proto, number> = { sftp: 22, ftps: 21, ftp: 21 }

const SCHEMES: Record<string, Proto> = {
  sftp: 'sftp',
  ssh: 'sftp',
  ftps: 'ftps',
  ftpes: 'ftps',
  ftp: 'ftp',
}

export function parseTarget(input: string): ParsedTarget | null {
  const text = input.trim()
  if (!text) return null

  const base: ParsedTarget = {
    proto: 'sftp',
    protoInferred: true,
    username: null,
    host: '',
    port: DEFAULT_PORTS.sftp,
    portExplicit: false,
    path: null,
    error: null,
    unavailable: null,
  }

  let rest = text
  const schemeMatch = /^([a-z][a-z0-9+.-]*):\/\//i.exec(rest)
  if (schemeMatch) {
    const scheme = (schemeMatch[1] ?? '').toLowerCase()
    const proto = SCHEMES[scheme]
    if (!proto) {
      return { ...base, host: '', error: `Unknown protocol “${scheme}”` }
    }
    base.proto = proto
    base.protoInferred = false
    base.port = DEFAULT_PORTS[proto]
    rest = rest.slice(schemeMatch[0].length)
  }

  const slash = rest.indexOf('/')
  if (slash >= 0) {
    base.path = rest.slice(slash) || '/'
    rest = rest.slice(0, slash)
  }

  const at = rest.lastIndexOf('@')
  if (at >= 0) {
    const username = rest.slice(0, at)
    base.username = username || null
    rest = rest.slice(at + 1)
  }

  // Bracketed IPv6, e.g. [::1]:2222.
  const ipv6 = /^\[([^\]]+)\](?::(\d+))?$/.exec(rest)
  if (ipv6) {
    base.host = ipv6[1] ?? ''
    if (ipv6[2]) {
      base.port = Number(ipv6[2])
      base.portExplicit = true
    }
  } else {
    const [host, port, ...extra] = rest.split(':')
    base.host = host ?? ''
    if (extra.length > 0) return { ...base, error: 'Too many colons in the address' }
    if (port !== undefined && port !== '') {
      if (!/^\d+$/.test(port)) return { ...base, error: `“${port}” is not a port` }
      base.port = Number(port)
      base.portExplicit = true
    }
  }

  if (!base.host) return { ...base, error: 'Enter a host' }
  if (base.port < 1 || base.port > 65535) return { ...base, error: 'Port must be 1–65535' }

  // Only infer from the port when the text did not say. Port 21 typed without a
  // scheme almost certainly means FTP, and silently trying SFTP on it would hang.
  if (base.protoInferred && base.portExplicit) {
    if (base.port === 21) base.proto = 'ftp'
    else if (base.port === 990) base.proto = 'ftps'
  }

  if (base.proto !== 'sftp') {
    base.unavailable = `${base.proto.toUpperCase()} is not available in this release`
  }
  return base
}

export function formatTarget(target: ParsedTarget): string {
  const user = target.username ? `${target.username}@` : ''
  const host = target.host.includes(':') ? `[${target.host}]` : target.host
  const port = target.port === DEFAULT_PORTS[target.proto] ? '' : `:${target.port}`
  return `${target.proto}://${user}${host}${port}${target.path ?? ''}`
}
