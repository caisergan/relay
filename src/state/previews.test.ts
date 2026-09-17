import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { JobSnapshot, JobState, Settings, TransferItem } from '@/ipc/gen'

import { notePreviewState, preview, previewWith } from './previews'
import { useUiStore, type PreviewRequest } from './uiStore'

const commands = vi.hoisted(() => ({
  settingsGet: vi.fn(() => Promise.resolve({ previewApp: null } as unknown as Settings)),
  previewPrepare: vi.fn((name: string) => Promise.resolve(`/tmp/relay-previews/${name}`)),
  queueEnqueue: vi.fn((_batch: string, _items: TransferItem[]) => Promise.resolve(['job'])),
  previewOpen: vi.fn((_path: string, _app: string) => Promise.resolve(null)),
}))

vi.mock('@/ipc/commands', () => ({ commands }))

/** A fresh name per test: previews on their way are remembered by path, in the module. */
let files = 0

function request(): PreviewRequest {
  files += 1
  const name = `notes-${files}.md`
  return { session: 's1', serverId: 'server', remotePath: `/home/deploy/${name}`, name }
}

const SUBLIME = '/Applications/Sublime Text.app'

function job(localPath: string, state: JobState): JobSnapshot {
  return {
    id: 'job',
    session: 's1',
    serverId: 'server',
    kind: 'file',
    direction: 'down',
    remotePath: '/home/deploy/notes.md',
    localPath,
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

const done: JobState = { kind: 'done', at: '', skipped: false }

/** Let promise callbacks run. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0))

beforeEach(() => {
  for (const fn of Object.values(commands)) fn.mockClear()
  useUiStore.setState({ previewAsk: null, toasts: [] })
})

describe('preview', () => {
  it('downloads to a preview folder of its own with the remembered application', async () => {
    commands.settingsGet.mockResolvedValueOnce({ previewApp: SUBLIME } as unknown as Settings)
    const file = request()

    await preview(file)

    expect(commands.previewPrepare).toHaveBeenCalledExactlyOnceWith(file.name)
    expect(commands.queueEnqueue.mock.calls[0]?.[1]).toEqual([
      {
        session: 's1',
        serverId: 'server',
        direction: 'down',
        remotePath: file.remotePath,
        localPath: `/tmp/relay-previews/${file.name}`,
        isDir: false,
      },
    ])
    expect(useUiStore.getState().previewAsk).toBeNull()
  })

  it('asks which application when none is remembered, and downloads nothing yet', async () => {
    const file = request()

    await preview(file)

    expect(useUiStore.getState().previewAsk).toEqual(file)
    expect(commands.previewPrepare).not.toHaveBeenCalled()
    expect(commands.queueEnqueue).not.toHaveBeenCalled()
  })
})

describe('notePreviewState', () => {
  it('opens the file in its application once it has arrived, and only once', async () => {
    const file = request()
    const path = `/tmp/relay-previews/${file.name}`
    await previewWith(file, SUBLIME)

    notePreviewState(job(path, { kind: 'transferring' }))
    expect(commands.previewOpen).not.toHaveBeenCalled()

    notePreviewState(job(path, done))
    notePreviewState(job(path, done))
    expect(commands.previewOpen).toHaveBeenCalledExactlyOnceWith(path, SUBLIME)
  })

  // A small file can finish before the call that queued it returns.
  it('opens a file that arrived before its enqueue answered', async () => {
    const file = request()
    const path = `/tmp/relay-previews/${file.name}`
    commands.queueEnqueue.mockImplementationOnce(() => {
      notePreviewState(job(path, done))
      return Promise.resolve(['job'])
    })

    await previewWith(file, SUBLIME)

    expect(commands.previewOpen).toHaveBeenCalledExactlyOnceWith(path, SUBLIME)
  })

  it('still opens after a failure the person retried', async () => {
    const file = request()
    const path = `/tmp/relay-previews/${file.name}`
    await previewWith(file, SUBLIME)

    notePreviewState(job(path, { kind: 'failed', error: { kind: 'cancelled' }, attempts: 1 }))
    notePreviewState(job(path, done))

    expect(commands.previewOpen).toHaveBeenCalledOnce()
  })

  it('forgets a cancelled preview', async () => {
    const file = request()
    const path = `/tmp/relay-previews/${file.name}`
    await previewWith(file, SUBLIME)

    notePreviewState(job(path, { kind: 'cancelled', at: '' }))
    notePreviewState(job(path, done))

    expect(commands.previewOpen).not.toHaveBeenCalled()
  })

  it('ignores every download that is not a preview', () => {
    notePreviewState(job('/Users/ada/Downloads/notes.md', done))
    expect(commands.previewOpen).not.toHaveBeenCalled()
  })

  it('offers another application when this one could not open it', async () => {
    const file = request()
    const path = `/tmp/relay-previews/${file.name}`
    commands.previewOpen.mockRejectedValueOnce({
      kind: 'localIo',
      path: SUBLIME,
      message: 'not an application Relay can open previews with',
    })
    await previewWith(file, SUBLIME)

    notePreviewState(job(path, done))
    await settle()

    const [toast] = useUiStore.getState().toasts
    expect(toast?.kind).toBe('error')
    expect(toast?.text).toContain(`in Sublime Text`)
    toast?.action?.run()
    expect(useUiStore.getState().previewAsk).toEqual(file)
  })

  it('forgets a preview whose download could not be queued', async () => {
    const file = request()
    const path = `/tmp/relay-previews/${file.name}`
    commands.queueEnqueue.mockRejectedValueOnce(new Error('no session'))

    await expect(previewWith(file, SUBLIME)).rejects.toThrow('no session')
    notePreviewState(job(path, done))

    expect(commands.previewOpen).not.toHaveBeenCalled()
  })
})
