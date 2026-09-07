import { getCurrentWebview } from '@tauri-apps/api/webview'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'

import { commands } from '@/ipc/commands'
import type { LocalEntry, LogLine, RemoteEntry, ServerConfig, TransferItem } from '@/ipc/gen'
import { faultText, toFault } from '@/lib/errors'
import { localProperties, remoteProperties } from '@/lib/properties'
import { baseName, crumbs, joinPath, parentPath } from '@/lib/format'
import { transferPaths } from '@/lib/transfer'
import { canGoBack, canGoForward, peek, push, type History } from '@/lib/history'
import { useOrderedJobs } from '@/state/queueStore'
import { useServersStore } from '@/state/serversStore'
import { emptyPane, useSessionsStore } from '@/state/sessionsStore'
import { useUiStore } from '@/state/uiStore'

import { FileList, RowIcon, sortRows, type FileRow, type Sort, type SortKey } from './FileList'
import { FlowGutter } from './FlowGutter'
import {
  IconActivity,
  IconChevronLeft,
  IconChevronRight,
  IconDotfile,
  IconDotfileOff,
  IconFolderPlus,
  IconMonitor,
  IconRefresh,
  IconSearch,
  IconServer,
  IconWarning,
} from './Icons'
import { PaneFault, PaneMessage } from './PaneMessage'
import { Properties, useProperties, type Point } from './Properties'
import { PaneSplitter } from './PaneSplitter'
import { loadRoots, RootMenu, type Root } from './RootMenu'
import { ServerAvatar } from './ServerAvatar'
import { useRowDrag } from './useRowDrag'

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
  /** The remote pane's element, for hit-testing a drop from the operating system. */
  const remotePane = useRef<HTMLDivElement | null>(null)
  /** The failed path is remembered separately from `listing.path`, which still names
   * the last directory that actually listed. The pane needs both: one to draw the
   * denied state about, one to go back to. */
  const [remoteFailedPath, setRemoteFailedPath] = useState<string | null>(null)
  /// The splitter measures this to turn a pointer position into a percentage.
  const panesRef = useRef<HTMLDivElement>(null)
  const localPercent = useUiStore((s) => s.localPanePercent)

  const sessionJobs = useMemo(
    () => jobs.filter((job) => job.session === sessionId),
    [jobs, sessionId],
  )

  const loadLocal = useCallback(
    /// `record: false` is how back and forward replay a path without pushing it again,
    /// which would otherwise make the two buttons walk in circles.
    async (path: string, record = true) => {
      // A filter belongs to the directory it was typed in. Carried into the next one
      // it silently hides most of what is there, and the box that explains why is a
      // row above the listing where nobody looks. A refresh keeps it: same directory,
      // same question.
      const before = useSessionsStore.getState().panes[sessionId] ?? emptyPane
      patchPane(sessionId, {
        localLoading: true,
        localError: null,
        ...(path === before.localPath ? {} : { localFilter: '', localSelected: null }),
      })
      try {
        const entries = await commands.localListDir(path)
        // Read the history fresh rather than closing over it: this callback is memoised
        // and a captured history would be the one from the render that created it.
        const current = useSessionsStore.getState().panes[sessionId] ?? emptyPane
        patchPane(sessionId, {
          localPath: path,
          localEntries: entries,
          localLoading: false,
          ...(record ? { localHistory: push(current.localHistory, path) } : {}),
        })
      } catch (error) {
        // Keep the path that failed, not the one the pane is still showing: the
        // designed pane names the directory it could not read.
        patchPane(sessionId, {
          localLoading: false,
          localPath: path,
          localEntries: [],
          localError: toFault(error),
        })
      }
    },
    [patchPane, sessionId],
  )

  useEffect(() => {
    if (pane.localPath) return
    void commands.localDefaultDir().then(loadLocal)
  }, [pane.localPath, loadLocal])

  // Roots are per machine, not per session, but the menu belongs to a pane; fetching
  // once per mount keeps `local_roots` off the navigation path.
  const [roots, setRoots] = useState<Root[]>([])
  useEffect(() => {
    let live = true
    void loadRoots().then((next) => {
      if (live) setRoots(next)
    })
    return () => {
      live = false
    }
  }, [])

  const allRemoteRows = useMemo(() => (listing?.entries ?? []).map(remoteRow), [listing])
  const allLocalRows = useMemo(() => pane.localEntries.map(localRow), [pane.localEntries])

  /// Counted before the filter, so the toggle can say how much it is keeping back
  /// rather than leaving the user to wonder why a directory looks empty.
  const remoteHidden = useMemo(
    () => allRemoteRows.filter((row) => row.hidden).length,
    [allRemoteRows],
  )
  const localHidden = useMemo(
    () => allLocalRows.filter((row) => row.hidden).length,
    [allLocalRows],
  )

  const remoteRows = useMemo(
    () =>
      sortRows(
        allRemoteRows
          .filter(visible(pane.remoteShowHidden))
          .filter(matching(pane.remoteFilter)),
        pane.remoteSort,
      ),
    [allRemoteRows, pane.remoteFilter, pane.remoteSort, pane.remoteShowHidden],
  )
  const localRows = useMemo(
    () =>
      sortRows(
        allLocalRows.filter(visible(pane.localShowHidden)).filter(matching(pane.localFilter)),
        pane.localSort,
      ),
    [allLocalRows, pane.localFilter, pane.localSort, pane.localShowHidden],
  )

  // Above the early return, because hooks must run in the same order every render.
  // The values they need are read here rather than from the body below, which only
  // exists once there is a session.
  const serverId = session?.serverId ?? null
  const intoPath = listing?.path ?? session?.remotePath ?? '/'
  const ready = session?.state.kind === 'connected'

  /// Files dragged in from Finder or Explorer, dropped on the remote pane.
  ///
  /// The operating system hands over paths and a position, and nothing else — not
  /// which element was under the cursor, and not whether a path is a file or a
  /// directory. The first is answered by hit-testing the pane's own rectangle; the
  /// second is left to the engine, which can stat the path rather than guess.
  const acceptOsDrop = useCallback(
    (paths: string[], at: { x: number; y: number }) => {
      if (!serverId || !intoPath) return
      const box = remotePane.current?.getBoundingClientRect()
      if (!box) return
      // The position is in physical pixels; `getBoundingClientRect` is in CSS pixels.
      // On any display with a scale factor other than 1 — which is most of them —
      // comparing them directly puts every drop in the wrong pane.
      const x = at.x / window.devicePixelRatio
      const y = at.y / window.devicePixelRatio
      if (x < box.left || x > box.right || y < box.top || y > box.bottom) return

      const items: TransferItem[] = paths.map((path) => ({
        session: sessionId,
        serverId,
        direction: 'up',
        remotePath: joinPath(intoPath, baseName(path)),
        localPath: path,
        // The engine looks; see `TransferItem::is_dir`.
        isDir: false,
      }))
      if (items.length === 0) return
      void commands
        .queueEnqueue(crypto.randomUUID(), items)
        .catch((error: unknown) => toast('error', faultText(error)))
    },
    [sessionId, serverId, intoPath, toast],
  )

  useEffect(() => {
    if (!ready) return
    let stop: (() => void) | null = null
    let cancelled = false
    try {
      // `getCurrentWebview()` throws synchronously when there is no Tauri webview
      // behind it — a unit test, or a browser — so the whole call is guarded rather
      // than only the promise. Dragging files in is not available there, and nothing
      // else in this component depends on it.
      void getCurrentWebview()
        .onDragDropEvent((event) => {
          if (event.payload.type === 'drop') {
            acceptOsDrop(event.payload.paths, event.payload.position)
          }
        })
        .then((unlisten) => {
          // Registration is asynchronous, so an unmount can land before it returns.
          if (cancelled) unlisten()
          else stop = unlisten
        })
        .catch(() => undefined)
    } catch {
      return
    }
    return () => {
      cancelled = true
      stop?.()
    }
  }, [acceptOsDrop, ready])

  /// One gesture is one batch, so "apply to remaining" on a conflict covers the files
  /// that were dragged together and nothing else. Dropping ten files used to send ten
  /// separate requests, which left the queue with no way to tell them apart.
  ///
  /// Above the early return, on the hoisted values, because the drag hook below needs
  /// it and a hook cannot sit past a conditional return.
  const enqueue = (
    direction: 'up' | 'down',
    names: { name: string; isDir: boolean }[],
    /** A folder in the *destination* pane to land inside, rather than the directory
     * that pane is showing. Set when the drop was aimed at a folder row. */
    intoFolder?: string,
  ) => {
    if (!serverId) return
    const items: TransferItem[] = names.map((entry) => ({
      session: sessionId,
      serverId,
      direction,
      ...transferPaths({
        direction,
        remoteDir: intoPath,
        localDir: pane.localPath,
        name: entry.name,
        intoFolder,
      }),
      // A folder becomes a parent job the engine walks; its files arrive as children.
      isDir: entry.isDir,
    }))
    if (items.length === 0) return
    void commands
      .queueEnqueue(crypto.randomUUID(), items)
      .catch((error: unknown) => toast('error', faultText(error)))
  }

  /// Rows dragged from one pane to the other. Pointer-driven rather than the HTML
  /// drag model, which Tauri's native handler swallows before the page sees it; see
  /// `lib/rowDrag.ts`. The destination decides the direction: a drop on the remote
  /// pane is an upload, on the local pane a download.
  const { drag, over, ghostRef, begin } = useRowDrag((carried, target) => {
    enqueue(
      target.pane === 'remote' ? 'up' : 'down',
      carried.entries,
      target.folder ?? undefined,
    )
  })
  const lift = (from: 'local' | 'remote') => (row: FileRow, e: React.PointerEvent) =>
    begin({ from, entries: [{ name: row.name, isDir: row.isDir }] }, e)

  /// The inspector a right-click opens. Its state lives above the early return because
  /// it is a hook; the two lookups that fill it are below, where the listings are.
  const properties = useProperties()

  if (!session) return null

  const remotePath = listing?.path ?? session.remotePath ?? '/'
  const connected = session.state.kind === 'connected'

  const navigateRemote = (path: string, record = true) => {
    patchPane(sessionId, {
      remoteLoading: true,
      remoteError: null,
      ...(path === remotePath ? {} : { remoteFilter: '', remoteSelected: null }),
    })
    setRemoteFailedPath(null)
    commands
      .sessionListDir(sessionId, path)
      .then(() => {
        // Recorded on success only: a directory that refused to list is not somewhere
        // back should be able to return to.
        if (!record) return
        const current = useSessionsStore.getState().panes[sessionId] ?? emptyPane
        patchPane(sessionId, { remoteHistory: push(current.remoteHistory, path) })
      })
      .catch((error: unknown) => {
        // A failed listing is a state of the pane, not a passing notification. The toast
        // it replaces expired while the pane went on showing the previous directory, so
        // nothing on screen said the folder had not opened.
        setRemoteFailedPath(path)
        patchPane(sessionId, { remoteLoading: false, remoteError: toFault(error) })
      })
  }

  /// Stepping moves the cursor first, then replays that path without recording it.
  const stepLocal = (delta: number) => {
    const target = peek(pane.localHistory, delta)
    if (target === null) return
    patchPane(sessionId, {
      localHistory: { ...pane.localHistory, at: pane.localHistory.at + delta },
    })
    void loadLocal(target, false)
  }

  const stepRemote = (delta: number) => {
    const target = peek(pane.remoteHistory, delta)
    if (target === null) return
    patchPane(sessionId, {
      remoteHistory: { ...pane.remoteHistory, at: pane.remoteHistory.at + delta },
    })
    navigateRemote(target, false)
  }

  const transfer = (direction: 'up' | 'down') => (row: FileRow) =>
    enqueue(direction, [{ name: row.name, isDir: row.isDir }])

  /// A row knows only what the listing draws — a name, a size, a date. The entry
  /// behind it knows the rest, so the inspector is filled from that rather than from
  /// the row, and a row whose entry has since gone opens nothing at all.
  const inspectLocal = (row: FileRow, at: Point) => {
    const entry = pane.localEntries.find((item) => item.name === row.name)
    if (!entry) return
    const facts = localProperties(entry)
    // A folder has no size until something walks it; a file already told us its own.
    properties.show(facts, at, facts.isDir ? () => commands.localMeasure(entry.path) : null)
  }
  /// A remote listing names its entries but not their paths, so the directory they were
  /// read from is handed in to answer "Where".
  const inspectRemote = (row: FileRow, at: Point) => {
    const entry = (listing?.entries ?? []).find((item) => item.name === row.name)
    if (!entry) return
    const facts = remoteProperties(entry, remotePath)
    const path = joinPath(remotePath, entry.name)
    properties.show(
      facts,
      at,
      facts.isDir ? () => commands.sessionMeasure(sessionId, path) : null,
    )
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
        toast('error', `Could not create the folder: ${faultText(error)}`),
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
      .catch((error: unknown) => toast('error', `Could not rename: ${faultText(error)}`))
  }

  const confirmDelete = (row: FileRow) => setPendingDelete(row)

  const doDelete = (row: FileRow) => {
    setPendingDelete(null)
    void commands
      .sessionRemove(sessionId, joinPath(remotePath, row.name), row.isDir)
      .then(refreshRemote)
      .catch((error: unknown) => toast('error', `Could not delete: ${faultText(error)}`))
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
      <div className="panes" ref={panesRef}>
        <div className="pane pane--local" style={{ width: `${localPercent}%` }}>
          <PaneHeader
            kind="local"
            title="This Mac"
            path={pane.localPath || '/'}
            filter={pane.localFilter}
            filterLabel="Search"
            onFilter={(localFilter) => patchPane(sessionId, { localFilter })}
            onNavigate={(path) => void loadLocal(path)}
            roots={roots}
            showHidden={pane.localShowHidden}
            hiddenCount={localHidden}
            onToggleHidden={() =>
              patchPane(sessionId, { localShowHidden: !pane.localShowHidden })
            }
            history={pane.localHistory}
            onStep={stepLocal}
          />
          {pane.localError ? (
            <PaneFault
              fault={pane.localError}
              side="local"
              {...(pane.localPath && pane.localPath !== '/'
                ? { onBack: () => void loadLocal(parentPath(pane.localPath)) }
                : {})}
            />
          ) : (
            <FileList
              pane="local"
              rows={localRows}
              loading={pane.localLoading}
              direction="up"
              {...(pane.localPath && pane.localPath !== '/'
                ? { onBack: () => void loadLocal(parentPath(pane.localPath)) }
                : {})}
              sort={pane.localSort}
              onSort={(key) => patchPane(sessionId, { localSort: cycle(pane.localSort, key) })}
              selected={pane.localSelected}
              onSelect={(localSelected) => patchPane(sessionId, { localSelected })}
              onOpen={(row) => row.isDir && void loadLocal(joinPath(pane.localPath, row.name))}
              onAction={transfer('up')}
              onInspect={inspectLocal}
              {...(connected ? { canReceive: true, onDragStart: lift('local') } : {})}
              drag={drag}
              over={over}
              dropLabel={pane.localPath}
            />
          )}
        </div>

        <PaneSplitter areaRef={panesRef} />
        <FlowGutter jobs={sessionJobs} />

        <div className="pane pane--remote" ref={remotePane}>
          {/* Spread rather than `onNewFolder={connected ? fn : undefined}`:
              `exactOptionalPropertyTypes` forbids an explicit undefined for an
              optional prop, so the prop is either present or absent. */}
          <PaneHeader
            kind="remote"
            title={session.name}
            path={remotePath}
            filter={pane.remoteFilter}
            filterLabel="Search"
            onFilter={(remoteFilter) => patchPane(sessionId, { remoteFilter })}
            onNavigate={navigateRemote}
            onRefresh={refreshRemote}
            showHidden={pane.remoteShowHidden}
            hiddenCount={remoteHidden}
            onToggleHidden={() =>
              patchPane(sessionId, { remoteShowHidden: !pane.remoteShowHidden })
            }
            history={pane.remoteHistory}
            onStep={stepRemote}
            {...(connected ? { onNewFolder: newFolder } : {})}
          />
          {/* Order matters: a dropped connection explains a failed listing, so the
              connection state is drawn before the listing's own error. */}
          {!connected ? (
            session.state.kind === 'disconnected' ? (
              <PaneMessage kind="disconnected" side="remote" body={session.state.reason} />
            ) : session.state.kind === 'connecting' ? (
              <PaneMessage kind="connecting" side="remote" />
            ) : (
              <PaneMessage kind="lost" side="remote" />
            )
          ) : pane.remoteError ? (
            <PaneFault
              fault={pane.remoteError}
              side="remote"
              onBack={() => navigateRemote(parentPath(remoteFailedPath ?? remotePath))}
            />
          ) : (
            <FileList
              pane="remote"
              rows={remoteRows}
              loading={pane.remoteLoading}
              direction="down"
              {...(remotePath !== '/'
                ? { onBack: () => navigateRemote(parentPath(remotePath)) }
                : {})}
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
              onInspect={inspectRemote}
              canReceive
              onDragStart={lift('remote')}
              drag={drag}
              over={over}
              dropLabel={remotePath}
              showPerms
            />
          )}
        </div>
      </div>
      {logOpen && <LogPanel sessionId={sessionId} onClose={() => setLogOpen(false)} />}
      {properties.open && (
        <Properties
          // A fresh panel per opening, so right-clicking a second row starts its
          // measurement from scratch rather than inheriting the previous row's.
          key={properties.open.seq}
          facts={properties.open.facts}
          at={properties.open.at}
          measure={properties.open.measure}
          onClose={properties.close}
        />
      )}
      {/* The row being carried, following the pointer. In the body rather than in the
          pane, so no ancestor's transform can turn "fixed" into "relative to me". */}
      {drag &&
        createPortal(
          <div className="dragghost" ref={ghostRef} aria-hidden>
            <span className="dragghost__icon">
              <RowIcon
                name={drag.entries[0]?.name ?? ''}
                isDir={drag.entries[0]?.isDir ?? false}
              />
            </span>
            <span className="dragghost__name">{drag.entries[0]?.name}</span>
          </div>,
          document.body,
        )}
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

  // A session that gave up is still a session: its queue is paused, not lost, and the
  // same banner offers the same button. The only difference is that nothing is
  // counting down any more.
  const outage =
    state.kind === 'reconnecting'
      ? { attempt: state.attempt, retryInSecs: state.retryInSecs }
      : state.kind === 'disconnected' && state.unexpected
        ? { attempt: null, retryInSecs: null }
        : null

  return (
    <div className="session">
      {outage && (
        <div className="session__lost" role="status">
          {outage.retryInSecs !== null && <span className="spinner spinner--transit" />}
          <span style={{ flex: 1 }}>
            <b>Connection lost.</b>{' '}
            {outage.retryInSecs !== null
              ? `Retrying in ${outage.retryInSecs}s (attempt ${outage.attempt}) — the queue is paused and resumes on its own.`
              : 'The queue is paused and waiting; nothing has been lost.'}
          </span>
          <button
            className="btn btn--small"
            onClick={() => {
              commands
                .sessionReconnect(sessionId)
                .catch((error: unknown) => toast('error', faultText(error)))
            }}
          >
            Reconnect now
          </button>
        </div>
      )}
      <div className="session__bar">
        <ServerAvatar name={session.name} color={server?.color ?? null} size={30} />
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
              toast('error', `Could not disconnect: ${faultText(error)}`)
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
  roots,
  showHidden,
  hiddenCount,
  onToggleHidden,
  history,
  onStep,
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
  /** Local pane only. A server has one root, so the remote breadcrumb has no menu. */
  roots?: Root[]
  showHidden: boolean
  /** How many rows the toggle is currently keeping back, or would. */
  hiddenCount: number
  onToggleHidden: () => void
  history: History
  /** -1 for back, +1 for forward. */
  onStep: (delta: number) => void
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
            className="ghostbtn ghostbtn--lg"
            title="New folder"
            aria-label="New folder"
            onClick={onNewFolder}
          >
            <IconFolderPlus size={17} />
          </button>
        )}
        {/* The count is in the label rather than on a badge: the only time this
            control matters is when it is holding something back, and that is
            exactly when the number is worth saying. */}
        <button
          className={`ghostbtn ghostbtn--lg${showHidden ? '' : ' ghostbtn--on'}`}
          title={
            showHidden
              ? `Hide ${hiddenCount} hidden ${hiddenCount === 1 ? 'item' : 'items'}`
              : `Show ${hiddenCount} hidden ${hiddenCount === 1 ? 'item' : 'items'}`
          }
          aria-label={showHidden ? 'Hide hidden items' : 'Show hidden items'}
          aria-pressed={!showHidden}
          onClick={onToggleHidden}
        >
          {showHidden ? <IconDotfile size={17} /> : <IconDotfileOff size={17} />}
        </button>
        {/* No parent button: the breadcrumb above already names every ancestor and
            navigates to it in one click, so a chevron that walks up one level at a
            time was a second, slower way to do the same thing. */}
        {onRefresh && (
          <button
            className="ghostbtn ghostbtn--lg"
            title="Refresh"
            aria-label="Refresh"
            onClick={onRefresh}
          >
            <IconRefresh size={17} />
          </button>
        )}
      </div>

      {/* The menu sits outside `.crumbs`, which scrolls horizontally: a popup inside a
          scroll container is clipped by it, and `overflow-x: auto` makes the vertical
          axis a scroll container too. */}
      <div className="pane-header__path">
        {roots && <RootMenu roots={roots} current={path} onPick={onNavigate} />}
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
      </div>

      {/* The filter shares its row with the history buttons, in the design's own
          pattern for this position: a bordered segmented group beside a filter that
          takes the remaining width. */}
      <div className="pane-header__tools">
        <div className="searchbox searchbox--pane">
          <IconSearch size={12} className="searchbox__icon" />
          <input
            placeholder={filterLabel}
            value={filter}
            aria-label={`${filterLabel} — ${title}`}
            onChange={(e) => onFilter(e.currentTarget.value)}
          />
        </div>
        <div className="segbtns">
          <button
            className="segbtn"
            title={`Back${peek(history, -1) ? ` to ${peek(history, -1)}` : ''}`}
            aria-label={`Back — ${title}`}
            disabled={!canGoBack(history)}
            onClick={() => onStep(-1)}
          >
            <IconChevronLeft size={17} strokeWidth={2.6} />
          </button>
          <button
            className="segbtn"
            title={`Forward${peek(history, 1) ? ` to ${peek(history, 1)}` : ''}`}
            aria-label={`Forward — ${title}`}
            disabled={!canGoForward(history)}
            onClick={() => onStep(1)}
          >
            <IconChevronRight size={17} strokeWidth={2.6} />
          </button>
        </div>
      </div>
    </div>
  )
}

/** Hidden files are shown by default, as the design draws them. Hiding is the opt-in,
 * because a listing that silently omits `.env` is worse than a busy one. */
function visible(showHidden: boolean) {
  return (row: FileRow) => showHidden || !row.hidden
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
