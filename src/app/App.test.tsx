import { act } from 'react'
import { createRoot } from 'react-dom/client'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { Session } from '@/ipc/gen'
import { useSessionsStore } from '@/state/sessionsStore'

import { App } from './App'

/** The engine is not running in a unit test, so the commands answer from a table.
 *
 * Answering `[]` to everything was enough while the panes only listed things; it is
 * not enough now that `local_default_dir` feeds a breadcrumb. A mock that returns the
 * wrong *shape* fails somewhere far from the mock, which is the worst kind of test
 * failure to read. */
const ANSWERS: Record<string, unknown> = {
  local_default_dir: '/home/tester',
  local_list_dir: [],
  servers_list: [],
  session_logs: [],
  secrets_status: { password: false, passphrase: false },
}

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn((command: string) => Promise.resolve(ANSWERS[command] ?? [])),
  Channel: class {
    onmessage: unknown = null
  },
}))

async function mount(): Promise<{ host: HTMLDivElement; errors: unknown[] }> {
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

  return { host, errors }
}

const session: Session = {
  id: 'session-1',
  serverId: 'server-1',
  name: 'example.test',
  proto: 'sftp',
  state: { kind: 'connected', info: fakeInfo() },
  latencyMs: 12,
  remotePath: '/home/deploy',
}

function fakeInfo() {
  return {
    banner: null,
    software: 'SSH-2.0-OpenSSH_9.6',
    kex: null,
    cipher: 'chacha20-poly1305@openssh.com',
    mac: null,
    hostKeyAlgo: 'ssh-ed25519',
    hostKeySha256: null,
    homePath: '/home/deploy',
  }
}

/** Mounting the whole app, once per surface.
 *
 * This exists because a store selector that returned a fresh array on every call sent
 * React into an infinite update loop, which threw, which unmounted the root — and the
 * only symptom was a window with nothing in it. Nothing in the type system or the
 * linter catches that; mounting does. */
describe('App', () => {
  beforeEach(() => {
    useSessionsStore.setState({
      sessions: {},
      order: [],
      activeId: null,
      listings: {},
      panes: {},
    })
  })

  it('mounts and renders its shell without looping', async () => {
    const { host, errors } = await mount()

    expect(errors.map(String)).toEqual([])
    expect(host.querySelector('.shell')).not.toBeNull()
    expect(host.querySelector('.connect')).not.toBeNull()
  })

  it('renders a session with both panes, the gutter, and the queue drawer', async () => {
    useSessionsStore.getState().upsert(session)

    const { host, errors } = await mount()

    expect(errors.map(String)).toEqual([])
    expect(host.querySelector('.pane--local')).not.toBeNull()
    expect(host.querySelector('.pane--remote')).not.toBeNull()
    expect(host.querySelector('.gutter__spine')).not.toBeNull()
    expect(host.querySelector('.drawer__bar')).not.toBeNull()
    // The session bar has to name the account, not just the server: two panes of
    // dotfiles look the same whichever login produced them.
    expect(host.querySelector('.session__meta')?.textContent).toContain('Connected')
  })
})
