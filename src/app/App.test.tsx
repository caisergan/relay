import { act } from 'react'
import { createRoot } from 'react-dom/client'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { JobSnapshot, Session } from '@/ipc/gen'
import { emptyStats, useQueueStore } from '@/state/queueStore'
import { useServersStore } from '@/state/serversStore'
import { useSessionsStore } from '@/state/sessionsStore'
import { useUiStore } from '@/state/uiStore'

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

const { invoke } = await import('@tauri-apps/api/core')
const invoked = invoke as unknown as ReturnType<typeof vi.fn>

function savedServer() {
  return {
    id: 'server-1',
    name: '104.197.160.37',
    host: '104.197.160.37',
    port: 22,
    proto: 'sftp',
    username: 'deploy',
    auth: { kind: 'agent' },
    color: null,
    group: null,
    initialRemotePath: null,
  }
}

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
    useQueueStore.setState({ jobs: {}, stats: emptyStats })
    useUiStore.setState({ sidebarCollapsed: false, localPanePercent: 37 })
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
    expect(host.querySelector('.drawer__bar')).not.toBeNull()
    // The session bar has to name the account, not just the server: two panes of
    // dotfiles look the same whichever login produced them.
    expect(host.querySelector('.session__meta')?.textContent).toContain('Connected')
  })

  // Both panes list nothing here, so both land on the designed empty state. This is
  // the wiring check: the component renders in isolation in PaneMessage.test.tsx, but
  // only mounting proves FileList actually reaches for it.
  it('draws the designed empty state in a pane with no rows', async () => {
    useSessionsStore.getState().upsert(session)

    const { host, errors } = await mount()

    expect(errors.map(String)).toEqual([])
    const titles = [...host.querySelectorAll('.panemsg__title')].map((n) => n.textContent)
    expect(titles).toContain('This folder is empty')
    // Scoped to the panes: `.empty` is still the sidebar's own "No servers yet".
    expect(host.querySelector('.panes .empty')).toBeNull()
  })

  // `local_roots` shipped in phase 1 §1.5 with no caller at all, so the pane could
  // only reach the home directory's subtree. The menu is how a volume is reachable.
  it('offers the breadcrumb root menu on the local pane only', async () => {
    useSessionsStore.getState().upsert(session)

    const { host } = await mount()

    expect(host.querySelectorAll('.rootmenu')).toHaveLength(1)
    expect(host.querySelector('.pane--local .rootmenu')).not.toBeNull()
    expect(host.querySelector('.pane--remote .rootmenu')).toBeNull()
  })
})

/** A job the gutter will draw, i.e. one actually moving bytes. */
function transferringJob(): JobSnapshot {
  return {
    id: 'job-1',
    session: 'session-1',
    serverId: 'server-1',
    direction: 'down',
    remotePath: '/home/deploy/app.tar',
    localPath: '/home/tester/app.tar',
    size: 1000,
    transferred: 400,
    state: { kind: 'transferring' },
    order: 0,
    speedBps: 2048,
    etaSecs: 3,
    conflictPolicy: null,
    parent: null,
    startedAt: null,
  }
}

describe('the flow gutter', () => {
  beforeEach(() => {
    useSessionsStore.setState({
      sessions: {},
      order: [],
      activeId: null,
      listings: {},
      panes: {},
    })
    useQueueStore.setState({ jobs: {}, stats: emptyStats })
  })

  // An empty 52px channel with a `0` in it is a permanent reminder of the one thing
  // the panes already make obvious.
  it('is absent when nothing is in flight', async () => {
    useSessionsStore.getState().upsert(session)

    const { host } = await mount()

    expect(host.querySelector('.gutter')).toBeNull()
  })

  it('appears with a pill once a transfer is moving', async () => {
    useSessionsStore.getState().upsert(session)
    useQueueStore.getState().upsert(transferringJob())

    const { host } = await mount()

    expect(host.querySelector('.gutter')).not.toBeNull()
    expect(host.querySelector('.gutter__spine')).not.toBeNull()
    expect(host.querySelector('.gutter__count')?.textContent).toBe('1')
    expect(host.querySelector('.pill')?.textContent).toBe('40%')
  })

  // A queued-but-not-started job would otherwise reinstate the `0` bubble: the
  // visibility rule has to match what the bubble counts.
  it('stays absent for a job that is only queued', async () => {
    useSessionsStore.getState().upsert(session)
    useQueueStore.getState().upsert({ ...transferringJob(), state: { kind: 'queued' } })

    const { host } = await mount()

    expect(host.querySelector('.gutter')).toBeNull()
  })
})

describe('the sidebar and the pane splitter', () => {
  beforeEach(() => {
    useSessionsStore.setState({
      sessions: {},
      order: [],
      activeId: null,
      listings: {},
      panes: {},
    })
    useUiStore.setState({ sidebarCollapsed: false, localPanePercent: 37 })
  })

  // A column of its own, not a button floating over the panes: the floating one
  // landed on top of the session header's avatar.
  it('collapses to a rail that still lists the servers', async () => {
    ANSWERS.servers_list = [savedServer()]
    useUiStore.setState({ sidebarCollapsed: true })

    const { host } = await mount()

    expect(host.querySelector('.sidebar')).toBeNull()
    const rail = host.querySelector('.rail')
    expect(rail).not.toBeNull()
    expect(rail?.querySelector('[aria-label="Expand sidebar"]')).not.toBeNull()
    // The servers stay one click away rather than behind an expand step.
    expect(rail?.querySelector('[aria-label="104.197.160.37"]')).not.toBeNull()
    expect(rail?.querySelector('[aria-label="New server"]')).not.toBeNull()
    ANSWERS.servers_list = []
  })

  it('shows the sidebar and no rail when expanded', async () => {
    const { host } = await mount()

    expect(host.querySelector('.sidebar')).not.toBeNull()
    expect(host.querySelector('.rail')).toBeNull()
  })

  // The width is the store's, not the stylesheet's, or the splitter could not move it.
  it('drives the local pane width from the stored percentage', async () => {
    useSessionsStore.getState().upsert(session)
    useUiStore.setState({ localPanePercent: 55 })

    const { host } = await mount()

    const local = host.querySelector<HTMLElement>('.pane--local')
    expect(local?.style.width).toBe('55%')
    expect(host.querySelector('.splitter')).not.toBeNull()
  })

  it('refuses a split that would collapse a pane to nothing', () => {
    useUiStore.getState().setLocalPanePercent(2)
    expect(useUiStore.getState().localPanePercent).toBeGreaterThan(10)

    useUiStore.getState().setLocalPanePercent(99)
    expect(useUiStore.getState().localPanePercent).toBeLessThan(90)
  })
})

describe('the listing columns', () => {
  beforeEach(() => {
    useSessionsStore.setState({
      sessions: {},
      order: [],
      activeId: null,
      listings: {},
      panes: {},
    })
    useUiStore.setState({ sidebarCollapsed: false, localPanePercent: 37 })
  })

  // Removed in favour of the breadcrumb, which reaches any ancestor in one click.
  it('offers no parent-directory button on either pane', async () => {
    useSessionsStore.getState().upsert(session)

    const { host } = await mount()

    expect(host.querySelector('[aria-label="Parent directory"]')).toBeNull()
  })

  it('gives new folder and refresh the larger hit target', async () => {
    useSessionsStore.getState().upsert(session)

    const { host } = await mount()

    for (const label of ['New folder', 'Refresh']) {
      const button = host.querySelector(`[aria-label="${label}"]`)
      expect(button, label).not.toBeNull()
      expect(button?.className).toContain('ghostbtn--lg')
    }
  })
})

describe('hiding dotfiles', () => {
  beforeEach(() => {
    useSessionsStore.setState({
      sessions: {},
      order: [],
      activeId: null,
      listings: {},
      panes: {},
    })
  })

  it('offers the toggle on both panes', async () => {
    useSessionsStore.getState().upsert(session)

    const { host } = await mount()

    // Shown by default, as the design draws them: hiding is the opt-in.
    const toggles = host.querySelectorAll('[aria-label="Hide hidden items"]')
    expect(toggles).toHaveLength(2)
    expect(host.querySelector('.pane--local [aria-label="Hide hidden items"]')).not.toBeNull()
    expect(host.querySelector('.pane--remote [aria-label="Hide hidden items"]')).not.toBeNull()
  })

  it('flips one pane without touching the other', async () => {
    useSessionsStore.getState().upsert(session)

    const { host } = await mount()
    const local = host.querySelector<HTMLButtonElement>(
      '.pane--local [aria-label="Hide hidden items"]',
    )
    act(() => local?.click())

    const pane = useSessionsStore.getState().panes['session-1']
    expect(pane?.localShowHidden).toBe(false)
    expect(pane?.remoteShowHidden).toBe(true)
    // The engaged state has to be visible, or the pane just looks short of files.
    expect(
      host.querySelector('.pane--local [aria-label="Show hidden items"]')?.className,
    ).toContain('ghostbtn--on')
  })
})

describe('pane history', () => {
  beforeEach(() => {
    useSessionsStore.setState({
      sessions: {},
      order: [],
      activeId: null,
      listings: {},
      panes: {},
    })
  })

  it('puts back and forward on both panes, disabled with nowhere to go', async () => {
    useSessionsStore.getState().upsert(session)

    const { host } = await mount()

    for (const pane of ['local', 'remote']) {
      const back = host.querySelector<HTMLButtonElement>(`.pane--${pane} .segbtn`)
      expect(back, pane).not.toBeNull()
      // Disabled, not absent: a button that appears once there is somewhere to go
      // would shift the filter box sideways while browsing.
      expect(back?.disabled, pane).toBe(true)
    }
    expect(host.querySelectorAll('.segbtn')).toHaveLength(4)
  })

  it('enables back once the pane has been somewhere', async () => {
    useSessionsStore.getState().upsert(session)
    const { host } = await mount()

    act(() => {
      useSessionsStore.getState().patchPane('session-1', {
        remoteHistory: { entries: ['/home', '/home/deploy'], at: 1 },
      })
    })

    const buttons = host.querySelectorAll<HTMLButtonElement>('.pane--remote .segbtn')
    expect(buttons[0]?.disabled).toBe(false)
    expect(buttons[1]?.disabled).toBe(true)
  })
})

describe('what the app opens on launch', () => {
  beforeEach(() => {
    invoked.mockClear()
    ANSWERS.servers_list = []
    useServersStore.setState({ servers: [], loading: false, loaded: false })
    useSessionsStore.setState({
      sessions: {},
      order: [],
      activeId: null,
      listings: {},
      panes: {},
    })
  })

  it('opens the saved server rather than the connect form', async () => {
    ANSWERS.servers_list = [savedServer()]

    const { host } = await mount()

    expect(invoked.mock.calls.filter(([command]) => command === 'session_open')).toEqual([
      ['session_open', { serverId: 'server-1' }],
    ])
    expect(host.querySelector('.sidebar')).not.toBeNull()
  })

  it('still shows the connect form when nothing is saved', async () => {
    const { host } = await mount()

    expect(invoked.mock.calls.some(([command]) => command === 'session_open')).toBe(false)
    expect(host.querySelector('.connect')).not.toBeNull()
  })

  // The trap this guards: reopening on every render would make a closed session
  // immediately come back, and there would be no way out of the window.
  it('does not reopen once a session already exists', async () => {
    ANSWERS.servers_list = [savedServer()]
    useSessionsStore.getState().upsert(session)

    await mount()

    expect(invoked.mock.calls.some(([command]) => command === 'session_open')).toBe(false)
  })
})

describe('the server avatar', () => {
  beforeEach(() => {
    ANSWERS.servers_list = []
    useServersStore.setState({ servers: [], loading: false, loaded: false })
    useSessionsStore.setState({
      sessions: {},
      order: [],
      activeId: null,
      listings: {},
      panes: {},
    })
  })

  // `104.197.160.37` used to render as "10", which names nothing: every host on the
  // subnet gives the same two characters.
  it('draws a glyph rather than digits for an address', async () => {
    ANSWERS.servers_list = [savedServer()]

    const { host } = await mount()

    const avatar = host.querySelector('.sidebar .avatar')
    expect(avatar).not.toBeNull()
    expect(avatar?.textContent).toBe('')
    expect(avatar?.querySelector('svg')).not.toBeNull()
  })

  it('keeps initials for a name someone chose', async () => {
    ANSWERS.servers_list = [{ ...savedServer(), name: 'Acme Corp' }]

    const { host } = await mount()

    expect(host.querySelector('.sidebar .avatar')?.textContent).toBe('AC')
    ANSWERS.servers_list = []
  })
})
