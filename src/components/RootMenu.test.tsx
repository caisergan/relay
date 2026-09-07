import { act } from 'react'
import { createRoot } from 'react-dom/client'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const ANSWERS: Record<string, unknown> = {
  local_default_dir: '/Users/tester',
  local_roots: ['/', '/Volumes/Backup'],
}

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn((command: string) => Promise.resolve(ANSWERS[command] ?? [])),
  Channel: class {
    onmessage: unknown = null
  },
}))

const { loadRoots, RootMenu } = await import('./RootMenu')

function render(node: React.ReactNode): HTMLDivElement {
  const host = document.createElement('div')
  document.body.appendChild(host)
  act(() => {
    createRoot(host).render(node)
  })
  return host
}

describe('loadRoots', () => {
  beforeEach(() => {
    ANSWERS.local_default_dir = '/Users/tester'
    ANSWERS.local_roots = ['/', '/Volumes/Backup']
  })

  it('puts home first and names volumes by their basename', async () => {
    expect(await loadRoots()).toEqual([
      { label: 'Home', path: '/Users/tester', kind: 'home' },
      { label: '/', path: '/', kind: 'volume' },
      { label: 'Backup', path: '/Volumes/Backup', kind: 'volume' },
    ])
  })

  it('does not list home twice when it is also a root', async () => {
    ANSWERS.local_roots = ['/', '/Users/tester']
    const roots = await loadRoots()
    expect(roots.filter((r) => r.path === '/Users/tester')).toHaveLength(1)
  })

  // A Windows drive has no basename to fall back on; it must keep its own text.
  it('keeps a drive letter as its own label', async () => {
    ANSWERS.local_roots = ['C:\\', 'D:\\']
    ANSWERS.local_default_dir = 'C:\\Users\\tester'
    const roots = await loadRoots()
    expect(roots.map((r) => r.label)).toEqual(['Home', 'C:\\', 'D:\\'])
  })

  // The menu is an extra way to navigate, not a precondition for the pane rendering.
  it('still offers home when root enumeration fails', async () => {
    ANSWERS.local_roots = Promise.reject(new Error('no such directory'))
    const roots = await loadRoots()
    expect(roots).toEqual([{ label: 'Home', path: '/Users/tester', kind: 'home' }])
  })
})

describe('RootMenu', () => {
  const roots = [
    { label: 'Home', path: '/Users/tester', kind: 'home' as const },
    { label: 'Backup', path: '/Volumes/Backup', kind: 'volume' as const },
  ]

  it('opens on click and reports the picked path', () => {
    const onPick = vi.fn()
    const host = render(<RootMenu roots={roots} current="/Users/tester" onPick={onPick} />)

    expect(host.querySelector('.rootmenu__pop')).toBeNull()
    act(() => host.querySelector<HTMLButtonElement>('.rootmenu__btn')?.click())

    const items = host.querySelectorAll<HTMLButtonElement>('.rootmenu__item')
    expect(items).toHaveLength(2)
    // The volume is the entry that is otherwise unreachable by walking up from home.
    act(() => items[1]?.click())
    expect(onPick).toHaveBeenCalledWith('/Volumes/Backup')
    expect(host.querySelector('.rootmenu__pop')).toBeNull()
  })

  it('marks where the pane already is', () => {
    const host = render(<RootMenu roots={roots} current="/Users/tester" onPick={vi.fn()} />)
    act(() => host.querySelector<HTMLButtonElement>('.rootmenu__btn')?.click())
    expect(host.querySelectorAll('.rootmenu__item--on')).toHaveLength(1)
    expect(host.querySelector('.rootmenu__item--on')?.textContent).toContain('Home')
  })

  it('says so rather than showing an empty popup when there are no roots', () => {
    const host = render(<RootMenu roots={[]} current="/" onPick={vi.fn()} />)
    act(() => host.querySelector<HTMLButtonElement>('.rootmenu__btn')?.click())
    expect(host.querySelector('.rootmenu__empty')).not.toBeNull()
  })
})
