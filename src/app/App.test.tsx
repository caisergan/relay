import { act } from 'react'
import { createRoot } from 'react-dom/client'
import { describe, expect, it, vi } from 'vitest'

import { App } from './App'

/** The engine is not running in a unit test; every command resolves empty. */
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  Channel: class {
    onmessage: unknown = null
  },
}))

/** Mounting the whole app, once.
 *
 * This exists because a store selector that returned a fresh array on every call sent
 * React into an infinite update loop, which threw, which unmounted the root — and the
 * only symptom was a window with nothing in it. Nothing in the type system or the
 * linter catches that; mounting does. */
describe('App', () => {
  it('mounts and renders its shell without looping', async () => {
    const errors: unknown[] = []
    const host = document.createElement('div')
    document.body.appendChild(host)

    await act(async () => {
      createRoot(host, {
        onUncaughtError: (error: unknown) => errors.push(error),
        onCaughtError: (error: unknown) => errors.push(error),
      }).render(<App />)
      // Let the mount effects' promises settle, so a rejection in the engine bridge
      // or the server load counts as a failure too.
      await Promise.resolve()
    })

    expect(errors.map(String)).toEqual([])
    expect(host.querySelector('.shell')).not.toBeNull()
    expect(host.querySelector('.connect')).not.toBeNull()
  })
})
