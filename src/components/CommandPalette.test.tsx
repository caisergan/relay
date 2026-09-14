import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ServerConfig, Session } from '@/ipc/gen'
import { useServersStore } from '@/state/serversStore'
import { useSessionsStore } from '@/state/sessionsStore'
import { useUiStore } from '@/state/uiStore'

import { CommandPalette } from './CommandPalette'

const sessionOpen = vi.hoisted(() => vi.fn((_id: string) => Promise.resolve('opened')))

vi.mock('@/ipc/commands', () => ({
  commands: {
    sessionOpen,
    sessionClose: vi.fn(() => Promise.resolve(null)),
    sessionListDir: vi.fn(() => Promise.resolve([])),
  },
}))

const prod = {
  id: 'prod',
  name: 'Prod',
  host: 'prod.example.com',
  username: 'deploy',
} as unknown as ServerConfig

const connected: Session = {
  id: 'tab-1',
  serverId: 'prod',
  name: 'Prod',
  proto: 'sftp',
  state: {
    kind: 'connected',
    info: {
      banner: null,
      software: null,
      kex: null,
      cipher: null,
      mac: null,
      hostKeyAlgo: null,
      hostKeySha256: null,
      homePath: '/home/deploy',
    },
  },
  latencyMs: null,
  remotePath: '/home/deploy',
}

beforeEach(() => {
  sessionOpen.mockClear()
  useServersStore.setState({ servers: [prod] })
  useSessionsStore.setState({ sessions: {}, order: [], activeId: null })
  useUiStore.setState({ paletteOpen: true, settingsOpen: false, pendingGo: null })
})

const input = () => screen.getByRole('combobox')
const options = () => screen.queryAllByRole('option').map((el) => el.textContent)
const selected = () => screen.getByRole('option', { selected: true }).textContent

describe('the command palette', () => {
  it('finds a command by part of its name and runs it with Return', () => {
    render(<CommandPalette />)
    fireEvent.change(input(), { target: { value: 'sett' } })

    expect(options()).toEqual(['Open settings'])
    fireEvent.keyDown(input(), { key: 'Enter' })

    expect(useUiStore.getState().settingsOpen).toBe(true)
    expect(useUiStore.getState().paletteOpen).toBe(false)
  })

  it('moves through the list with the arrow keys', () => {
    render(<CommandPalette />)
    const first = selected()

    fireEvent.keyDown(input(), { key: 'ArrowDown' })
    expect(selected()).not.toBe(first)
    fireEvent.keyDown(input(), { key: 'ArrowUp' })
    expect(selected()).toBe(first)
  })

  it('connects to a saved server that has no tab yet', () => {
    render(<CommandPalette />)
    fireEvent.change(input(), { target: { value: 'prod' } })
    fireEvent.keyDown(input(), { key: 'Enter' })

    expect(sessionOpen).toHaveBeenCalledWith('prod')
  })

  // Dialling a second connection to a server already open would be the Sidebar bug again.
  it('switches to the tab a server already has, rather than dialling it again', () => {
    useSessionsStore.setState({ sessions: { 'tab-1': connected }, order: ['tab-1'] })
    render(<CommandPalette />)
    fireEvent.change(input(), { target: { value: 'switch' } })
    fireEvent.keyDown(input(), { key: 'Enter' })

    expect(sessionOpen).not.toHaveBeenCalled()
    expect(useSessionsStore.getState().activeId).toBe('tab-1')
  })

  it('sends a typed path to the pane it names', () => {
    useSessionsStore.setState({
      sessions: { 'tab-1': connected },
      order: ['tab-1'],
      activeId: 'tab-1',
    })
    render(<CommandPalette />)
    fireEvent.change(input(), { target: { value: '/var/log' } })

    expect(options()).toEqual(['Go to /var/logProd', 'Go to /var/logThis Mac'])
    fireEvent.keyDown(input(), { key: 'Enter' })

    expect(useUiStore.getState().pendingGo).toEqual({
      sessionId: 'tab-1',
      side: 'remote',
      text: '/var/log',
    })
  })

  it('closes on Escape without running anything', () => {
    render(<CommandPalette />)
    fireEvent.keyDown(input(), { key: 'Escape' })

    expect(useUiStore.getState().paletteOpen).toBe(false)
    expect(useUiStore.getState().settingsOpen).toBe(false)
  })

  it('says so when nothing matches', () => {
    render(<CommandPalette />)
    fireEvent.change(input(), { target: { value: 'zzzz' } })

    expect(options()).toEqual([])
    expect(screen.getByText('No matching commands')).toBeTruthy()
  })
})
