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

  const open = (id: string) => {
    commands.sessionOpen(id).catch((error: unknown) => {
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
                className="rail__item"
                onClick={() => open(server.id)}
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
                  <div key={server.id} className="srv">
                    <button
                      className="srv__hit"
                      onClick={() => open(server.id)}
                      title={`${server.username}@${server.host}:${server.port}`}
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
