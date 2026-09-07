import { useCallback, useEffect, useMemo } from 'react'

import { commands } from '@/ipc/commands'
import type { LocalEntry, RemoteEntry } from '@/ipc/gen'
import { crumbs, joinPath, parentPath } from '@/lib/format'
import { selectOrderedJobs, useQueueStore } from '@/state/queueStore'
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
  const jobs = useQueueStore(selectOrderedJobs)
  const toast = useUiStore((s) => s.toast)

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

  const download = (row: FileRow) => {
    if (row.isDir) {
      toast('info', 'Folder transfers arrive with the phase 2 queue.')
      return
    }
    void commands
      .queueEnqueue(
        sessionId,
        'down',
        joinPath(remotePath, row.name),
        joinPath(pane.localPath, row.name),
      )
      .catch((error: unknown) => toast('error', String(error)))
  }

  const upload = (row: FileRow) => {
    if (row.isDir) {
      toast('info', 'Folder transfers arrive with the phase 2 queue.')
      return
    }
    void commands
      .queueEnqueue(
        sessionId,
        'up',
        joinPath(remotePath, row.name),
        joinPath(pane.localPath, row.name),
      )
      .catch((error: unknown) => toast('error', String(error)))
  }

  return (
    <>
      <SessionHeader sessionId={sessionId} />
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
          <PaneHeader
            path={remotePath}
            filter={pane.remoteFilter}
            onFilter={(remoteFilter) => patchPane(sessionId, { remoteFilter })}
            onNavigate={navigateRemote}
            label={session.name}
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
              showPerms
            />
          )}
        </div>
      </div>
    </>
  )
}

function SessionHeader({ sessionId }: { sessionId: string }) {
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
    </div>
  )
}

function PaneHeader({
  path,
  filter,
  onFilter,
  onNavigate,
  label,
}: {
  path: string
  filter: string
  onFilter: (value: string) => void
  onNavigate: (path: string) => void
  label: string
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
