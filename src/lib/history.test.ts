import { describe, expect, it } from 'vitest'

import { canGoBack, canGoForward, emptyHistory, peek, push } from './history'

const walk = (...paths: string[]) => paths.reduce(push, emptyHistory)

describe('history', () => {
  it('has nowhere to go when empty or at the first entry', () => {
    expect(canGoBack(emptyHistory)).toBe(false)
    expect(canGoForward(emptyHistory)).toBe(false)

    const one = push(emptyHistory, '/home')
    expect(canGoBack(one)).toBe(false)
    expect(canGoForward(one)).toBe(false)
  })

  it('steps back and forward over what was visited', () => {
    const history = walk('/a', '/b', '/c')
    expect(canGoBack(history)).toBe(true)
    expect(peek(history, -1)).toBe('/b')

    const back = { ...history, at: history.at - 1 }
    expect(peek(back, -1)).toBe('/a')
    expect(peek(back, 1)).toBe('/c')
    expect(canGoForward(back)).toBe(true)
  })

  // The rule that makes forward mean anything.
  it('discards the forward trail when navigating somewhere new after going back', () => {
    const history = walk('/a', '/b', '/c')
    const back = { ...history, at: 0 }
    const branched = push(back, '/d')

    expect(branched.entries).toEqual(['/a', '/d'])
    expect(canGoForward(branched)).toBe(false)
  })

  // A refresh re-enters the same directory; back would otherwise step through
  // duplicates of where you already are.
  it('does not record re-entering the current directory', () => {
    const history = walk('/a', '/b')
    expect(push(history, '/b')).toBe(history)
  })

  it('records the same path again when it was left and returned to', () => {
    const history = walk('/a', '/b', '/a')
    expect(history.entries).toEqual(['/a', '/b', '/a'])
    expect(history.at).toBe(2)
  })

  it('stays bounded, keeping the most recent entries', () => {
    let history = emptyHistory
    for (let i = 0; i < 250; i += 1) history = push(history, `/dir-${i}`)

    expect(history.entries.length).toBeLessThanOrEqual(100)
    expect(history.entries.at(-1)).toBe('/dir-249')
    expect(history.at).toBe(history.entries.length - 1)
    expect(canGoBack(history)).toBe(true)
  })

  it('peeks nowhere past either end', () => {
    const history = walk('/a')
    expect(peek(history, -1)).toBeNull()
    expect(peek(history, 1)).toBeNull()
  })
})
