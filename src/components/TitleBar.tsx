import { isMac } from '@/app/useTheme'
import { commands } from '@/ipc/commands'
import { useServersStore } from '@/state/serversStore'
import { useSessionsStore } from '@/state/sessionsStore'
import { useUiStore } from '@/state/uiStore'

import { IconClose, IconMoon, IconPlus, IconSearch, IconSun } from './Icons'
import { ServerAvatar } from './ServerAvatar'

export function TitleBar() {
  const sessions = useSessionsStore((s) => s.sessions)
  const order = useSessionsStore((s) => s.order)
  const activeId = useSessionsStore((s) => s.activeId)
  const activate = useSessionsStore((s) => s.activate)
  const servers = useServersStore((s) => s.servers)
  const resolved = useUiStore((s) => s.resolved)
  const setTheme = useUiStore((s) => s.setTheme)
  const toast = useUiStore((s) => s.toast)

  // The tab avatar has to be the colour chosen in the editor, not a hash of the name:
  // picking a tint and then seeing a different one on the tab makes the setting look
  // broken.
  /// `ServerAvatar` falls back to the deterministic tint on its own, so a server that
  /// has not been given a colour reaches it as null rather than as a second hash here.
  const colourOf = (serverId: string) => servers.find((s) => s.id === serverId)?.color ?? null

  const dark = resolved === 'dark'

  return (
    <div className="titlebar" data-tauri-drag-region>
      {/* Space for the traffic lights, not a drawing of them. `tauri.macos.conf.json`
          sets `titleBarStyle: "Overlay"`, so macOS paints the real buttons over the
          web content at this corner; painting our own put a second, dead set directly
          on top of the working ones. Only the reserved width is ours. */}
      {isMac() && <div className="titlebar__lights" aria-hidden />}
      <div className="titlebar__tabs" data-tauri-drag-region>
        {order.map((id) => {
          const session = sessions[id]
          if (!session) return null
          const active = id === activeId
          return (
            <div
              key={id}
              className={`tab${active ? ' tab--active' : ''}`}
              onClick={() => activate(id)}
              role="tab"
              aria-selected={active}
              tabIndex={0}
              onKeyDown={(e) => e.key === 'Enter' && activate(id)}
            >
              <ServerAvatar name={session.name} color={colourOf(session.serverId)} size={16} />
              <span className="tab__label">{session.name}</span>
              <span
                className="tab__close"
                title="Close session"
                onClick={(e) => {
                  e.stopPropagation()
                  void commands.sessionClose(id)
                }}
              >
                <IconClose size={11} />
              </span>
            </div>
          )
        })}
        <button
          className="tab__new"
          title="New session (⌘T)"
          onClick={() => activate(null)}
          aria-label="New session"
        >
          <IconPlus size={15} />
        </button>
      </div>
      <div className="titlebar__actions">
        <button
          className="iconbtn iconbtn--bordered"
          title="Command palette (⌘K)"
          // The palette itself is 4.4. The button is the design's and stays, but it
          // says so rather than flipping a flag nothing renders.
          onClick={() => toast('info', 'The command palette arrives in phase 4.')}
        >
          <IconSearch size={13} />
          <span className="kbd">⌘K</span>
        </button>
        <button
          className="iconbtn"
          title="Toggle theme"
          aria-label="Toggle theme"
          onClick={() => setTheme(dark ? 'light' : 'dark')}
        >
          {dark ? <IconSun size={16} /> : <IconMoon size={16} />}
        </button>
      </div>
    </div>
  )
}
