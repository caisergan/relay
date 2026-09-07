import { describe, expect, it } from 'vitest'

import type { EngineError } from '@/ipc/gen'

import { faultText, toFault } from './errors'

describe('toFault', () => {
  it('keeps the kind and path of a tagged engine error', () => {
    const error: EngineError = { kind: 'permissionDenied', path: '/private' }
    expect(toFault(error)).toEqual({
      kind: 'permissionDenied',
      path: '/private',
      message: 'Permission denied for /private.',
    })
  })

  it('keeps notFound distinct, because the two draw different panes', () => {
    expect(toFault({ kind: 'notFound', path: '/gone' } satisfies EngineError).kind).toBe(
      'notFound',
    )
  })

  // The regression this module exists for: `invoke` rejects with the serialised union,
  // so `String(error)` produced "[object Object]" in the pane and in every toast.
  it('never renders a tagged error as [object Object]', () => {
    const error: EngineError = { kind: 'auth', message: 'bad password' }
    expect(faultText(error)).toBe('bad password')
    expect(faultText(error)).not.toContain('object Object')
  })

  it('builds a sentence from the fields of a variant that has no single message', () => {
    const error: EngineError = { kind: 'timeout', operation: 'list', afterSecs: 30 }
    expect(faultText(error)).toBe('list timed out after 30s.')
  })

  it('reports no path for a variant that does not carry one', () => {
    expect(
      toFault({ kind: 'network', message: 'refused' } satisfies EngineError).path,
    ).toBeNull()
  })

  it('preserves a plain Error and an arbitrary throw', () => {
    expect(toFault(new Error('boom'))).toEqual({
      kind: 'unknown',
      path: null,
      message: 'boom',
    })
    expect(toFault('just a string').message).toBe('just a string')
  })

  // `{ kind: 3 }` is not the union; treating it as one would read a `kind` the panes
  // then fail to match, so it has to fall through to the unknown branch.
  it('does not mistake a non-string kind for an engine error', () => {
    expect(toFault({ kind: 3 }).kind).toBe('unknown')
  })
})
