/** Density. The setting changed the rows' CSS height and nothing else: the virtualiser
 * went on placing rows 40px apart, so compact rows sat in comfortable gaps and the
 * listing looked exactly as it had. */

import { act, render } from '@testing-library/react'
import { beforeAll, describe, expect, it, vi } from 'vitest'

import { useUiStore } from '@/state/uiStore'
import { giveViewport } from '@/test/viewport'

import { FileList, type FileRow } from './FileList'

beforeAll(giveViewport)

const ROWS: FileRow[] = ['a.bin', 'b.bin', 'c.bin'].map((name) => ({
  key: name,
  name,
  isDir: false,
  hidden: false,
  size: 0,
  modified: null,
  perms: null,
}))

const placement = (container: HTMLElement) =>
  [...container.querySelectorAll<HTMLElement>('.row')].map((row) => ({
    top: row.style.transform,
    height: row.style.height,
  }))

describe('density', () => {
  it('spaces the rows by the density chosen, and again when it changes', () => {
    useUiStore.setState({ density: 'comfortable' })
    const { container } = render(
      <FileList
        pane="local"
        rows={ROWS}
        loading={false}
        direction="up"
        sort={{ key: 'name', dir: 1 }}
        onSort={vi.fn()}
        selected={[]}
        onSelect={vi.fn()}
        onOpen={vi.fn()}
        onAction={vi.fn()}
      />,
    )
    expect(placement(container)).toEqual([
      { top: 'translateY(0px)', height: '40px' },
      { top: 'translateY(40px)', height: '40px' },
      { top: 'translateY(80px)', height: '40px' },
    ])

    act(() => useUiStore.setState({ density: 'compact' }))

    expect(placement(container)).toEqual([
      { top: 'translateY(0px)', height: '32px' },
      { top: 'translateY(32px)', height: '32px' },
      { top: 'translateY(64px)', height: '32px' },
    ])
  })
})
