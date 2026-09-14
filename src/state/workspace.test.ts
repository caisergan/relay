import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ServerConfig, Session } from '@/ipc/gen'

import { emptyPane, useSessionsStore } from './sessionsStore'
import { currentWorkspace, restoreWorkspace } from './workspace'

const sessionOpen = vi.hoisted(() => {
  let opened = 0
  return vi.fn((_serverId: string, _startPath?: string | null) => {
    opened += 1
    return Promise.resolve(`s${opened}`)
  })
})

vi.mock('@/ipc/commands', () => ({
  commands: { sessionOpen, workspaceSet: vi.fn(() => Promise.resolve(null)) },
}))

function session(id: string, serverId: string, remotePath: string | null): Session {
  return {
    id,
    serverId,
    name: serverId,
    proto: 'sftp',
    state: { kind: 'connecting' },
    latencyMs: null,
    remotePath,
  }
}

const servers = (...ids: string[]) => ids.map((id) => ({ id }) as unknown as ServerConfig)

beforeEach(() => {
  sessionOpen.mockClear()
  useSessionsStore.setState({
    sessions: {},
    order: [],
    activeId: null,
    listings: {},
    panes: {},
    localStarts: {},
    localClaims: {},
  })
})

describe('currentWorkspace', () => {
  it('records the tabs in order, each with its folders, and which one is showing', () => {
    useSessionsStore.setState({
      sessions: { s1: session('s1', 'a', '/var/www'), s2: session('s2', 'b', null) },
      order: ['s2', 's1'],
      activeId: 's1',
      panes: { s1: { ...emptyPane, localPath: '/Users/ada/site' }, s2: emptyPane },
    })

    expect(currentWorkspace(useSessionsStore.getState())).toEqual({
      tabs: [
        { serverId: 'b', localPath: null, remotePath: null, active: false },
        { serverId: 'a', localPath: '/Users/ada/site', remotePath: '/var/www', active: true },
      ],
    })
  })
})

describe('restoreWorkspace', () => {
  it('reopens known servers in order, in their folders, showing the tab that was showing', async () => {
    const opened = await restoreWorkspace(
      {
        tabs: [
          { serverId: 'a', localPath: '/Users/ada/a', remotePath: '/srv', active: false },
          { serverId: 'deleted', localPath: '/tmp', remotePath: '/gone', active: false },
          { serverId: 'b', localPath: null, remotePath: null, active: true },
        ],
      },
      servers('a', 'b'),
    )

    expect(opened).toBe(true)
    expect(sessionOpen.mock.calls).toEqual([
      ['a', '/srv'],
      ['b', null],
    ])
    expect(useSessionsStore.getState().activeId).toBe('s2')
    const store = useSessionsStore.getState()
    expect(store.claimLocalStart('s1', 'a')).toBe('/Users/ada/a')
    expect(store.claimLocalStart('s2', 'b')).toBeNull()
  })

  // A launch that finds nothing to reopen has to know, so it can open the usual way.
  it('reports nothing opened when every saved server is gone', async () => {
    const opened = await restoreWorkspace(
      { tabs: [{ serverId: 'deleted', localPath: null, remotePath: null, active: true }] },
      servers('a'),
    )
    expect(opened).toBe(false)
    expect(sessionOpen).not.toHaveBeenCalled()
  })
})

describe('claimLocalStart', () => {
  // Two tabs of one server, and a mount effect React runs twice in development.
  it('hands each session its own folder, and the same one when it asks again', () => {
    const store = useSessionsStore.getState()
    store.queueLocalStart('a', '/one')
    store.queueLocalStart('a', '/two')

    expect(useSessionsStore.getState().claimLocalStart('s1', 'a')).toBe('/one')
    expect(useSessionsStore.getState().claimLocalStart('s1', 'a')).toBe('/one')
    expect(useSessionsStore.getState().claimLocalStart('s2', 'a')).toBe('/two')
    expect(useSessionsStore.getState().claimLocalStart('s3', 'a')).toBeNull()
  })
})
