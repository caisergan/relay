import { useVirtualizer } from '@tanstack/react-virtual'
import { useRef, useState } from 'react'

import { FileIcon } from '@react-symbols/icons/utils'

import { EXTENSIONS, NAMES } from '@/lib/fileIcon'
import { formatBytes, formatWhen } from '@/lib/format'

import { IconArrowLeft, IconArrowRight, IconFolder, IconPencil, IconTrash } from './Icons'
import { PaneMessage } from './PaneMessage'

/** File icons are 20px in a 20px slot, filling it exactly.
 *
 * These are Miguel Solorio's Symbols icons, drawn on the same 24-unit grid as the rest
 * of Relay's own marks, so they were built to be read at this size rather than shrunk
 * down to it. They carry no `theme` prop and no `currentColor`: each is a fixed,
 * saturated brand colour chosen to hold up on a light or a dark editor background,
 * which is why nothing here has to be told which theme is showing. */
const ICON = 20

/** Folders keep the design's own stroked mark, and do not come from the icon library.
 *
 * The library has a `folder`, but it is a page with a small folder stamped inside it —
 * at row size that reads as a document, which is the one thing a directory must never
 * look like. Ours is a folder at any size, and the blue is what says "you can go in
 * here". Smaller than `ICON` because a stroked outline carries less internal padding
 * than a filled page and would otherwise tower over its neighbours. */
const FOLDER_ICON = 16

export interface FileRow {
  key: string
  name: string
  isDir: boolean
  hidden: boolean
  size: number
  modified: string | null
  perms: string | null
}

export type SortKey = 'name' | 'size' | 'modified'
export interface Sort {
  key: SortKey
  dir: 1 | -1
}

/** Directories first, then the chosen key. Both halves of the design do this, and it
 * is what makes a 5k-entry directory navigable: the folders you might descend into
 * are always at the top, wherever the sort is pointed. */
export function sortRows(rows: FileRow[], sort: Sort): FileRow[] {
  return [...rows].sort((a, b) => {
    if (a.isDir !== b.isDir) return a.isDir ? -1 : 1
    const value = compare(a, b, sort.key)
    return value * sort.dir
  })
}

function compare(a: FileRow, b: FileRow, key: SortKey): number {
  switch (key) {
    case 'size':
      return a.size - b.size
    case 'modified':
      return (a.modified ?? '').localeCompare(b.modified ?? '')
    default:
      return a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: 'base' })
  }
}

/** The MIME type carried by a drag between panes. A private type rather than
 * `text/plain`, so a stray text drag from another app cannot start a transfer. */
export const ROW_DRAG = 'application/x-relay-rows'

/** The one permission string the design tints: readable and writable by its owner and
 * by nobody else. Matched exactly, as the design does — `rwx------` on a directory is
 * ordinary and stays faint. */
const OWNER_ONLY = 'rw-------'

/** Exported so the rule can be tested directly: the rows themselves are virtualised,
 * and a virtualiser measures its scroll container, which in jsdom is zero pixels tall
 * and therefore renders no rows at all. */
export function permsClass(perms: string | null): string {
  return perms === OWNER_ONLY ? 'row__perms row__perms--private' : 'row__perms'
}

interface Props {
  pane: 'local' | 'remote'
  rows: FileRow[]
  loading: boolean
  /** Navigates to the parent, for the empty state's "Go back". Absent at a root. */
  onBack?: () => void
  /** Which way the row's transfer button sends bytes. Sets the arrow and its tooltip. */
  direction: 'up' | 'down'
  sort: Sort
  onSort: (key: SortKey) => void
  selected: string | null
  onSelect: (name: string) => void
  onOpen: (row: FileRow) => void
  onAction: (row: FileRow) => void
  /** Remote pane only. F2 and the row's rename affordance. */
  onRename?: (row: FileRow) => void
  /** Remote pane only. Delete and Backspace both reach it, because both keys mean
   * "remove this" depending on which keyboard someone learned. */
  onDelete?: (row: FileRow) => void
  showPerms?: boolean
  /** Names dragged in from the other pane. Absent while the pane cannot receive. */
  onDropRows?: (names: string[]) => void
}

/** Virtualized from the first commit: phase 5 budgets a 10k-entry directory at 60 fps,
 * and retrofitting virtualization onto a list that grew around plain rows is worse
 * than starting with it. */
export function FileList({
  pane,
  rows,
  loading,
  onBack,
  direction,
  sort,
  onSort,
  selected,
  onSelect,
  onOpen,
  onAction,
  onRename,
  onDelete,
  showPerms = false,
  onDropRows,
}: Props) {
  const parentRef = useRef<HTMLDivElement>(null)
  const rowHeight = readRowHeight()
  const [dropping, setDropping] = useState(false)

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => rowHeight,
    overscan: 12,
  })

  const dropProps = onDropRows
    ? {
        onDragOver: (e: React.DragEvent) => {
          if (!e.dataTransfer.types.includes(ROW_DRAG)) return
          e.preventDefault()
          e.dataTransfer.dropEffect = 'copy' as const
          setDropping(true)
        },
        onDragLeave: () => setDropping(false),
        onDrop: (e: React.DragEvent) => {
          setDropping(false)
          const raw = e.dataTransfer.getData(ROW_DRAG)
          if (raw === '') return
          e.preventDefault()
          const payload = JSON.parse(raw) as { pane: string; names: string[] }
          // A drag that starts and ends in the same pane is a no-op, not a transfer
          // of a file onto itself.
          if (payload.pane === pane) return
          onDropRows(payload.names)
        },
      }
    : {}

  const header = (
    <div className="cols">
      <ColButton
        label="Name"
        active={sort.key === 'name'}
        dir={sort.dir}
        onClick={() => onSort('name')}
      />
      <ColButton
        label="Size"
        className="row__size"
        active={sort.key === 'size'}
        dir={sort.dir}
        onClick={() => onSort('size')}
      />
      <ColButton
        label="Modified"
        className="row__when"
        active={sort.key === 'modified'}
        dir={sort.dir}
        onClick={() => onSort('modified')}
      />
      {showPerms && <span className="cols__label row__perms">Perms</span>}
    </div>
  )

  if (loading) {
    return (
      <>
        {header}
        <div className="rows">
          {Array.from({ length: 12 }, (_, i) => (
            <div className="row" key={i}>
              <div className="skeleton" style={{ width: `${30 + ((i * 13) % 45)}%` }} />
            </div>
          ))}
        </div>
      </>
    )
  }

  if (rows.length === 0) {
    return (
      <div className={`rows${dropping ? ' rows--dropping' : ''}`} {...dropProps}>
        <PaneMessage kind="empty" side={pane} {...(onBack ? { onBack } : {})} />
      </div>
    )
  }

  const Arrow = direction === 'up' ? IconArrowRight : IconArrowLeft

  return (
    <>
      {header}
      <div
        className={`rows${dropping ? ' rows--dropping' : ''}`}
        ref={parentRef}
        {...dropProps}
      >
        <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
          {virtualizer.getVirtualItems().map((item) => {
            const row = rows[item.index]
            if (!row) return null
            return (
              <div
                key={row.key}
                className={`row${row.hidden ? ' row--hidden' : ''}${
                  selected === row.name ? ' row--selected' : ''
                }`}
                style={{
                  position: 'absolute',
                  top: 0,
                  left: 0,
                  right: 0,
                  height: item.size,
                  transform: `translateY(${item.start}px)`,
                }}
                draggable
                onDragStart={(e) => {
                  onSelect(row.name)
                  e.dataTransfer.setData(ROW_DRAG, JSON.stringify({ pane, names: [row.name] }))
                  e.dataTransfer.effectAllowed = 'copy'
                }}
                onClick={() => onSelect(row.name)}
                onDoubleClick={() => onOpen(row)}
                tabIndex={0}
                role="row"
                aria-selected={selected === row.name}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') onOpen(row)
                  if (e.key === 'F2' && onRename) {
                    e.preventDefault()
                    onRename(row)
                  }
                  if ((e.key === 'Delete' || e.key === 'Backspace') && onDelete) {
                    e.preventDefault()
                    onDelete(row)
                  }
                }}
              >
                <span className="row__icon">
                  {row.isDir ? (
                    <IconFolder size={FOLDER_ICON} stroke="var(--signal)" />
                  ) : (
                    <FileIcon
                      fileName={row.name}
                      autoAssign
                      editFileExtensionData={EXTENSIONS}
                      editFileNameData={NAMES}
                      width={ICON}
                      height={ICON}
                    />
                  )}
                </span>
                <span className={`row__name${row.isDir ? ' row__name--dir' : ''}`}>
                  {row.name}
                </span>
                <span className="row__size">{row.isDir ? '—' : formatBytes(row.size)}</span>
                <span className="row__when">{formatWhen(row.modified)}</span>
                {showPerms && <span className={permsClass(row.perms)}>{row.perms ?? '—'}</span>}

                {/* Floating, so it overlays the metadata columns on hover instead of
                    shoving them sideways. The design's pill: panel, hairline, lift. */}
                <span className="acts">
                  {onRename && (
                    <button
                      className="acts__btn"
                      title={`Rename ${row.name} (F2)`}
                      aria-label={`Rename ${row.name}`}
                      onClick={(e) => {
                        e.stopPropagation()
                        onRename(row)
                      }}
                    >
                      <IconPencil size={14} />
                    </button>
                  )}
                  {onDelete && (
                    <button
                      className="acts__btn acts__btn--danger"
                      title={`Delete ${row.name}`}
                      aria-label={`Delete ${row.name}`}
                      onClick={(e) => {
                        e.stopPropagation()
                        onDelete(row)
                      }}
                    >
                      <IconTrash size={14} />
                    </button>
                  )}
                  <button
                    className="acts__btn acts__btn--go"
                    title={direction === 'up' ? `Upload ${row.name}` : `Download ${row.name}`}
                    aria-label={
                      direction === 'up' ? `Upload ${row.name}` : `Download ${row.name}`
                    }
                    onClick={(e) => {
                      e.stopPropagation()
                      onAction(row)
                    }}
                  >
                    <Arrow size={14} />
                  </button>
                </span>
              </div>
            )
          })}
        </div>
      </div>
    </>
  )
}

function ColButton({
  label,
  className,
  active,
  dir,
  onClick,
}: {
  label: string
  className?: string
  active: boolean
  dir: 1 | -1
  onClick: () => void
}) {
  return (
    <button
      className={`cols__label${className ? ` ${className}` : ''}${active ? ' cols__label--on' : ''}`}
      onClick={onClick}
      aria-sort={active ? (dir > 0 ? 'ascending' : 'descending') : 'none'}
    >
      {label}
      <span className="cols__arrow">{active ? (dir > 0 ? '↑' : '↓') : ''}</span>
    </button>
  )
}

/** Row height is a token, so density changes reach the virtualizer too. */
function readRowHeight(): number {
  const raw = getComputedStyle(document.documentElement).getPropertyValue('--row')
  const parsed = Number.parseInt(raw, 10)
  return Number.isFinite(parsed) && parsed > 0 ? parsed : 40
}
