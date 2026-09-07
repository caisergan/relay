import { act } from 'react'
import { createRoot } from 'react-dom/client'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { JobSnapshot, JobState, QueueOp } from '@/ipc/gen'
import { emptyStats, useQueueStore } from '@/state/queueStore'
import { useUiStore, type DrawerTab } from '@/state/uiStore'

import { QueueDrawer } from './QueueDrawer'

const control = vi.hoisted(() => vi.fn((_op: QueueOp) => Promise.resolve()))

vi.mock('@/ipc/commands', () => ({
  commands: { queueControl: control },
}))

function job(overrides: Partial<JobSnapshot> = {}): JobSnapshot {
  return {
    id: 'job-1',
    session: 'session-1',
    serverId: 'server-1',
    kind: 'file',
    direction: 'down',
    remotePath: '/var/www/app.tar',
    localPath: '/home/tester/app.tar',
    size: 1000,
    transferred: 400,
    state: { kind: 'transferring' },
    order: 1024,
    speedBps: 2048,
    etaSecs: 3,
    attempts: 1,
    retryAt: null,
    conflictPolicy: null,
    parent: null,
    startedAt: null,
    ...overrides,
  }
}

function render(jobs: JobSnapshot[], tab: DrawerTab = 'active'): HTMLDivElement {
  useQueueStore.setState({
    jobs: Object.fromEntries(jobs.map((j) => [j.id, j])),
    stats: emptyStats,
  })
  useUiStore.setState({ drawerOpen: true, drawerTab: tab })
  const host = document.createElement('div')
  document.body.appendChild(host)
  act(() => {
    createRoot(host).render(<QueueDrawer />)
  })
  return host
}

function rows(host: HTMLElement): HTMLElement[] {
  return [...host.querySelectorAll<HTMLElement>('.job')]
}

function labelled(host: HTMLElement, label: string): HTMLButtonElement | null {
  return host.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`)
}

function sent(): QueueOp[] {
  return control.mock.calls.map(([op]) => op)
}

beforeEach(() => {
  control.mockClear()
})

describe('the queue drawer', () => {
  it('offers the control that matches the state a job is actually in', () => {
    const moving = render([job()])
    expect(labelled(moving, 'Pause this transfer')).not.toBeNull()
    expect(labelled(moving, 'Resume this transfer')).toBeNull()
    expect(labelled(moving, 'Try this transfer again')).toBeNull()

    const paused = render([job({ state: { kind: 'paused', reason: 'user' } })])
    expect(labelled(paused, 'Resume this transfer')).not.toBeNull()
    expect(labelled(paused, 'Pause this transfer')).toBeNull()
  })

  it('sends the queue operation the button says it will', () => {
    const host = render([job()])
    act(() => labelled(host, 'Pause this transfer')?.click())
    expect(sent()).toEqual([{ kind: 'pause', job: 'job-1' }])
  })

  /// Retrying is the only way out of a failure the scheduler will not retry itself.
  it('offers retry on a failed job and nothing else does', () => {
    const failed: JobState = {
      kind: 'failed',
      error: { kind: 'auth', message: 'denied' },
      attempts: 3,
    }
    const host = render([job({ state: failed })], 'failed')
    act(() => labelled(host, 'Try this transfer again')?.click())
    expect(sent()).toEqual([{ kind: 'retry', job: 'job-1' }])
  })

  /// A job waiting out its backoff is queued, but saying only "queued" hides that
  /// something went wrong and is being tried again.
  it('says a job is retrying rather than merely queued', () => {
    const host = render([
      job({
        state: { kind: 'queued' },
        attempts: 1,
        retryAt: new Date(Date.now() + 2000).toISOString(),
      }),
    ])
    expect(host.querySelector('.job__status')?.textContent).toContain('retrying')

    const plain = render([job({ state: { kind: 'queued' }, attempts: 0, retryAt: null })])
    expect(plain.querySelector('.job__status')?.textContent).toBe('queued')
  })

  it('names a pause after what a person can do about it', () => {
    const byHand = render([job({ state: { kind: 'paused', reason: 'user' } })])
    expect(byHand.querySelector('.job__status')?.textContent).toBe('paused')

    const waiting = render([job({ state: { kind: 'paused', reason: 'sessionDown' } })])
    expect(waiting.querySelector('.job__status')?.textContent).toBe('waiting for the server')
  })

  /// A skipped job succeeded at doing nothing. The success colour would say a file was
  /// transferred that deliberately was not.
  it('draws a skipped job apart from a transferred one', () => {
    const host = render(
      [
        job({ id: 'a', state: { kind: 'done', at: '2026-01-01T00:00:00Z', skipped: true } }),
        job({
          id: 'b',
          order: 2048,
          state: { kind: 'done', at: '2026-01-01T00:00:00Z', skipped: false },
        }),
      ],
      'completed',
    )

    const statuses = [...host.querySelectorAll('.job__status')].map((el) => el.textContent)
    expect(statuses).toEqual(['skipped', 'done'])
    expect(host.querySelector('.job__status--ok')).not.toBeNull()
    expect(host.querySelectorAll('.job__status--ok')).toHaveLength(1)
  })

  /// Dropping a row onto another means "go before it", which is the row above's
  /// `after`. Dropping onto the first row means the front of the queue, which is `null`
  /// rather than the id of a row that does not exist.
  it('reorders by naming the row the dragged job should follow', () => {
    const host = render([
      job({ id: 'a', order: 1024 }),
      job({ id: 'b', order: 2048 }),
      job({ id: 'c', order: 3072 }),
    ])
    const [first, , third] = rows(host)

    act(() => {
      third?.dispatchEvent(new Event('dragstart', { bubbles: true }))
    })
    act(() => {
      first?.dispatchEvent(new Event('drop', { bubbles: true }))
    })

    expect(sent()).toEqual([{ kind: 'reorder', job: 'c', after: null }])
  })

  it('drops a job after the row above the one it landed on', () => {
    const host = render([
      job({ id: 'a', order: 1024 }),
      job({ id: 'b', order: 2048 }),
      job({ id: 'c', order: 3072 }),
    ])
    const [, , third] = rows(host)
    const second = rows(host)[1]

    act(() => {
      third?.dispatchEvent(new Event('dragstart', { bubbles: true }))
    })
    act(() => {
      second?.dispatchEvent(new Event('drop', { bubbles: true }))
    })

    expect(sent()).toEqual([{ kind: 'reorder', job: 'c', after: 'a' }])
  })

  /// A child belongs to its parent's position. Dragging one out of the folder that
  /// queued it would be reordering something the user never queued directly.
  it("keeps a folder job's child where its parent put it", () => {
    const host = render([
      job({ id: 'parent', kind: 'folder', state: { kind: 'scanning' } }),
      job({ id: 'child', order: 2048, parent: 'parent' }),
    ])
    const [parent, child] = rows(host)
    expect(parent?.getAttribute('draggable')).toBe('true')
    expect(child?.getAttribute('draggable')).toBe('false')
    expect(child?.className).toContain('job--child')
  })
})
