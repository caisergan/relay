import { useVirtualizer } from '@tanstack/react-virtual'
import { useRef } from 'react'

import { DefaultFolderIcon, FileIcon, getIconForFolder } from '@react-symbols/icons/utils'

import { EXTENSIONS, FOLDERS, NAMES } from '@/lib/fileIcon'
import {
  PANE_ATTR,
  ROW_DIR_ATTR,
  ROW_NAME_ATTR,
  ZONE_ATTR,
  type DropTarget,
  type RowDrag,
} from '@/lib/rowDrag'
import { formatBytes, formatWhen } from '@/lib/format'

import {
  IconArrowDown,
  IconArrowLeft,
  IconArrowRight,
  IconFolder,
  IconPencil,
  IconTrash,
} from './Icons'
import { PaneMessage } from './PaneMessage'

/** File icons are 20px in a 20px slot, filling it exactly.
 *
 * These are Miguel Solorio's Symbols icons, drawn on the same 24-unit grid as the rest
 * of Relay's own marks, so they were built to be read at this size rather than shrunk
 * down to it. They carry no `theme` prop and no `currentColor`: each is a fixed,
 * saturated brand colour chosen to hold up on a light or a dark editor background,
 * which is why nothing here has to be told which theme is showing. */
const ICON = 20

/** A folder Relay draws itself, at the size the design set. Smaller than `ICON`
 * because a stroked outline carries less internal padding than a filled glyph and
 * would otherwise tower over its neighbours. */
const FOLDER_ICON = 16

/** The icon for a directory.
 *
 * A recognised name gets the pack's folder for it — `src`, `node_modules`, `secrets`,
 * `logs` — and everything else keeps Relay's blue mark rather than the pack's grey
 * default. That is the whole reason this is not just `<FolderIcon />`: most of a
 * server's directories have names nobody has ever drawn an icon for, and the blue is
 * what says "you can go in here". Falling back to a grey outline would take that
 * signal away from the majority to give a picture to the few.
 *
 * The fallback is detected by identity rather than by rendering and comparing: the
 * library hands back an element, and its `type` is the component it chose, so this
 * costs a pointer comparison per row rather than a second render. */
function FolderRowIcon({ name }: { name: string }) {
  const look = (folderName: string) =>
    getIconForFolder({ folderName, editFolderNameData: FOLDERS, width: ICON, height: ICON })

  // Asked twice, because the library matches folder names case-sensitively while it
  // matches file names case-insensitively — so `Logs` and `Documents` find nothing
  // where `logs` and `documents` do. Capitalised directories are ordinary on a server
  // and on a Mac's home folder, and the second lookup is only reached when the first
  // has already missed.
  let chosen = look(name)
  if (chosen.type === DefaultFolderIcon) {
    const lower = name.toLowerCase()
    if (lower !== name) chosen = look(lower)
  }

  if (chosen.type === DefaultFolderIcon) {
    return <IconFolder size={FOLDER_ICON} stroke="var(--signal)" />
  }
  return chosen
}

/** The icon for an entry: a folder's, or the file pack's pick for its name. Exported
 * for the drag ghost, which shows the row being carried. */
export function RowIcon({ name, isDir }: { name: string; isDir: boolean }) {
  if (isDir) return <FolderRowIcon name={name} />
  return (
    <FileIcon
      fileName={name}
      autoAssign
      editFileExtensionData={EXTENSIONS}
      editFileNameData={NAMES}
      width={ICON}
      height={ICON}
    />
  )
}

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

/** The one permission string the design tints: readable and writable by its owner and
 * by nobody else. Matched exactly, as the design does — `rwx------` on a directory is
 * ordinary and stays faint. */
const OWNER_ONLY = 'rw-------'

/** The landing area that appears in the receiving pane while a drag is in flight.
 *
 * It exists because "drop anywhere in the pane" was invisible: the only way to learn
 * that the far pane would accept a file was to drag one over it and watch for a
 * border. This says so before the pointer gets there, and says *where* the file will
 * land, which the pane never did.
 *
 * Floating rather than in flow, so revealing it does not shove the listing upward at
 * the exact moment someone is aiming at a row in it. Translucent for the same reason:
 * the rows behind stay legible, and a folder they were aiming for stays visible and
 * droppable underneath. */
function DropZone({
  pane,
  label,
  active,
}: {
  pane: 'local' | 'remote'
  label: string
  active: boolean
}) {
  return (
    <div className={`dropzone${active ? ' dropzone--over' : ''}`} {...{ [ZONE_ATTR]: pane }}>
      <IconArrowDown size={16} />
      <span className="dropzone__label">
        Drop in <span className="dropzone__path">{label}</span>
      </span>
    </div>
  )
}

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
  /** A right-click on a row, with the pointer position to anchor the panel to. Absent
   * leaves the engine's own menu in place, which is the right fallback: a pane with
   * nothing to inspect should not swallow the gesture. */
  onInspect?: (row: FileRow, at: { x: number; y: number }) => void
  /** Whether this pane can take rows dragged from the other one. Absent while it
   * cannot — the local pane with no connection to download from, say. */
  canReceive?: boolean
  /** A press on a row that may become a drag. Absent while there is nowhere for a
   * drag from this pane to land, so nothing lifts off with no destination. */
  onDragStart?: (row: FileRow, e: React.PointerEvent) => void
  /** The drag in flight, if any, and where it would land right now.
   *
   * Shared rather than local because the destination has to reveal its drop zone the
   * moment the drag *begins*, not when the pointer finally arrives over it. A pane
   * cannot know that on its own — the press that starts it lands in the other one.
   * And the hit test that decides `over` runs on the window, not in either pane. */
  drag?: RowDrag | null
  over?: DropTarget | null
  /** The directory a plain drop lands in, written out on the drop zone so the
   * destination is a thing you read rather than a thing you assume. */
  dropLabel?: string
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
  onInspect,
  canReceive = false,
  onDragStart,
  drag = null,
  over = null,
  dropLabel,
}: Props) {
  const parentRef = useRef<HTMLDivElement>(null)
  const rowHeight = readRowHeight()

  /** This pane can take what is being dragged: it accepts drops at all, something is
   * being dragged, and it started somewhere else. Dragging within one pane is not a
   * transfer of a file onto itself, so the source pane offers no targets. */
  const receiving = canReceive && drag !== null && drag.from !== pane
  /** Where the pointer is, if it is over this pane. */
  const here = over?.pane === pane ? over : null
  /** Over the listing but not over a folder or the landing area: a drop lands in the
   * directory on show. */
  const dropping = here !== null && here.folder === null && !here.zone

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => rowHeight,
    overscan: 12,
  })

  /** Rendered by both the populated and the empty listing: an empty directory is a
   * perfectly good destination, and is in fact the one most in need of being told it
   * can receive something. */
  const zone = receiving ? (
    <DropZone pane={pane} label={dropLabel ?? 'this folder'} active={here?.zone === true} />
  ) : null

  /** Marks the listing as a target for the hit test, for exactly as long as it is one. */
  const listingProps = receiving ? { [PANE_ATTR]: pane } : {}

  /** A press on a row. Selects it, as picking something up should, and arms a drag
   * that only becomes one if the pointer travels. Not from the row's own buttons: a
   * press on "delete" is a press on "delete". */
  const press = (row: FileRow) => (e: React.PointerEvent) => {
    if (!onDragStart || e.button !== 0) return
    if (e.target instanceof Element && e.target.closest('button')) return
    onSelect(row.name)
    onDragStart(row, e)
  }

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
      <>
        <div className={`rows${dropping ? ' rows--dropping' : ''}`} {...listingProps}>
          <PaneMessage kind="empty" side={pane} {...(onBack ? { onBack } : {})} />
        </div>
        {zone}
      </>
    )
  }

  const Arrow = direction === 'up' ? IconArrowRight : IconArrowLeft

  return (
    <>
      {header}
      <div
        className={`rows${dropping ? ' rows--dropping' : ''}`}
        ref={parentRef}
        {...listingProps}
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
                }${here?.folder === row.name ? ' row--into' : ''}`}
                style={{
                  position: 'absolute',
                  top: 0,
                  left: 0,
                  right: 0,
                  height: item.size,
                  transform: `translateY(${item.start}px)`,
                }}
                {...{ [ROW_NAME_ATTR]: row.name, [ROW_DIR_ATTR]: String(row.isDir) }}
                onPointerDown={press(row)}
                onContextMenu={(e) => {
                  if (!onInspect) return
                  e.preventDefault()
                  // Selecting says which row the panel is about, the way every file
                  // manager does — the panel floats free of the listing and would
                  // otherwise be the only thing that knew.
                  onSelect(row.name)
                  onInspect(row, { x: e.clientX, y: e.clientY })
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
                  <RowIcon name={row.name} isDir={row.isDir} />
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
      {zone}
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
