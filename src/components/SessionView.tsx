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
import { exactText } from '@/lib/exactText'
import { isPathQuery, resolvePath, splitPath } from '@/lib/goto'
import { preview } from '@/state/previews'
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

/** How a pane moves. `select` lands with that row already selected — a typed path to a
 * file opens the folder it is in, and the file is the reason for going. */
interface Navigation {
  record?: boolean
  select?: string | null
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
  const [pendingDelete, setPendingDelete] = useState<FileRow[] | null>(null)
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
    async (path: string, { record = true, select = null }: Navigation = {}) => {
      // A filter belongs to the directory it was typed in. Carried into the next one
      // it silently hides most of what is there, and the box that explains why is a
      // row above the listing where nobody looks. A refresh keeps it: same directory,
      // same question.
      const before = useSessionsStore.getState().panes[sessionId] ?? emptyPane
      patchPane(sessionId, {
        localLoading: true,
        localError: null,
        ...(path === before.localPath ? {} : { localFilter: '', localSelected: [] }),
        ...(select === null ? {} : { localSelected: [select] }),
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

  /// Where the local pane starts: the folder a restored tab was in, while it is still a
  /// folder, and home otherwise. A saved folder that has since gone opens home rather
  /// than a pane announcing that something is missing.
  const startServer = session?.serverId
  useEffect(() => {
    if (pane.localPath || !startServer) return
    const saved = useSessionsStore.getState().claimLocalStart(sessionId, startServer)
    void (async () => {
      const found = saved ? await commands.localStat(saved).catch(() => null) : null
      const stillThere = found !== null && (found.kind === 'dir' || found.targetKind === 'dir')
      await loadLocal(saved && stillThere ? saved : await commands.localDefaultDir())
    })()
  }, [pane.localPath, loadLocal, sessionId, startServer])

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
          .filter(matching(pane.remoteFilter, 'remote')),
        pane.remoteSort,
      ),
    [allRemoteRows, pane.remoteFilter, pane.remoteSort, pane.remoteShowHidden],
  )
  const localRows = useMemo(
    () =>
      sortRows(
        allLocalRows
          .filter(visible(pane.localShowHidden))
          .filter(matching(pane.localFilter, 'local')),
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
  const lift = (from: 'local' | 'remote') => (rows: FileRow[], e: React.PointerEvent) =>
    begin({ from, entries: rows.map(({ name, isDir }) => ({ name, isDir })) }, e)

  /// The inspector a right-click opens. Its state lives above the early return because
  /// it is a hook; the two lookups that fill it are below, where the listings are.
  const properties = useProperties()

  if (!session) return null

  const remotePath = listing?.path ?? session.remotePath ?? '/'
  const connected = session.state.kind === 'connected'

  const navigateRemote = (path: string, { record = true, select = null }: Navigation = {}) => {
    patchPane(sessionId, {
      remoteLoading: true,
      remoteError: null,
      ...(path === remotePath ? {} : { remoteFilter: '', remoteSelected: [] }),
      ...(select === null ? {} : { remoteSelected: [select] }),
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
    void loadLocal(target, { record: false })
  }

  const stepRemote = (delta: number) => {
    const target = peek(pane.remoteHistory, delta)
    if (target === null) return
    patchPane(sessionId, {
      remoteHistory: { ...pane.remoteHistory, at: pane.remoteHistory.at + delta },
    })
    navigateRemote(target, { record: false })
  }

  /// Where the search box sends a typed path. It asks what is there before moving the
  /// pane: a folder opens, a file opens the folder it is in with the file selected, and
  /// nothing at all leaves the pane where it was with the path still in the box — a typo
  /// should cost a keystroke, not the place you were in.
  ///
  /// `~` on the server is the account's home, as the server resolved it on connect.
  const goTo = async (side: 'local' | 'remote', text: string) => {
    try {
      const home =
        side === 'local'
          ? await commands.localDefaultDir()
          : session.state.kind === 'connected'
            ? session.state.info.homePath
            : '/'
      const path = resolvePath(text, home)
      const found =
        side === 'local'
          ? await commands.localStat(path)
          : await commands.sessionStat(sessionId, path)
      if (!found) {
        toast('error', `Nothing at ${path}`)
        return
      }
      const { parent, name } = splitPath(path)
      const target =
        found.kind === 'dir' || found.targetKind === 'dir'
          ? { to: path, select: null }
          : { to: parent, select: name }
      // Cleared here as well as by the navigation, which keeps a filter when the
      // destination is the folder already on show.
      if (side === 'local') {
        patchPane(sessionId, { localFilter: '' })
        await loadLocal(target.to, { select: target.select })
      } else {
        patchPane(sessionId, { remoteFilter: '' })
        navigateRemote(target.to, { select: target.select })
      }
    } catch (error) {
      toast('error', faultText(error))
    }
  }

  /// A row's transfer button, carrying the selection it belongs to as one batch.
  const transfer = (direction: 'up' | 'down') => (rows: FileRow[]) => enqueue(direction, rows)

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

  /// A file opens in an application on this Mac rather than in the pane, which has
  /// nothing to show a file with. Asks which application unless one is remembered.
  const previewRemote = (row: FileRow) => {
    if (!serverId) return
    preview({
      session: sessionId,
      serverId,
      remotePath: joinPath(remotePath, row.name),
      name: row.name,
    }).catch((error: unknown) => toast('error', faultText(error)))
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

  const confirmDelete = (rows: FileRow[]) => setPendingDelete(rows)

  /// One at a time: the session has a single command queue, so sending them together
  /// would only line them up in it. A failure does not stop the rest — the others were
  /// chosen too — and the pane is refreshed once, after the last.
  const doDelete = async (rows: FileRow[]) => {
    setPendingDelete(null)
    const failed: string[] = []
    for (const row of rows) {
      try {
        await commands.sessionRemove(sessionId, joinPath(remotePath, row.name), row.isDir)
      } catch (error) {
        failed.push(`${row.name}: ${faultText(error)}`)
      }
    }
    if (failed.length < rows.length) {
      patchPane(sessionId, { remoteSelected: [] })
      refreshRemote()
    }
    if (failed.length === 1) toast('error', `Could not delete ${failed[0] ?? ''}`)
    if (failed.length > 1) {
      toast(
        'error',
        `Could not delete ${failed.length} of ${rows.length} items — ${failed[0] ?? ''}`,
      )
    }
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
      <PendingGo sessionId={sessionId} onGo={(side, text) => void goTo(side, text)} />
      <div className="panes" ref={panesRef}>
        <div className="pane pane--local" style={{ width: `${localPercent}%` }}>
          <PaneHeader
            kind="local"
            title="This Mac"
            path={pane.localPath || '/'}
            filter={pane.localFilter}
            filterLabel="Search"
            onFilter={(localFilter) => patchPane(sessionId, { localFilter })}
            onGo={(text) => void goTo('local', text)}
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
            onGo={(text) => void goTo('remote', text)}
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
              onOpen={(row) =>
                row.isDir ? navigateRemote(joinPath(remotePath, row.name)) : previewRemote(row)
              }
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
            <span className="dragghost__name">
              {drag.entries.length === 1
                ? drag.entries[0]?.name
                : `${drag.entries.length} items`}
            </span>
          </div>,
          document.body,
        )}
      {pendingDelete && (
        <div className="scrim" role="dialog" aria-modal="true">
          <div className="sheet sheet--danger">
            <h2 className="sheet__title">
              Delete{' '}
              {pendingDelete.length === 1
                ? pendingDelete[0]?.name
                : `${pendingDelete.length} items`}
              ?
            </h2>
            <p className="sheet__body">{deleteWarning(pendingDelete)}</p>
            <div className="sheet__actions">
              <button className="btn" onClick={() => setPendingDelete(null)}>
                Cancel
              </button>
              <button className="btn btn--danger" onClick={() => void doDelete(pendingDelete)}>
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
  onGo,
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
  /** Return in the box, or its Go button, while what is typed is a path. */
  onGo: (text: string) => void
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
  const going = isPathQuery(filter, kind)
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
            {...exactText}
            placeholder={filterLabel}
            value={filter}
            aria-label={`${filterLabel} — ${title}`}
            onChange={(e) => onFilter(e.currentTarget.value)}
            onKeyDown={(e) => {
              // Not mid-composition, where Return confirms the composed text instead.
              if (e.key === 'Enter' && going && !e.nativeEvent.isComposing) onGo(filter)
            }}
          />
          {going && (
            <button
              className="searchbox__go"
              title="Go to this path (Return)"
              aria-label={`Go to this path — ${title}`}
              onClick={() => onGo(filter)}
            >
              Go <span className="kbd">↵</span>
            </button>
          )}
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

/** What the delete sheet warns. Several items are named rather than counted: a stray
 * ⌘-click is exactly the mistake this sheet is there to catch, and "3 items" does not
 * say which three. */
function deleteWarning(rows: FileRow[]): string {
  const warning = rows.some((row) => row.isDir)
    ? rows.length === 1
      ? 'The folder must be empty. This cannot be undone.'
      : 'Folders must be empty. This cannot be undone.'
    : 'This cannot be undone — there is no trash on the server.'
  if (rows.length === 1) return warning
  const shown = rows
    .slice(0, 5)
    .map((row) => row.name)
    .join(', ')
  const more = rows.length > 5 ? ` and ${rows.length - 5} more` : ''
  return `${shown}${more}. ${warning}`
}

/** Takes a path sent from outside the pane — the command palette — and goes there.
 *
 * A component of its own because the going lives below the view's early return, where
 * no effect can be, and an effect is what has to notice the request. It reads the store
 * again before acting: React runs a mount effect twice in development, and the first run
 * has already taken the request by then. */
function PendingGo({
  sessionId,
  onGo,
}: {
  sessionId: string
  onGo: (side: 'local' | 'remote', text: string) => void
}) {
  const pending = useUiStore((s) => s.pendingGo)
  useEffect(() => {
    const request = useUiStore.getState().pendingGo
    if (!request || request !== pending || request.sessionId !== sessionId) return
    useUiStore.getState().clearGo()
    onGo(request.side, request.text)
  }, [pending, sessionId, onGo])
  return null
}

/** Hidden files are shown by default, as the design draws them. Hiding is the opt-in,
 * because a listing that silently omits `.env` is worse than a busy one. */
function visible(showHidden: boolean) {
  return (row: FileRow) => showHidden || !row.hidden
}

/** A path is somewhere to go, not a name to look for — and as a name it could only match
 * nothing, since no name contains a slash. While one is being typed the listing stays
 * whole. */
function matching(filter: string, side: 'local' | 'remote') {
  if (isPathQuery(filter, side)) return () => true
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
