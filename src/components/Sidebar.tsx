import { commands } from '@/ipc/commands'
import { groupServers, useServersStore } from '@/state/serversStore'
import { useUiStore } from '@/state/uiStore'

import { avatarFor, tintFor } from './TitleBar'

export function Sidebar() {
  const servers = useServersStore((s) => s.servers)
  const loading = useServersStore((s) => s.loading)
  const toast = useUiStore((s) => s.toast)

  const open = (id: string) => {
    commands.sessionOpen(id).catch((error: unknown) => {
      toast('error', `Could not open the session: ${String(error)}`)
    })
  }

  return (
    <nav className="sidebar" aria-label="Servers">
      {loading && <div className="skeleton" style={{ margin: 8 }} />}
      {groupServers(servers).map(([group, entries]) => (
        <div key={group}>
          <div className="sidebar__group">{group}</div>
          {entries.map((server) => (
            <button
              key={server.id}
              className="sidebar__item"
              onClick={() => open(server.id)}
              title={`${server.username}@${server.host}:${server.port}`}
            >
              <span
                className="tab__avatar"
                style={{ background: server.color ?? tintFor(server.name) }}
              >
                {avatarFor(server.name)}
              </span>
              <span className="tab__label">{server.name}</span>
              <span className="sidebar__meta">{server.proto}</span>
            </button>
          ))}
        </div>
      ))}
      {!loading && servers.length === 0 && (
        <div className="empty">
          <span className="empty__title">No servers yet</span>
          <span>Connect to one and it will appear here.</span>
        </div>
      )}
    </nav>
  )
}
