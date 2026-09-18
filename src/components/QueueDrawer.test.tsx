import { act } from 'react'
import { createRoot } from 'react-dom/client'
import { beforeAll, beforeEach, describe, expect, it, onTestFinished, vi } from 'vitest'

import type { JobSnapshot, JobState, QueueOp } from '@/ipc/gen'
import { emptyStats, useQueueStore } from '@/state/queueStore'
import { useUiStore, type DrawerTab } from '@/state/uiStore'
import { giveViewport } from '@/test/viewport'

import { QueueDrawer } from './QueueDrawer'

const control = vi.hoisted(() => vi.fn((_op: QueueOp) => Promise.resolve()))

// The list is virtualised, and jsdom lays nothing out: without dimensions the
// virtualiser decides no row is on screen and mounts none of them.
beforeAll(giveViewport)

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
  useQueueStore.getState().replaceAll(jobs, emptyStats)
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

describe('the figures at the end of a row', () => {
  const figures = (row: HTMLElement | undefined) =>
    [...(row?.querySelectorAll<HTMLElement>('.job__meta') ?? [])].map((el) => ({
      text: el.textContent,
      title: el.getAttribute('title'),
    }))

  it('say how long a finished transfer took and when it started', () => {
    // The start is written as a clock time only on the day it happened, and as a date on
    // any later one, so the clock is held a minute after this job rather than left on
    // whatever day the suite happens to run.
    vi.useFakeTimers({ toFake: ['Date'] })
    vi.setSystemTime(new Date('2026-09-14T10:01:00.000Z'))
    onTestFinished(() => {
      vi.useRealTimers()
    })
    const host = render(
      [
        job({
          startedAt: '2026-09-14T10:00:00.000Z',
          transferred: 1000,
          state: { kind: 'done', at: '2026-09-14T10:00:03.400Z', skipped: false },
        }),
      ],
      'completed',
    )

    const [size, took, started] = figures(rows(host)[0])
    expect(size?.text).toBe('1000 B')
    expect(took).toEqual({ text: '3.4s', title: 'Took 3.4s' })
    expect(started?.text).toMatch(/\d{1,2}:\d{2}/)
    expect(started?.title).toMatch(/^Started /)
    // Each figure carries the icon that says which one it is.
    expect(rows(host)[0]?.querySelectorAll('.job__meta svg')).toHaveLength(2)
  })

  // A folder never runs itself, so it has no start of its own to report.
  it('time a folder by the files inside it', () => {
    const host = render(
      [
        job({
          id: 'folder',
          kind: 'folder',
          startedAt: null,
          state: { kind: 'done', at: '2026-09-14T10:00:10.000Z', skipped: false },
        }),
        job({
          id: 'later',
          order: 2048,
          parent: 'folder',
          startedAt: '2026-09-14T10:00:05.000Z',
          state: { kind: 'done', at: '2026-09-14T10:00:10.000Z', skipped: false },
        }),
        job({
          id: 'first',
          order: 3072,
          parent: 'folder',
          startedAt: '2026-09-14T10:00:02.000Z',
          state: { kind: 'done', at: '2026-09-14T10:00:04.000Z', skipped: false },
        }),
      ],
      'completed',
    )

    expect(figures(rows(host)[0])[1]?.text).toBe('8.0s')
  })

  it('show speed and time left while a transfer moves', () => {
    const host = render([job()])
    const [size, speed, left] = figures(rows(host)[0])
    expect(size).toEqual({ text: '400 B', title: '400 B of 1000 B' })
    expect(speed?.text).toBe('2.0 KB/s')
    expect(left?.text).toBe('3s left')
  })

  // The complaint this follows: every finished row ended in two dashes.
  it('leave out what a job does not have, rather than drawing dashes', () => {
    const host = render([
      job({
        size: null,
        transferred: 0,
        speedBps: null,
        etaSecs: null,
        state: { kind: 'queued' },
      }),
    ])
    expect(figures(rows(host)[0]).map((figure) => figure.text)).toEqual(['', '', ''])
    expect(host.querySelector('.job')?.textContent).not.toContain('—')
  })

  // "0 B" beside a failure is a figure about nothing. When it started still says something.
  it('give a failure that moved nothing its start time and no size', () => {
    const host = render(
      [
        job({
          transferred: 0,
          startedAt: '2026-09-14T10:00:00.000Z',
          state: {
            kind: 'failed',
            error: { kind: 'notFound', path: '/home/tester/app.tar' },
            attempts: 1,
          },
        }),
      ],
      'failed',
    )
    const [size, took, started] = figures(rows(host)[0])
    expect(size?.text).toBe('')
    expect(took?.text).toBe('')
    expect(started?.title).toMatch(/^Started /)
  })
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

  it('asks once before cancelling everything, then sends it', () => {
    const host = render([job(), job({ id: 'job-2', state: { kind: 'queued' } })])
    const button = () =>
      [...host.querySelectorAll<HTMLButtonElement>('.drawer__act')].find((b) =>
        b.textContent.startsWith('Cancel'),
      )

    act(() => button()?.click())
    expect(sent(), 'the first press only arms it').toEqual([])
    expect(button()?.textContent).toBe('Cancel 2?')

    act(() => button()?.click())
    expect(sent()).toEqual([{ kind: 'cancelAll' }])
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
