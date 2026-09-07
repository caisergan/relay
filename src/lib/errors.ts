/** Turning what `invoke` rejects with into something a pane can render.
 *
 * Tauri rejects with the *serialised* `EngineError`, so a caught value is the tagged
 * object from `gen.ts` — `{ kind: 'permissionDenied', path }` — not an `Error`. Passing
 * that through `String()` yields `[object Object]`, which is what the panes showed
 * before this existed. The taxonomy in `error.rs` is deliberate ("do not add a variant
 * the interface cannot act on differently"), so the interface has to read the tag. */

import type { EngineError } from '@/ipc/gen'

/** What a pane needs in order to choose a designed state: which one, about what path,
 * and a sentence to fall back to when the kind is one no pane draws specially. */
export interface Fault {
  kind: EngineError['kind'] | 'unknown'
  /** The path the failure was about, when the variant carries one. */
  path: string | null
  message: string
}

/** True for the tagged union `EngineError` serialises to. */
function isEngineError(value: unknown): value is EngineError {
  return (
    typeof value === 'object' &&
    value !== null &&
    'kind' in value &&
    typeof value.kind === 'string'
  )
}

/** The sentence shown when a fault has no designed pane of its own. Built from the
 * variant's own fields rather than `JSON.stringify`, so it reads as prose. */
function describe(error: EngineError): string {
  switch (error.kind) {
    case 'auth':
    case 'network':
    case 'protocol':
      return error.message
    case 'timeout':
      return `${error.operation} timed out after ${error.afterSecs}s.`
    case 'notFound':
      return `${error.path} does not exist.`
    case 'permissionDenied':
      return `Permission denied for ${error.path}.`
    case 'trustRejected':
      return `The host key for ${error.endpoint} was not accepted.`
    case 'cancelled':
      return 'Cancelled.'
    case 'sourceChanged':
      return `${error.path} changed since the transfer started.`
    case 'resumeUnverifiable':
      return `Resume could not be verified: ${error.reason}`
    case 'integrityMismatch':
      return `Integrity check failed: expected ${error.expected}, got ${error.actual}.`
    case 'localIo':
      return error.message
    case 'unsupported':
      return `This server does not support ${error.operation}.`
  }
}

/** The path a variant is about, or null. Only the path-carrying variants have one. */
function pathOf(error: EngineError): string | null {
  switch (error.kind) {
    case 'notFound':
    case 'permissionDenied':
    case 'sourceChanged':
      return error.path
    case 'localIo':
      return error.path
    default:
      return null
  }
}

/** Normalise anything thrown on the IPC boundary into a `Fault`.
 *
 * Non-engine rejections still happen — a channel that died, a bug in a wrapper — and
 * they must not be lost, so they arrive as `unknown` with their text preserved. */
export function toFault(error: unknown): Fault {
  if (isEngineError(error)) {
    return { kind: error.kind, path: pathOf(error), message: describe(error) }
  }
  if (error instanceof Error) {
    return { kind: 'unknown', path: null, message: error.message }
  }
  return { kind: 'unknown', path: null, message: String(error) }
}

/** For toasts and other one-line reports, where the kind is not acted on. */
export function faultText(error: unknown): string {
  return toFault(error).message
}
