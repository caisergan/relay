import { isMac } from '@/app/useTheme'
import { commands } from '@/ipc/commands'
import { useServersStore } from '@/state/serversStore'
import { useSessionsStore } from '@/state/sessionsStore'
import { useUiStore } from '@/state/uiStore'

import { IconClose, IconMoon, IconPlus, IconSearch, IconSun } from './Icons'

const LIGHTS = ['#FF5F57', '#FEBC2E', '#28C840']

function avatarFor(name: string): string {
  return name.slice(0, 2).toUpperCase()
}

/** Deterministic tint for a server that has not been given one.
 *
 * The palette is the design's swatch row, so a generated tint and a chosen tint are
 * drawn from the same six colours — an auto-assigned server cannot end up a shade
 * that the editor would never offer. */
const PALETTE = ['#2456E6', '#7C4DDB', '#E8A03C', '#2E9E5B', '#0FA3A3', '#D6453C']

function tintFor(name: string): string {
  let hash = 0
  for (const char of name) hash = (hash * 31 + char.charCodeAt(0)) >>> 0
  return PALETTE[hash % PALETTE.length] ?? '#2456E6'
}

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
  const colourOf = (serverId: string, name: string) =>
    servers.find((s) => s.id === serverId)?.color ?? tintFor(name)

  const dark = resolved === 'dark'

  return (
    <div className="titlebar" data-tauri-drag-region>
      {isMac() && (
        <div className="titlebar__lights">
          {LIGHTS.map((color) => (
            <span
              key={color}
              style={{ width: 12, height: 12, borderRadius: '50%', background: color }}
            />
          ))}
        </div>
      )}
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
              <span
                className="tab__avatar"
                style={{ background: colourOf(session.serverId, session.name) }}
              >
                {avatarFor(session.name)}
              </span>
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

export { tintFor, avatarFor }
