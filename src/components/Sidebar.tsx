import { useState } from 'react'

import { commands } from '@/ipc/commands'
import type { ServerConfig } from '@/ipc/gen'
import { groupServers, useServersStore } from '@/state/serversStore'
import { useUiStore } from '@/state/uiStore'

import { ServerEditor, blankServer } from './ServerEditor'
import { avatarFor, tintFor } from './TitleBar'

export function Sidebar() {
  const servers = useServersStore((s) => s.servers)
  const loading = useServersStore((s) => s.loading)
  const toast = useUiStore((s) => s.toast)
  const [editing, setEditing] = useState<ServerConfig | null>(null)

  const open = (id: string) => {
    commands.sessionOpen(id).catch((error: unknown) => {
      toast('error', `Could not open the session: ${String(error)}`)
    })
  }

  return (
    <>
      <nav className="sidebar" aria-label="Servers">
        <div className="sidebar__top">
          <span className="sidebar__group">Servers</span>
          <button
            className="iconbtn"
            title="Add a server"
            aria-label="Add a server"
            onClick={() => setEditing(blankServer())}
          >
            +
          </button>
        </div>
        {loading && <div className="skeleton" style={{ margin: 8 }} />}
        {groupServers(servers).map(([group, entries]) => (
          <div key={group}>
            <div className="sidebar__group">{group}</div>
            {entries.map((server) => (
              <div key={server.id} className="sidebar__row">
                <button
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
                <button
                  className="iconbtn sidebar__edit"
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
      </nav>
      {editing && (
        <ServerEditor
          key={editing.id}
          server={editing}
          isNew={!servers.some((s) => s.id === editing.id)}
          onClose={() => setEditing(null)}
        />
      )}
    </>
  )
}
