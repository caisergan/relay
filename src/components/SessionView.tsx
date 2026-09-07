import { useCallback, useEffect, useMemo, useState } from 'react'

import { commands } from '@/ipc/commands'
import type { LocalEntry, LogLine, RemoteEntry, ServerConfig } from '@/ipc/gen'
import { crumbs, joinPath, parentPath } from '@/lib/format'
import { useOrderedJobs } from '@/state/queueStore'
import { useServersStore } from '@/state/serversStore'
import { emptyPane, useSessionsStore } from '@/state/sessionsStore'
import { useUiStore } from '@/state/uiStore'

import { FileList, sortRows, type FileRow, type Sort, type SortKey } from './FileList'
import { FlowGutter } from './FlowGutter'
import {
  IconActivity,
  IconChevronLeft,
  IconFolderPlus,
  IconMonitor,
  IconRefresh,
  IconSearch,
  IconServer,
  IconWarning,
} from './Icons'
import { avatarFor, tintFor } from './TitleBar'

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
    () =>
      sortRows(
        (listing?.entries ?? []).map(remoteRow).filter(matching(pane.remoteFilter)),
        pane.remoteSort,
      ),
    [listing, pane.remoteFilter, pane.remoteSort],
  )
  const localRows = useMemo(
    () =>
      sortRows(
        pane.localEntries.map(localRow).filter(matching(pane.localFilter)),
        pane.localSort,
      ),
    [pane.localEntries, pane.localFilter, pane.localSort],
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

  const enqueue = (direction: 'up' | 'down', name: string, isDir: boolean) => {
    if (isDir) {
      toast('info', 'Folder transfers arrive with the phase 2 queue.')
      return
    }
    void commands
      .queueEnqueue(
        sessionId,
        session.serverId,
        direction,
        joinPath(remotePath, name),
        joinPath(pane.localPath, name),
      )
      .catch((error: unknown) => toast('error', String(error)))
  }

  const transfer = (direction: 'up' | 'down') => (row: FileRow) =>
    enqueue(direction, row.name, row.isDir)

  /// A drag names files, not rows: the source pane's row objects are not in scope by
  /// the time the drop lands, so the destination looks them up in its own listing.
  const dropped = (direction: 'up' | 'down', from: FileRow[]) => (names: string[]) => {
    for (const name of names) {
      enqueue(direction, name, from.find((row) => row.name === name)?.isDir ?? false)
    }
  }

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

  /// Clicking the same column again reverses it, which is the behaviour every file
  /// manager has trained people to expect.
  const cycle = (current: Sort, key: SortKey): Sort =>
    current.key === key ? { key, dir: current.dir === 1 ? -1 : 1 } : { key, dir: 1 }

  return (
    <>
      <SessionHeader
        sessionId={sessionId}
        logOpen={logOpen}
        onToggleLog={() => setLogOpen((v) => !v)}
      />
      <div className="panes">
        <div className="pane pane--local">
          <PaneHeader
            kind="local"
            title="This Mac"
            path={pane.localPath || '/'}
            filter={pane.localFilter}
            filterLabel="Filter"
            onFilter={(localFilter) => patchPane(sessionId, { localFilter })}
            onNavigate={(path) => void loadLocal(path)}
          />
          {pane.localError ? (
            <div className="empty">
              <span className="empty__title">Cannot read this folder</span>
              <span>{pane.localError}</span>
            </div>
          ) : (
            <FileList
              pane="local"
              rows={localRows}
              loading={pane.localLoading}
              emptyTitle="Nothing here"
              emptyBody="This folder is empty."
              direction="up"
              sort={pane.localSort}
              onSort={(key) => patchPane(sessionId, { localSort: cycle(pane.localSort, key) })}
              selected={pane.localSelected}
              onSelect={(localSelected) => patchPane(sessionId, { localSelected })}
              onOpen={(row) => row.isDir && void loadLocal(joinPath(pane.localPath, row.name))}
              onAction={transfer('up')}
              {...(connected ? { onDropRows: dropped('down', remoteRows) } : {})}
            />
          )}
        </div>

        <FlowGutter jobs={sessionJobs} />

        <div className="pane pane--remote">
          {/* Spread rather than `onNewFolder={connected ? fn : undefined}`:
              `exactOptionalPropertyTypes` forbids an explicit undefined for an
              optional prop, so the prop is either present or absent. */}
          <PaneHeader
            kind="remote"
            title={session.name}
            path={remotePath}
            filter={pane.remoteFilter}
            filterLabel="Filter this folder"
            onFilter={(remoteFilter) => patchPane(sessionId, { remoteFilter })}
            onNavigate={navigateRemote}
            onRefresh={refreshRemote}
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
              pane="remote"
              rows={remoteRows}
              loading={pane.remoteLoading}
              emptyTitle="Empty directory"
              emptyBody="Nothing on the server at this path."
              direction="down"
              sort={pane.remoteSort}
              onSort={(key) =>
                patchPane(sessionId, { remoteSort: cycle(pane.remoteSort, key) })
              }
              selected={pane.remoteSelected}
              onSelect={(remoteSelected) => patchPane(sessionId, { remoteSelected })}
              onOpen={(row) => row.isDir && navigateRemote(joinPath(remotePath, row.name))}
              onAction={transfer('down')}
              onRename={renameRemote}
              onDelete={confirmDelete}
              onDropRows={dropped('up', localRows)}
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

/** The design's session header: who you are, where, over what, and how far away.
 *
 * `user@host:port` is the load-bearing part. Two panes of dotfiles look identical
 * whichever account produced them, and the name at the top is whatever the server was
 * called in the sidebar — it does not say which login is looking. */
function SessionHeader({
  sessionId,
  logOpen,
  onToggleLog,
}: {
  sessionId: string
  logOpen: boolean
  onToggleLog: () => void
}) {
  const session = useSessionsStore((s) => s.sessions[sessionId])
  const servers = useServersStore((s) => s.servers)
  const toast = useUiStore((s) => s.toast)
  if (!session) return null

  const server: ServerConfig | undefined = servers.find((s) => s.id === session.serverId)
  const state = session.state

  const conn =
    state.kind === 'connected'
      ? { label: 'Connected', dot: 'dot--ok', tone: 'var(--ok)' }
      : state.kind === 'connecting'
        ? { label: 'Connecting', dot: 'dot--busy', tone: 'var(--transit)' }
        : state.kind === 'reconnecting'
          ? {
              label: `Reconnecting (${state.attempt})`,
              dot: 'dot--busy',
              tone: 'var(--transit)',
            }
          : { label: 'Disconnected', dot: 'dot--down', tone: 'var(--danger)' }

  const lost = state.kind === 'reconnecting'

  return (
    <div className="session">
      {lost && (
        <div className="session__lost" role="status">
          <span className="spinner spinner--transit" />
          <span style={{ flex: 1 }}>
            <b>Connection lost.</b> Retrying in {state.retryInSecs}s — the queue is safe and
            resumes on its own.
          </span>
        </div>
      )}
      <div className="session__bar">
        <span
          className="session__avatar"
          style={{ background: server?.color ?? tintFor(session.name) }}
        >
          {avatarFor(session.name)}
        </span>
        <div className="session__id">
          <div className="session__line">
            <span className="session__name">{session.name}</span>
            {session.proto !== 'sftp' && (
              <span className="session__insecure">
                <IconWarning size={11} />
                Unencrypted
              </span>
            )}
          </div>
          <div className="session__meta">
            <span className={`dot ${conn.dot}`} />
            <span style={{ color: conn.tone, fontWeight: 500 }}>{conn.label}</span>
            <span>·</span>
            <span>
              {server ? `${server.username}@${server.host}:${server.port}` : session.name}
            </span>
            {state.kind === 'connected' && session.latencyMs !== null && (
              <>
                <span>·</span>
                <span>{session.latencyMs} ms</span>
              </>
            )}
            {state.kind === 'connected' && state.info.cipher && (
              <>
                <span>·</span>
                <span>{state.info.cipher}</span>
              </>
            )}
          </div>
        </div>
        <div style={{ flex: 1 }} />
        <button
          className={`pillbtn${logOpen ? ' pillbtn--on' : ''}`}
          title="Session log"
          onClick={onToggleLog}
        >
          <IconActivity size={14} />
          Activity
        </button>
        <button
          className="pillbtn pillbtn--danger"
          title="Close this session"
          onClick={() => {
            commands.sessionClose(sessionId).catch((error: unknown) => {
              toast('error', `Could not disconnect: ${String(error)}`)
            })
          }}
        >
          Disconnect
        </button>
      </div>
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
        {lines.length === 0 && <span className="row__size">Nothing logged yet.</span>}
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

/** Three rows, as designed: who this pane is, where it is, and how to narrow it.
 *
 * The single strip this replaces put an anonymous breadcrumb next to an anonymous
 * filter box, so the two panes were told apart only by their contents. */
function PaneHeader({
  kind,
  title,
  path,
  filter,
  filterLabel,
  onFilter,
  onNavigate,
  onRefresh,
  onNewFolder,
}: {
  kind: 'local' | 'remote'
  title: string
  path: string
  filter: string
  filterLabel: string
  onFilter: (value: string) => void
  onNavigate: (path: string) => void
  onRefresh?: () => void
  onNewFolder?: () => void
}) {
  return (
    <div className="pane-header">
      <div className="pane-header__id">
        {kind === 'local' ? (
          <IconMonitor size={15} className="pane-header__glyph" />
        ) : (
          <IconServer size={15} className="pane-header__glyph pane-header__glyph--signal" />
        )}
        <span className="pane-header__title">{title}</span>
        {kind === 'remote' && <span className="pane-header__tag">remote</span>}
        <div style={{ flex: 1 }} />
        {onNewFolder && (
          <button
            className="ghostbtn"
            title="New folder"
            aria-label="New folder"
            onClick={onNewFolder}
          >
            <IconFolderPlus size={14} />
          </button>
        )}
        {onRefresh && (
          <button className="ghostbtn" title="Refresh" aria-label="Refresh" onClick={onRefresh}>
            <IconRefresh size={14} />
          </button>
        )}
        <button
          className="ghostbtn"
          title="Parent directory"
          aria-label="Parent directory"
          onClick={() => onNavigate(parentPath(path))}
        >
          <IconChevronLeft size={14} />
        </button>
      </div>

      <div className="crumbs" title={`${title} — ${path}`}>
        {crumbs(path).map((crumb, index, all) => (
          <span key={crumb.path} className="crumbs__seg">
            <button
              className={`crumb${index === all.length - 1 ? ' crumb--last' : ''}`}
              onClick={() => onNavigate(crumb.path)}
            >
              {crumb.label}
            </button>
            {index < all.length - 1 && index > 0 && <span className="crumbs__sep">/</span>}
          </span>
        ))}
      </div>

      <div className="searchbox searchbox--pane">
        <IconSearch size={12} className="searchbox__icon" />
        <input
          placeholder={filterLabel}
          value={filter}
          aria-label={`${filterLabel} — ${title}`}
          onChange={(e) => onFilter(e.currentTarget.value)}
        />
      </div>
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
