import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type {
  JobSnapshot,
  JobState,
  LocalEntry,
  RemoteEntry,
  ServerInfo,
  Session,
  SessionState,
} from '@/ipc/gen'

import { changesListing, mayHaveChanged, noteArrival, SETTLE_MS } from './arrivals'
import { emptyPane, useSessionsStore } from './sessionsStore'

const localListDir = vi.hoisted(() =>
  vi.fn((_path: string) => Promise.resolve([] as LocalEntry[])),
)
const sessionRelist = vi.hoisted(() =>
  vi.fn((_id: string, _path: string) => Promise.resolve(true)),
)

vi.mock('@/ipc/commands', () => ({ commands: { localListDir, sessionRelist } }))

const connected: SessionState = { kind: 'connected', info: {} as ServerInfo }

const local = (name: string): LocalEntry => ({
  name,
  path: `/Users/ada/Downloads/${name}`,
  kind: 'file',
  targetKind: null,
  size: 1,
  modified: null,
  hidden: false,
  readonly: false,
})

const remote = (name: string): RemoteEntry => ({
  name,
  kind: 'file',
  targetKind: null,
  size: 1,
  modified: null,
  perms: null,
  mode: null,
  owner: null,
  group: null,
})

/** A fresh id per test: arrivals are gathered per session, in the module, and a lane one
 * test left behind must not become another test's. */
let sessions = 0

function openTab({
  localPath = '/Users/ada/Downloads',
  localNames = ['notes.md'],
  remotePath = '/var/www',
  remoteNames = ['index.html'],
  state = connected,
}: {
  localPath?: string
  localNames?: string[]
  remotePath?: string
  remoteNames?: string[]
  state?: SessionState
} = {}): string {
  sessions += 1
  const id = `s${sessions}`
  const session: Session = {
    id,
    serverId: 'server',
    name: 'Server',
    proto: 'sftp',
    state,
    latencyMs: null,
    remotePath,
  }
  useSessionsStore.setState({
    sessions: { [id]: session },
    order: [id],
    activeId: id,
    listings: {
      [id]: {
        session: id,
        path: remotePath,
        request: 1,
        entries: remoteNames.map(remote),
        at: '',
      },
    },
    panes: { [id]: { ...emptyPane, localPath, localEntries: localNames.map(local) } },
  })
  return id
}

const done: JobState = { kind: 'done', at: '', skipped: false }

function job(
  session: string,
  direction: 'up' | 'down',
  path: string,
  { kind = 'file', state = done }: { kind?: 'file' | 'folder'; state?: JobState } = {},
): JobSnapshot {
  return {
    id: crypto.randomUUID(),
    session,
    serverId: 'server',
    kind,
    direction,
    remotePath: direction === 'up' ? path : '/var/www/source',
    localPath: direction === 'down' ? path : '/Users/ada/source',
    size: 1,
    transferred: 1,
    state,
    order: 0,
    speedBps: null,
    etaSecs: null,
    attempts: 1,
    retryAt: null,
    conflictPolicy: null,
    parent: null,
    startedAt: null,
  }
}

const pane = (id: string) => useSessionsStore.getState().panes[id] ?? emptyPane

beforeEach(() => {
  vi.useFakeTimers()
  localListDir.mockReset().mockResolvedValue([])
  sessionRelist.mockReset().mockResolvedValue(true)
})

afterEach(() => {
  vi.useRealTimers()
})

describe('changesListing', () => {
  it('counts a file landing straight in the directory, new or not', () => {
    expect(changesListing('/srv', ['a.txt'], '/srv/a.txt', false)).toBe(true)
    expect(changesListing('/srv', [], '/srv/b.txt', false)).toBe(true)
  })

  it('counts a deeper file only while the folder it came through is not a row yet', () => {
    expect(changesListing('/srv', ['site'], '/srv/site/css/app.css', false)).toBe(false)
    expect(changesListing('/srv', [], '/srv/site/css/app.css', false)).toBe(true)
  })

  it('ignores anything outside the directory, including a sibling sharing its prefix', () => {
    expect(changesListing('/srv/site', [], '/srv/other/a.txt', false)).toBe(false)
    expect(changesListing('/srv/site', [], '/srv/site-old/a.txt', false)).toBe(false)
    expect(changesListing('/srv/site', [], '/srv/site.txt', false)).toBe(false)
  })

  // Its walk made the empty directories inside it, which no file job reports.
  it('counts a finished folder for any directory within it', () => {
    expect(changesListing('/srv/site/empty', [], '/srv/site', true)).toBe(true)
    expect(changesListing('/srv/site', [], '/srv/site', true)).toBe(true)
    expect(changesListing('/srv/site', [], '/srv/site', false)).toBe(false)
  })

  it('reads the root and Windows paths', () => {
    expect(changesListing('/', ['etc'], '/notes.md', false)).toBe(true)
    expect(changesListing('C:\\Users\\ada', [], 'C:\\Users\\ada\\notes.md', false)).toBe(true)
    expect(changesListing('/srv/', [], '/srv/notes.md', false)).toBe(true)
  })
})

describe('mayHaveChanged', () => {
  it('is true once a job stops in a way that can have touched its destination', () => {
    expect(mayHaveChanged(done)).toBe(true)
    expect(mayHaveChanged({ kind: 'done', at: '', skipped: true })).toBe(false)
    expect(mayHaveChanged({ kind: 'cancelled', at: '' })).toBe(true)
    expect(mayHaveChanged({ kind: 'transferring' })).toBe(false)
    expect(mayHaveChanged({ kind: 'queued' })).toBe(false)
  })
})

describe('noteArrival', () => {
  it('lists the local pane again once a download lands in it, without a loading state', async () => {
    const id = openTab()
    localListDir.mockResolvedValueOnce([local('notes.md'), local('report.pdf')])

    noteArrival(job(id, 'down', '/Users/ada/Downloads/report.pdf'))
    expect(localListDir).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(SETTLE_MS)

    expect(localListDir).toHaveBeenCalledExactlyOnceWith('/Users/ada/Downloads')
    expect(pane(id).localEntries.map((e) => e.name)).toEqual(['notes.md', 'report.pdf'])
    expect(pane(id).localLoading).toBe(false)
    expect(sessionRelist).not.toHaveBeenCalled()
  })

  it('lists the remote pane again once an upload lands in it', async () => {
    const id = openTab()

    noteArrival(job(id, 'up', '/var/www/app.js'))
    await vi.advanceTimersByTimeAsync(SETTLE_MS)

    expect(sessionRelist).toHaveBeenCalledExactlyOnceWith(id, '/var/www')
    expect(localListDir).not.toHaveBeenCalled()
  })

  it('leaves both panes alone for a transfer that landed somewhere else', async () => {
    const id = openTab()

    noteArrival(job(id, 'down', '/Users/ada/Desktop/report.pdf'))
    noteArrival(job(id, 'up', '/etc/nginx/nginx.conf'))
    await vi.advanceTimersByTimeAsync(SETTLE_MS * 4)

    expect(localListDir).not.toHaveBeenCalled()
    expect(sessionRelist).not.toHaveBeenCalled()
  })

  it('lists once for files that finish together', async () => {
    const id = openTab()

    for (const name of ['a', 'b', 'c', 'd']) {
      noteArrival(job(id, 'up', `/var/www/${name}.js`))
    }
    await vi.advanceTimersByTimeAsync(SETTLE_MS * 4)

    expect(sessionRelist).toHaveBeenCalledOnce()
  })

  it('waits for a listing on its way instead of sending a second beside it', async () => {
    const id = openTab()
    let finish = () => undefined as unknown
    sessionRelist.mockImplementationOnce(
      () => new Promise((resolve) => (finish = () => resolve(true))),
    )

    noteArrival(job(id, 'up', '/var/www/a.js'))
    await vi.advanceTimersByTimeAsync(SETTLE_MS)
    noteArrival(job(id, 'up', '/var/www/b.js'))
    await vi.advanceTimersByTimeAsync(SETTLE_MS * 4)
    expect(sessionRelist).toHaveBeenCalledOnce()

    finish()
    await vi.advanceTimersByTimeAsync(SETTLE_MS)
    expect(sessionRelist).toHaveBeenCalledTimes(2)
  })

  it('does not ask a server that is not connected', async () => {
    const id = openTab({ state: { kind: 'reconnecting', attempt: 1, retryInSecs: 2 } })

    noteArrival(job(id, 'up', '/var/www/app.js'))
    await vi.advanceTimersByTimeAsync(SETTLE_MS * 4)

    expect(sessionRelist).not.toHaveBeenCalled()
  })

  // The file arrived while the pane was on its way into the folder it arrived in. The
  // folder's listing may have been read before the file was there.
  it('judges an arrival mid-navigation against where the pane lands', async () => {
    const id = openTab()
    useSessionsStore.getState().patchPane(id, { localLoading: true })

    noteArrival(job(id, 'down', '/Users/ada/Downloads/site/index.html'))
    noteArrival(job(id, 'down', '/Users/ada/Downloads/site/style.css'))
    await vi.advanceTimersByTimeAsync(SETTLE_MS * 4)
    expect(localListDir).not.toHaveBeenCalled()

    useSessionsStore.getState().patchPane(id, {
      localLoading: false,
      localPath: '/Users/ada/Downloads/site',
      localEntries: [local('index.html')],
    })
    await vi.advanceTimersByTimeAsync(SETTLE_MS)

    expect(localListDir).toHaveBeenCalledExactlyOnceWith('/Users/ada/Downloads/site')
  })

  it('keeps a listing the pane has moved away from off it', async () => {
    const id = openTab()
    let finish = (_entries: LocalEntry[]) => undefined as unknown
    localListDir.mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)))

    noteArrival(job(id, 'down', '/Users/ada/Downloads/report.pdf'))
    await vi.advanceTimersByTimeAsync(SETTLE_MS)
    useSessionsStore.getState().patchPane(id, {
      localPath: '/Users/ada/Desktop',
      localEntries: [local('todo.txt')],
    })
    finish([local('notes.md'), local('report.pdf')])
    await vi.advanceTimersByTimeAsync(0)

    expect(pane(id).localPath).toBe('/Users/ada/Desktop')
    expect(pane(id).localEntries.map((e) => e.name)).toEqual(['todo.txt'])
  })

  it('does not paper over a pane that could not list its directory', async () => {
    const id = openTab()
    useSessionsStore.getState().patchPane(id, {
      localError: { kind: 'unknown', path: '/Users/ada/Downloads', message: 'gone' },
    })

    noteArrival(job(id, 'down', '/Users/ada/Downloads/report.pdf'))
    await vi.advanceTimersByTimeAsync(SETTLE_MS * 4)

    expect(localListDir).not.toHaveBeenCalled()
  })
})
