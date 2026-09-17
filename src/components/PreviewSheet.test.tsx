import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { Settings } from '@/ipc/gen'
import { useUiStore } from '@/state/uiStore'

import { PreviewSheet } from './PreviewSheet'

const SUBLIME = '/Applications/Sublime Text.app'

const dialogOpen = vi.hoisted(() =>
  vi.fn((_options: unknown) => Promise.resolve<unknown>(null)),
)
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: dialogOpen }))

const commands = vi.hoisted(() => ({
  settingsGet: vi.fn(() => Promise.resolve({ theme: 'system' } as unknown as Settings)),
  settingsSet: vi.fn((settings: Settings) => Promise.resolve(settings)),
  previewPrepare: vi.fn((name: string) => Promise.resolve(`/tmp/relay-previews/x/${name}`)),
  queueEnqueue: vi.fn(() => Promise.resolve(['job'])),
}))
vi.mock('@/ipc/commands', () => ({ commands }))

const file = {
  session: 's1',
  serverId: 'server',
  remotePath: '/var/www/index.html',
  name: 'index.html',
}

beforeEach(() => {
  dialogOpen.mockReset().mockResolvedValue(null)
  for (const fn of Object.values(commands)) fn.mockClear()
  useUiStore.setState({ previewAsk: file, toasts: [] })
})

afterEach(cleanup)

const button = (name: RegExp) => screen.getByRole('button', { name })

async function chooseSublime() {
  dialogOpen.mockResolvedValueOnce(SUBLIME)
  await act(async () => {
    fireEvent.click(button(/choose application/i))
    await Promise.resolve()
  })
}

describe('PreviewSheet', () => {
  it('asks about the file, and cannot open it until an application is chosen', () => {
    render(<PreviewSheet />)

    expect(screen.getByRole('heading').textContent).toBe('Open index.html with…')
    expect(button(/just this once/i)).toHaveProperty('disabled', true)
    expect(button(/always use/i)).toHaveProperty('disabled', true)
  })

  it('opens the system picker on Applications, offering only applications', async () => {
    Object.defineProperty(navigator, 'userAgent', {
      value: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 15_0)',
      configurable: true,
    })
    render(<PreviewSheet />)

    await chooseSublime()

    expect(dialogOpen).toHaveBeenCalledOnce()
    expect(dialogOpen.mock.calls[0]?.[0]).toMatchObject({
      multiple: false,
      directory: false,
      defaultPath: '/Applications',
      filters: [{ extensions: ['app'] }],
    })
    expect(screen.getByText('Sublime Text')).toBeTruthy()
    expect(button(/always use sublime text/i)).toHaveProperty('disabled', false)
  })

  it('keeps the sheet as it was when the picker is closed without a choice', async () => {
    render(<PreviewSheet />)

    await act(async () => {
      fireEvent.click(button(/choose application/i))
      await Promise.resolve()
    })

    expect(screen.getByText('No application chosen')).toBeTruthy()
    expect(useUiStore.getState().previewAsk).toEqual(file)
  })

  it('opens once without remembering the application', async () => {
    render(<PreviewSheet />)
    await chooseSublime()

    await act(async () => {
      fireEvent.click(button(/just this once/i))
      await new Promise((resolve) => setTimeout(resolve, 0))
    })

    expect(useUiStore.getState().previewAsk).toBeNull()
    expect(commands.previewPrepare).toHaveBeenCalledExactlyOnceWith('index.html')
    expect(commands.queueEnqueue).toHaveBeenCalledOnce()
    expect(commands.settingsSet).not.toHaveBeenCalled()
  })

  it('remembers the application for next time when asked to', async () => {
    render(<PreviewSheet />)
    await chooseSublime()

    await act(async () => {
      fireEvent.click(button(/always use sublime text/i))
      await new Promise((resolve) => setTimeout(resolve, 0))
    })

    expect(commands.settingsSet).toHaveBeenCalledOnce()
    expect(commands.settingsSet.mock.calls[0]?.[0]).toMatchObject({
      theme: 'system',
      previewApp: SUBLIME,
    })
    expect(commands.queueEnqueue).toHaveBeenCalledOnce()
  })

  it('does nothing on cancel', () => {
    render(<PreviewSheet />)

    fireEvent.click(button(/cancel/i))

    expect(useUiStore.getState().previewAsk).toBeNull()
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(commands.queueEnqueue).not.toHaveBeenCalled()
  })
})
