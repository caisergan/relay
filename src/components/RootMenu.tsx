/** The breadcrumb's root menu — phase 1 §1.5's "volume/drive enumeration".
 *
 * `local_roots()` has existed in Rust since §1.5 and was exposed as a command, but
 * nothing called it, so the local pane could only reach what was under the home
 * directory. Walking up from `~` reaches `/`, but an external disk lives at
 * `/Volumes/…` and a second drive at `D:\`, and neither is an ancestor of anywhere the
 * pane starts. Without this the volumes were unreachable.
 *
 * Roots are read once per mount rather than on every open: a disk that is mounted
 * while the menu is closed appears the next time a session opens, which is the same
 * staleness the panes already have (SFTP has no directory notifications either). */

import { useEffect, useRef, useState } from 'react'

import { commands } from '@/ipc/commands'

import { IconChevronDown, IconDrive, IconHome } from './Icons'

export interface Root {
  label: string
  path: string
  kind: 'home' | 'volume'
}

/** `/Volumes/Time Machine` reads as "Time Machine". A root with no parent — `/`, or a
 * bare drive — keeps its own text: `C:` is not a shorter way of writing `C:\`, it means
 * "the current directory on C", so trimming the separator off would name a different
 * place than the one the menu navigates to. */
function labelForRoot(path: string): string {
  const trimmed = path.replace(/[/\\]+$/, '')
  if (trimmed === '') return path
  const base = trimmed.split(/[/\\]/).pop()
  // Nothing was split off, so this path has no parent segment to name it by.
  if (base === undefined || base === '' || base === trimmed) return path
  return base
}

/** Home first, then whatever the platform calls a volume. Home is not a root in the
 * Rust sense — it is the one entry people actually want, and `local_default_dir()`
 * already knows where it is. */
export async function loadRoots(): Promise<Root[]> {
  const [home, roots] = await Promise.all([
    commands.localDefaultDir().catch(() => null),
    commands.localRoots().catch(() => [] as string[]),
  ])
  const out: Root[] = []
  if (home) out.push({ label: 'Home', path: home, kind: 'home' })
  for (const path of roots) {
    if (home && path === home) continue
    out.push({ label: labelForRoot(path), path, kind: 'volume' })
  }
  return out
}

interface Props {
  roots: Root[]
  /** The path the pane is showing, so the menu can mark where you already are. */
  current: string
  onPick: (path: string) => void
}

export function RootMenu({ roots, current, onPick }: Props) {
  const [open, setOpen] = useState(false)
  const wrap = useRef<HTMLDivElement>(null)

  // Close on an outside click or Escape. `pointerdown` rather than `click`, so the
  // menu is gone before the click lands on whatever is underneath it.
  useEffect(() => {
    if (!open) return
    const onPointer = (event: PointerEvent) => {
      if (!wrap.current?.contains(event.target as Node)) setOpen(false)
    }
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false)
    }
    document.addEventListener('pointerdown', onPointer)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('pointerdown', onPointer)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  return (
    <div className="rootmenu" ref={wrap}>
      <button
        className="rootmenu__btn"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label="Volumes and drives"
        title="Volumes and drives"
        onClick={() => setOpen((v) => !v)}
      >
        <IconDrive size={12} />
        <IconChevronDown size={10} />
      </button>
      {open && (
        <div className="rootmenu__pop" role="menu">
          {roots.length === 0 && <div className="rootmenu__empty">No volumes found.</div>}
          {roots.map((root) => {
            const on = current === root.path
            return (
              <button
                key={root.path}
                role="menuitem"
                className={`rootmenu__item${on ? ' rootmenu__item--on' : ''}`}
                onClick={() => {
                  setOpen(false)
                  onPick(root.path)
                }}
              >
                {root.kind === 'home' ? (
                  <IconHome size={13} className="rootmenu__glyph" />
                ) : (
                  <IconDrive size={13} className="rootmenu__glyph" />
                )}
                <span className="rootmenu__label">{root.label}</span>
                {root.label !== root.path && (
                  <span className="rootmenu__path">{root.path}</span>
                )}
              </button>
            )
          })}
        </div>
      )}
    </div>
  )
}
