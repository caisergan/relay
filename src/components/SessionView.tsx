import { useCallback, useEffect, useMemo, useState } from 'react'

import { commands } from '@/ipc/commands'
import type { LocalEntry, LogLine, RemoteEntry } from '@/ipc/gen'
import { crumbs, joinPath, parentPath } from '@/lib/format'
import { useOrderedJobs } from '@/state/queueStore'
import { emptyPane, useSessionsStore } from '@/state/sessionsStore'
import { useUiStore } from '@/state/uiStore'

import { FileList, type FileRow } from './FileList'
import { FlowGutter } from './FlowGutter'

interface Props {
  sessionId: string
}

export function SessionView({ sessionId }: Props) {
  const session = useSessionsStore((s) => s.sessions[sessionId])
  const listing = useSessionsStore((s) => s.listings[sessionId])
  const pane = useSessionsStore((s) => s.panes[sessionId] ?? emptyPane)
  const patchPane = useSessionsStore((s) => s.patchPane)
  const jobs = useOrderedJobs()
  const toast = useUiStore((s) => s.toast)
  // Deleting is the one remote action with no undo, so it gets a real confirmation
  // rather than a `window.confirm` the user can dismiss by muscle memory.
  const [pendingDelete, setPendingDelete] = useState<FileRow | null>(null)
  const [logOpen, setLogOpen] = useState(false)

  const sessionJobs = useMemo(
    () => jobs.filter((job) => job.session === sessionId),
    [jobs, sessionId],
  )

  const loadLocal = useCallback(
    async (path: string) => {
      patchPane(sessionId, { localLoading: true, localError: null })
      try {
        const entries = await commands.localListDir(path)
        patchPane(sessionId, {
          localPath: path,
          localEntries: entries,
          localLoading: false,
        })
      } catch (error) {
        patchPane(sessionId, { localLoading: false, localError: String(error) })
      }
    },
    [patchPane, sessionId],
  )

  useEffect(() => {
    if (pane.localPath) return
    void commands.localDefaultDir().then(loadLocal)
  }, [pane.localPath, loadLocal])

  const remoteRows = useMemo(
    () => (listing?.entries ?? []).map(remoteRow).filter(matching(pane.remoteFilter)),
    [listing, pane.remoteFilter],
  )
  const localRows = useMemo(
    () => pane.localEntries.map(localRow).filter(matching(pane.localFilter)),
    [pane.localEntries, pane.localFilter],
  )

  if (!session) return null

  const remotePath = listing?.path ?? session.remotePath ?? '/'
  const connected = session.state.kind === 'connected'

  const navigateRemote = (path: string) => {
    patchPane(sessionId, { remoteLoading: true })
    commands.sessionListDir(sessionId, path).catch((error: unknown) => {
      patchPane(sessionId, { remoteLoading: false })
      toast('error', `Could not open ${path}: ${String(error)}`)
    })
  }

  const transfer = (direction: 'up' | 'down') => (row: FileRow) => {
    if (row.isDir) {
      toast('info', 'Folder transfers arrive with the phase 2 queue.')
      return
    }
    void commands
      .queueEnqueue(
        sessionId,
        session.serverId,
        direction,
        joinPath(remotePath, row.name),
        joinPath(pane.localPath, row.name),
      )
      .catch((error: unknown) => toast('error', String(error)))
  }
  const download = transfer('down')
  const upload = transfer('up')

  /// Refresh after a change, because SFTP has no directory notifications: what the
  /// pane shows is whatever the last listing said.
  const refreshRemote = () => navigateRemote(remotePath)

  const newFolder = () => {
    const name = window.prompt('Name for the new folder')
    if (name === null || name.trim() === '') return
    void commands
      .sessionMkdir(sessionId, joinPath(remotePath, name.trim()))
      .then(refreshRemote)
      .catch((error: unknown) =>
        toast('error', `Could not create the folder: ${String(error)}`),
      )
  }

  const renameRemote = (row: FileRow) => {
    const name = window.prompt(`Rename ${row.name} to`, row.name)
    if (name === null || name.trim() === '' || name === row.name) return
    void commands
      .sessionRename(
        sessionId,
        joinPath(remotePath, row.name),
        joinPath(remotePath, name.trim()),
      )
      .then(refreshRemote)
      .catch((error: unknown) => toast('error', `Could not rename: ${String(error)}`))
  }

  const confirmDelete = (row: FileRow) => setPendingDelete(row)

  const doDelete = (row: FileRow) => {
    setPendingDelete(null)
    void commands
      .sessionRemove(sessionId, joinPath(remotePath, row.name), row.isDir)
      .then(refreshRemote)
      .catch((error: unknown) => toast('error', `Could not delete: ${String(error)}`))
  }

  return (
    <>
      <SessionHeader sessionId={sessionId} onToggleLog={() => setLogOpen((v) => !v)} />
      <div className="panes">
        <div className="pane">
          <PaneHeader
            path={pane.localPath || '/'}
            filter={pane.localFilter}
            onFilter={(localFilter) => patchPane(sessionId, { localFilter })}
            onNavigate={(path) => void loadLocal(path)}
            label="This Mac"
          />
          {pane.localError ? (
            <div className="empty">
              <span className="empty__title">Cannot read this folder</span>
              <span>{pane.localError}</span>
            </div>
          ) : (
            <FileList
              rows={localRows}
              loading={pane.localLoading}
              emptyTitle="Nothing here"
              emptyBody="This folder is empty."
              actionLabel="Upload →"
              onOpen={(row) => row.isDir && void loadLocal(joinPath(pane.localPath, row.name))}
              onAction={upload}
            />
          )}
        </div>

        <FlowGutter jobs={sessionJobs} />

        <div className="pane">
          {/* Spread rather than `onNewFolder={connected ? fn : undefined}`:
              `exactOptionalPropertyTypes` forbids an explicit undefined for an
              optional prop, so the prop is either present or absent. */}
          <PaneHeader
            path={remotePath}
            filter={pane.remoteFilter}
            onFilter={(remoteFilter) => patchPane(sessionId, { remoteFilter })}
            onNavigate={navigateRemote}
            label={session.name}
            {...(connected ? { onNewFolder: newFolder } : {})}
          />
          {!connected ? (
            <div className="empty">
              <span className="empty__title">
                {session.state.kind === 'connecting' ? 'Connecting…' : 'Not connected'}
              </span>
              <span>
                {session.state.kind === 'disconnected'
                  ? session.state.reason
                  : 'Waiting for the server.'}
              </span>
            </div>
          ) : (
            <FileList
              rows={remoteRows}
              loading={pane.remoteLoading}
              emptyTitle="Empty directory"
              emptyBody="Nothing on the server at this path."
              actionLabel="↓ Download"
              onOpen={(row) => row.isDir && navigateRemote(joinPath(remotePath, row.name))}
              onAction={download}
              onRename={renameRemote}
              onDelete={confirmDelete}
              showPerms
            />
          )}
        </div>
      </div>
      {logOpen && <LogPanel sessionId={sessionId} onClose={() => setLogOpen(false)} />}
      {pendingDelete && (
        <div className="scrim" role="dialog" aria-modal="true">
          <div className="sheet sheet--danger">
            <h2 className="sheet__title">Delete {pendingDelete.name}?</h2>
            <p className="sheet__body">
              {pendingDelete.isDir
                ? 'The folder must be empty. This cannot be undone.'
                : 'This cannot be undone — there is no trash on the server.'}
            </p>
            <div className="sheet__actions">
              <button className="btn" onClick={() => setPendingDelete(null)}>
                Cancel
              </button>
              <button className="btn btn--danger" onClick={() => doDelete(pendingDelete)}>
                Delete
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  )
}

function SessionHeader({
  sessionId,
  onToggleLog,
}: {
  sessionId: string
  onToggleLog: () => void
}) {
  const session = useSessionsStore((s) => s.sessions[sessionId])
  if (!session) return null

  const dot =
    session.state.kind === 'connected'
      ? 'dot--ok'
      : session.state.kind === 'disconnected'
        ? 'dot--down'
        : 'dot--busy'

  const detail =
    session.state.kind === 'connected'
      ? (session.state.info.cipher ?? 'encrypted')
      : session.state.kind === 'reconnecting'
        ? `Reconnecting, attempt ${session.state.attempt}`
        : session.state.kind === 'disconnected'
          ? session.state.reason
          : 'Connecting…'

  return (
    <div className="session-header">
      <span className={`dot ${dot}`} />
      <strong style={{ fontFamily: 'var(--font-display)', fontSize: 13 }}>
        {session.name}
      </strong>
      <span style={{ color: 'var(--ink-faint)' }}>{detail}</span>
      {session.latencyMs !== null && (
        <span className="sidebar__meta">{session.latencyMs} ms</span>
      )}
      <span style={{ flex: 1 }} />
      <button className="iconbtn" title="Session log" onClick={onToggleLog}>
        Log
      </button>
    </div>
  )
}

/** The per-session protocol log, fetched on demand.
 *
 * The engine keeps a bounded backlog and redacts secrets as lines are built, not on
 * the way out — so what is copied from here is what the audit in phase 5 greps. */
function LogPanel({ sessionId, onClose }: { sessionId: string; onClose: () => void }) {
  const [lines, setLines] = useState<LogLine[]>([])
  const toast = useUiStore((s) => s.toast)

  useEffect(() => {
    let live = true
    const load = () => {
      commands
        .sessionLogs(sessionId)
        .then((next) => {
          if (live) setLines(next)
        })
        .catch(() => undefined)
    }
    load()
    const timer = setInterval(load, 1000)
    return () => {
      live = false
      clearInterval(timer)
    }
  }, [sessionId])

  const copy = () => {
    const text = lines.map((l) => `${l.at} ${l.kind} ${l.line}`).join('\n')
    navigator.clipboard
      .writeText(text)
      .then(() => toast('ok', 'Log copied.'))
      .catch(() => toast('error', 'Could not copy the log.'))
  }

  return (
    <div className="logpanel">
      <div className="logpanel__bar">
        <strong>Session log</strong>
        <span style={{ flex: 1 }} />
        <button className="iconbtn" onClick={copy}>
          Copy
        </button>
        <button className="iconbtn" onClick={onClose} aria-label="Close the log">
          ×
        </button>
      </div>
      <div className="logpanel__body">
        {lines.length === 0 && <span className="sidebar__meta">Nothing logged yet.</span>}
        {lines.map((line, index) => (
          <div key={`${line.at}-${index}`} className={`logline logline--${line.kind}`}>
            <span className="logline__at">{line.at.slice(11, 19)}</span>
            {line.line}
          </div>
        ))}
      </div>
    </div>
  )
}

function PaneHeader({
  path,
  filter,
  onFilter,
  onNavigate,
  label,
  onNewFolder,
}: {
  path: string
  filter: string
  onFilter: (value: string) => void
  onNavigate: (path: string) => void
  label: string
  onNewFolder?: () => void
}) {
  return (
    <div className="pane-header">
      <button
        className="iconbtn"
        title="Parent directory"
        aria-label="Parent directory"
        onClick={() => onNavigate(parentPath(path))}
      >
        ↑
      </button>
      <div className="crumbs" title={`${label} — ${path}`}>
        {crumbs(path).map((crumb, index, all) => (
          <button
            key={crumb.path}
            className={`crumb${index === all.length - 1 ? ' crumb--last' : ''}`}
            onClick={() => onNavigate(crumb.path)}
          >
            {crumb.label}
          </button>
        ))}
      </div>
      <input
        className="filter"
        placeholder="Filter"
        value={filter}
        aria-label={`Filter ${label}`}
        onChange={(e) => onFilter(e.currentTarget.value)}
      />
      {onNewFolder && (
        <button
          className="iconbtn"
          title="New folder"
          aria-label="New folder"
          onClick={onNewFolder}
        >
          ＋
        </button>
      )}
    </div>
  )
}

function matching(filter: string) {
  const needle = filter.trim().toLowerCase()
  return (row: FileRow) => needle === '' || row.name.toLowerCase().includes(needle)
}

function remoteRow(entry: RemoteEntry): FileRow {
  return {
    key: entry.name,
    name: entry.name,
    isDir: entry.kind === 'dir' || entry.targetKind === 'dir',
    hidden: entry.name.startsWith('.'),
    size: entry.size,
    modified: entry.modified,
    perms: entry.perms,
  }
}

function localRow(entry: LocalEntry): FileRow {
  return {
    key: entry.path,
    name: entry.name,
    isDir: entry.kind === 'dir' || entry.targetKind === 'dir',
    hidden: entry.hidden,
    size: entry.size,
    modified: entry.modified,
    perms: null,
  }
}
