/** The conflict sheet, which had no test at all.
 *
 * It is the one screen in Relay that asks about losing something, and every fact on it
 * is a fact someone is about to make a decision from. That makes the failures here
 * quiet ones: a heading that points at the wrong card, a timestamp rendered as a dash,
 * a difference the eye slides past — none of them throw, and all of them lead to the
 * wrong file being overwritten. */

import { cleanup, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'

import type { FileFacts, PromptRequest } from '@/ipc/gen'
import { useUiStore } from '@/state/uiStore'

import { PromptSheets } from './PromptSheets'

const LOCAL: FileFacts = {
  path: '/Users/ada/Documents/us-server.ovpn',
  size: 4900,
  modified: '2026-09-07T00:04:00Z',
  digest: null,
}

const REMOTE: FileFacts = {
  path: '/home/ada/vpn/us-server.ovpn',
  size: 4900,
  modified: '2026-09-07T00:04:00Z',
  digest: null,
}

function open(
  over: Partial<{
    local: FileFacts
    remote: FileFacts
    direction: 'up' | 'down'
    remaining: number
    resumeAllowed: boolean
  }> = {},
) {
  const request: PromptRequest = {
    id: 'p1',
    session: 's1',
    openedAt: '2026-09-07T00:00:00Z',
    prompt: {
      kind: 'conflict',
      local: LOCAL,
      remote: REMOTE,
      direction: 'down',
      remaining: 0,
      resumeAllowed: false,
      ...over,
    },
  }
  useUiStore.getState().setPrompts([request])
  return render(<PromptSheets />)
}

/** The card for one side, found by its heading. */
function card(place: string): HTMLElement {
  const el = screen.getByText(place).closest('.conflict__side')
  if (!(el instanceof HTMLElement)) throw new Error(`no card for "${place}"`)
  return el
}

beforeEach(() => {
  // Both halves matter: the store outlives a test, and so does anything already
  // mounted — a second `render` in one test leaves two sheets in the document and
  // every query then finds two of everything.
  cleanup()
  useUiStore.getState().setPrompts([])
})

describe('which copy is about to be lost', () => {
  /** The question the sheet exists to answer. Marking the wrong card is the failure
   * that loses a file, and it is invisible — both cards otherwise look the same. */
  it('marks the local copy when the transfer is a download', () => {
    open({ direction: 'down' })
    expect(card('On this Mac').className).toContain('conflict__side--doomed')
    expect(card('On the server').className).not.toContain('conflict__side--doomed')
  })

  it('marks the remote copy when the transfer is an upload', () => {
    open({ direction: 'up' })
    expect(card('On the server').className).toContain('conflict__side--doomed')
    expect(card('On this Mac').className).not.toContain('conflict__side--doomed')
  })

  it('says so in words as well as in colour', () => {
    open({ direction: 'down' })
    expect(card('On this Mac').textContent).toContain('replaced')
  })

  it('names the file being replaced, and where it is', () => {
    open({ direction: 'down' })
    expect(screen.getByRole('heading').textContent).toContain('us-server.ovpn')
    expect(screen.getByText('/Users/ada/Documents')).toBeTruthy()
  })

  it('points at the remote directory when uploading', () => {
    open({ direction: 'up' })
    expect(screen.getByText('/home/ada/vpn')).toBeTruthy()
  })
})

describe('the comparison', () => {
  it('plays down facts that match on both sides', () => {
    open()
    for (const side of ['On this Mac', 'On the server']) {
      const differing = card(side).querySelectorAll('.conflict__val--differs')
      expect(differing.length, `${side} marked a difference where there is none`).toBe(0)
    }
  })

  it('picks out a size that differs', () => {
    open({ remote: { ...REMOTE, size: 9800 } })
    // Asserted as "this row is emphasised", not as a particular string: `formatBytes`
    // is base-1024, so 9800 bytes reads as 9.6 KB, and pinning the text here would be
    // testing the formatter rather than the emphasis.
    for (const side of ['On this Mac', 'On the server']) {
      const marked = card(side).querySelectorAll('.conflict__val--differs')
      expect(marked.length, `${side} did not pick out the size`).toBe(1)
    }
  })

  it('picks out a timestamp that differs, and marks the newer one', () => {
    open({ remote: { ...REMOTE, modified: '2026-09-08T12:00:00Z' } })
    expect(card('On the server').textContent).toContain('newer')
    expect(card('On this Mac').textContent).not.toContain('newer')
  })

  /** A server that withholds mtimes would otherwise mark every file "newer" on
   * whichever side has one. */
  it('marks neither side newer when a timestamp is missing', () => {
    open({ remote: { ...REMOTE, modified: null } })
    expect(card('On the server').textContent).not.toContain('newer')
    expect(card('On this Mac').textContent).not.toContain('newer')
  })

  /** The regression this redesign was for. A missing timestamp rendered as a bare
   * dash, which reads as a rendering failure rather than as "the server did not say"
   * — and it is the fact most likely to decide the answer. */
  it('says a missing timestamp is unknown rather than drawing a dash', () => {
    open({ remote: { ...REMOTE, modified: null } })
    const server = card('On the server')
    expect(server.textContent).toContain('not known')
    expect(server.querySelector('.conflict__unknown')).not.toBeNull()
  })
})

describe('the choices', () => {
  it('offers skip, keep both and overwrite', () => {
    open()
    for (const label of ['Skip', 'Keep both', 'Overwrite']) {
      expect(screen.getByText(label), `"${label}" is missing`).toBeTruthy()
    }
  })

  /** A button that might refuse is worse than no button, so resume appears only once
   * the engine has proven it would be safe. */
  it('hides resume until the engine has allowed it', () => {
    open({ resumeAllowed: false })
    expect(screen.queryByText('Resume')).toBeNull()
  })

  it('offers resume once the engine has allowed it', () => {
    open({ resumeAllowed: true })
    expect(screen.queryByText('Resume')).not.toBeNull()
  })

  it('does not offer to answer for the rest when this is the only one', () => {
    open({ remaining: 0 })
    expect(screen.queryByText(/Do this for the other/)).toBeNull()
  })

  it('offers to answer for the rest, and says how many', () => {
    open({ remaining: 3 })
    expect(screen.getByText(/Do this for the other/).textContent).toContain('3')
  })
})
