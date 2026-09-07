import { useVirtualizer } from '@tanstack/react-virtual'
import { useRef } from 'react'

import { formatBytes, formatWhen } from '@/lib/format'

export interface FileRow {
  key: string
  name: string
  isDir: boolean
  hidden: boolean
  size: number
  modified: string | null
  perms: string | null
}

interface Props {
  rows: FileRow[]
  loading: boolean
  /** Renders the designed empty / denied states instead of an empty list. */
  emptyTitle: string
  emptyBody: string
  actionLabel: string
  onOpen: (row: FileRow) => void
  onAction: (row: FileRow) => void
  showPerms?: boolean
}

/** Virtualized from the first commit: phase 5 budgets a 10k-entry directory at 60 fps,
 * and retrofitting virtualization onto a list that grew around plain rows is worse
 * than starting with it. */
export function FileList({
  rows,
  loading,
  emptyTitle,
  emptyBody,
  actionLabel,
  onOpen,
  onAction,
  showPerms = false,
}: Props) {
  const parentRef = useRef<HTMLDivElement>(null)
  const rowHeight = readRowHeight()

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => rowHeight,
    overscan: 12,
  })

  if (loading) {
    return (
      <div className="rows" ref={parentRef}>
        {Array.from({ length: 12 }, (_, i) => (
          <div className="row" key={i}>
            <div className="skeleton" style={{ width: `${30 + ((i * 13) % 45)}%` }} />
          </div>
        ))}
      </div>
    )
  }

  if (rows.length === 0) {
    return (
      <div className="empty">
        <span className="empty__title">{emptyTitle}</span>
        <span>{emptyBody}</span>
      </div>
    )
  }

  return (
    <div className="rows" ref={parentRef}>
      <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
        {virtualizer.getVirtualItems().map((item) => {
          const row = rows[item.index]
          if (!row) return null
          return (
            <div
              key={row.key}
              className={`row${row.hidden ? ' row--hidden' : ''}`}
              style={{
                position: 'absolute',
                top: 0,
                left: 0,
                right: 0,
                height: item.size,
                transform: `translateY(${item.start}px)`,
              }}
              onDoubleClick={() => onOpen(row)}
              tabIndex={0}
              role="row"
              onKeyDown={(e) => {
                if (e.key === 'Enter') onOpen(row)
              }}
            >
              <span aria-hidden style={{ width: 14 }}>
                {row.isDir ? '▸' : '·'}
              </span>
              <span className="row__name">{row.name}</span>
              <button
                className="row__pill"
                onClick={(e) => {
                  e.stopPropagation()
                  onAction(row)
                }}
              >
                {actionLabel}
              </button>
              <span className="row__meta row__size">
                {row.isDir ? '—' : formatBytes(row.size)}
              </span>
              <span className="row__meta row__when">{formatWhen(row.modified)}</span>
              {showPerms && <span className="row__meta row__perms">{row.perms ?? '—'}</span>}
            </div>
          )
        })}
      </div>
    </div>
  )
}

/** Row height is a token, so density changes reach the virtualizer too. */
function readRowHeight(): number {
  const raw = getComputedStyle(document.documentElement).getPropertyValue('--row')
  const parsed = Number.parseInt(raw, 10)
  return Number.isFinite(parsed) && parsed > 0 ? parsed : 40
}
