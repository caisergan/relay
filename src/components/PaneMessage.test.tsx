import { act } from 'react'
import { createRoot } from 'react-dom/client'
import { describe, expect, it, vi } from 'vitest'

import { toFault } from '@/lib/errors'

import { kindForFault, PaneFault, PaneMessage } from './PaneMessage'

function render(node: React.ReactNode): HTMLDivElement {
  const host = document.createElement('div')
  document.body.appendChild(host)
  act(() => {
    createRoot(host).render(node)
  })
  return host
}

describe('kindForFault', () => {
  it('maps the two faults the design gives a pane of their own', () => {
    expect(kindForFault(toFault({ kind: 'permissionDenied', path: '/x' }))).toBe('denied')
    expect(kindForFault(toFault({ kind: 'notFound', path: '/x' }))).toBe('notFound')
  })

  // The taxonomy is deliberate: a variant with no designed pane must not borrow one.
  it('claims no designed state for a fault the panes do not act on', () => {
    expect(kindForFault(toFault({ kind: 'network', message: 'refused' }))).toBeNull()
  })
})

describe('PaneMessage', () => {
  it('draws the designed permission-denied state around the path that failed', () => {
    const host = render(<PaneMessage kind="denied" side="remote" path="/private" />)

    expect(host.querySelector('.panemsg__title')?.textContent).toBe('Permission denied')
    expect(host.querySelector('.panemsg__body')?.textContent).toContain('/private')
    expect(host.querySelector('.panemsg__icon--danger')).not.toBeNull()
  })

  it('offers Go back only when there is somewhere to go', () => {
    const back = vi.fn()
    const withBack = render(<PaneMessage kind="notFound" side="local" onBack={back} />)
    const button = withBack.querySelector<HTMLButtonElement>('.panemsg__btn')
    expect(button?.textContent).toBe('Go back')
    act(() => button?.click())
    expect(back).toHaveBeenCalledOnce()

    const withoutBack = render(<PaneMessage kind="notFound" side="local" />)
    expect(withoutBack.querySelector('.panemsg__btn')).toBeNull()
  })

  // A dropped connection returns on its own; a button would imply it could be hurried.
  it('gives the connection-lost state no button and a status role', () => {
    const host = render(<PaneMessage kind="lost" side="remote" onBack={() => undefined} />)
    expect(host.querySelector('.panemsg__btn')).toBeNull()
    expect(host.querySelector('.panemsg')?.getAttribute('role')).toBe('status')
  })

  it('separates a first connect from a dropped one', () => {
    const connecting = render(<PaneMessage kind="connecting" side="remote" />)
    expect(connecting.querySelector('.panemsg__body')?.textContent).not.toContain('dropped')

    const lost = render(<PaneMessage kind="lost" side="remote" />)
    expect(lost.querySelector('.panemsg__body')?.textContent).toContain('dropped')
  })
})

describe('PaneFault', () => {
  it('renders the designed state for a mapped fault', () => {
    const host = render(
      <PaneFault fault={toFault({ kind: 'permissionDenied', path: '/root' })} side="remote" />,
    )
    expect(host.querySelector('.panemsg__title')?.textContent).toBe('Permission denied')
  })

  // The failure this guards: routing every unmapped fault through the notFound pane
  // told the user a network error meant the folder was missing.
  it('does not label an unmapped fault as a missing folder', () => {
    const host = render(
      <PaneFault
        fault={toFault({ kind: 'network', message: 'connection refused' })}
        side="remote"
      />,
    )
    const title = host.querySelector('.panemsg__title')?.textContent
    expect(title).toBe('Could not open this folder')
    expect(title).not.toContain('not found')
    expect(host.querySelector('.panemsg__body')?.textContent).toBe('connection refused')
  })
})
