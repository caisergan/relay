import { useMemo, useState } from 'react'

import { commands } from '@/ipc/commands'
import type { ServerConfig } from '@/ipc/gen'
import { faultText } from '@/lib/errors'
import { groupServers, useServersStore } from '@/state/serversStore'
import { useSessionsStore } from '@/state/sessionsStore'
import { useUiStore } from '@/state/uiStore'

import { IconPlus, IconRelay, IconSearch, IconSidebar } from './Icons'
import { ServerAvatar } from './ServerAvatar'
import { ServerEditor, blankServer } from './ServerEditor'

export function Sidebar() {
  const servers = useServersStore((s) => s.servers)
  const loading = useServersStore((s) => s.loading)
  const sessions = useSessionsStore((s) => s.sessions)
  const order = useSessionsStore((s) => s.order)
  const activeId = useSessionsStore((s) => s.activeId)
  const activate = useSessionsStore((s) => s.activate)
  const toast = useUiStore((s) => s.toast)
  const collapsed = useUiStore((s) => s.sidebarCollapsed)
  const toggleSidebar = useUiStore((s) => s.toggleSidebar)
  const [editing, setEditing] = useState<ServerConfig | null>(null)
  const [query, setQuery] = useState('')

  /// Which servers currently have a session, so the avatar can carry the live dot.
  /// Sessions are keyed by their own id, not by server id, so this is a lookup rather
  /// than a membership test on the array.
  const live = useMemo(() => {
    const ids = new Set<string>()
    for (const session of Object.values(sessions)) {
      if (session.state.kind === 'connected') ids.add(session.serverId)
    }
    return ids
  }, [sessions])

  const matches = useMemo(() => {
    const needle = query.trim().toLowerCase()
    if (needle === '') return servers
    return servers.filter(
      (s) =>
        s.name.toLowerCase().includes(needle) ||
        s.host.toLowerCase().includes(needle) ||
        s.username.toLowerCase().includes(needle),
    )
  }, [servers, query])

  /// This server's tabs, in tab order. Sessions are keyed by their own id, so the
  /// server they belong to is a property rather than the key.
  const tabsByServer = useMemo(() => {
    const map = new Map<string, string[]>()
    for (const id of order) {
      const serverId = sessions[id]?.serverId
      if (serverId === undefined) continue
      map.set(serverId, [...(map.get(serverId) ?? []), id])
    }
    return map
  }, [order, sessions])

  /// The server the visible tab belongs to, so its card can read as selected. Null
  /// while the connect form is showing, which belongs to no server.
  const activeServerId = activeId ? (sessions[activeId]?.serverId ?? null) : null

  /** Clicking a server goes to it; it does not dial it again.
   *
   * Every click used to call `session_open`, so a server with a tab already open
   * collected another one — four tabs onto the same host, each its own connection, each
   * costing a handshake and a keychain lookup.
   *
   * With more than one tab onto a server, a repeat click steps to the next of them and
   * wraps. That makes a second click "show me the other window onto this server"
   * rather than nothing, and with the usual single tab it is a no-op. Opening a
   * genuinely new session is still possible, deliberately, with the modifier that
   * means "new" everywhere else. */
  const open = (serverId: string, newSession = false) => {
    const tabs = tabsByServer.get(serverId) ?? []
    if (!newSession && tabs.length > 0) {
      const at = activeId ? tabs.indexOf(activeId) : -1
      const next = tabs[(at + 1) % tabs.length]
      if (next !== undefined) {
        activate(next)
        return
      }
    }
    commands.sessionOpen(serverId).catch((error: unknown) => {
      toast('error', `Could not open the session: ${faultText(error)}`)
    })
  }

  const editor = editing && (
    <ServerEditor
      key={editing.id}
      server={editing}
      isNew={!servers.some((s) => s.id === editing.id)}
      onClose={() => setEditing(null)}
    />
  )

  /* Collapsed, the sidebar becomes a rail rather than disappearing behind a button
     floating over the panes — that button landed on top of the session header's own
     avatar. A rail keeps the servers reachable in one click, keeps its own column so
     nothing overlaps, and makes the collapsed state a smaller sidebar instead of an
     absence. */
  if (collapsed) {
    return (
      <>
        <nav className="rail" aria-label="Servers">
          <button
            className="rail__mark"
            title="Expand sidebar"
            aria-label="Expand sidebar"
            aria-expanded={false}
            onClick={() => toggleSidebar(false)}
          >
            <IconRelay size={15} />
          </button>
          <div className="rail__list">
            {servers.map((server) => (
              <button
                key={server.id}
                className={`rail__item${activeServerId === server.id ? ' rail__item--active' : ''}`}
                onClick={(event) => open(server.id, event.metaKey || event.ctrlKey)}
                aria-current={activeServerId === server.id ? 'true' : undefined}
                title={`${server.name} — ${server.username}@${server.host}:${server.port}`}
                aria-label={server.name}
              >
                <ServerAvatar
                  name={server.name}
                  color={server.color}
                  live={live.has(server.id)}
                  size={30}
                />
              </button>
            ))}
          </div>
          <button
            className="rail__item"
            title="New server"
            aria-label="New server"
            onClick={() => setEditing(blankServer())}
          >
            <IconPlus size={16} />
          </button>
        </nav>
        {editor}
      </>
    )
  }

  return (
    <>
      {
        <nav className="sidebar" aria-label="Servers">
          <div className="sidebar__brand">
            <span className="sidebar__mark">
              <IconRelay size={14} />
            </span>
            <span className="sidebar__wordmark">Relay</span>
            <div style={{ flex: 1 }} />
            <button
              className="ghostbtn"
              title="Collapse sidebar"
              aria-label="Collapse sidebar"
              aria-expanded
              onClick={() => toggleSidebar(true)}
            >
              <IconSidebar size={15} />
            </button>
          </div>

          <div className="searchbox">
            <IconSearch size={13} className="searchbox__icon" />
            <input
              value={query}
              onChange={(e) => setQuery(e.currentTarget.value)}
              placeholder="Search servers"
              aria-label="Search servers"
              spellCheck={false}
            />
          </div>

          <div className="sidebar__list">
            {loading && <div className="skeleton" style={{ margin: 8 }} />}
            {groupServers(matches).map(([group, entries]) => (
              <div className="sidebar__section" key={group}>
                <div className="sidebar__group">
                  <span>{group}</span>
                  <span className="sidebar__rule" />
                </div>
                {entries.map((server) => (
                  <div
                    key={server.id}
                    className={`srv${activeServerId === server.id ? ' srv--active' : ''}`}
                  >
                    <button
                      className="srv__hit"
                      onClick={(event) => open(server.id, event.metaKey || event.ctrlKey)}
                      aria-current={activeServerId === server.id ? 'true' : undefined}
                      title={`${server.username}@${server.host}:${server.port}\n⌘-click for a second session`}
                    >
                      <ServerAvatar
                        name={server.name}
                        color={server.color}
                        live={live.has(server.id)}
                      />
                      <span className="srv__text">
                        <span className="srv__line">
                          <span className="srv__name">{server.name}</span>
                          <span
                            className={`srv__chip${server.proto === 'ftp' ? ' srv__chip--insecure' : ''}`}
                          >
                            {server.proto}
                          </span>
                        </span>
                        <span className="srv__host">
                          {server.username}@{server.host}
                        </span>
                      </span>
                    </button>
                    <button
                      className="srv__kebab"
                      title={`Edit ${server.name}`}
                      aria-label={`Edit ${server.name}`}
                      onClick={() => setEditing(server)}
                    >
                      ⋯
                    </button>
                  </div>
                ))}
              </div>
            ))}
            {!loading && servers.length === 0 && (
              <div className="empty">
                <span className="empty__title">No servers yet</span>
                <span>Connect to one and it will appear here.</span>
              </div>
            )}
            {!loading && servers.length > 0 && matches.length === 0 && (
              <div className="empty">
                <span>Nothing matches “{query}”.</span>
              </div>
            )}
          </div>

          <button className="newserver" onClick={() => setEditing(blankServer())}>
            <IconPlus size={14} />
            New server
          </button>
        </nav>
      }
      {editor}
    </>
  )
}
