import { isMac } from '@/app/useTheme'
import { commands } from '@/ipc/commands'
import { useSessionsStore } from '@/state/sessionsStore'
import { useUiStore } from '@/state/uiStore'

const LIGHTS = ['#FF5F57', '#FEBC2E', '#28C840']

function avatarFor(name: string): string {
  return name.slice(0, 2).toUpperCase()
}

/** Deterministic tint so a server keeps the same colour between sessions. */
function tintFor(name: string): string {
  const palette = ['#2456E6', '#2E9E5B', '#E8A03C', '#D6453C', '#6B4FD8']
  let hash = 0
  for (const char of name) hash = (hash * 31 + char.charCodeAt(0)) >>> 0
  return palette[hash % palette.length] ?? '#2456E6'
}

export function TitleBar() {
  const sessions = useSessionsStore((s) => s.sessions)
  const order = useSessionsStore((s) => s.order)
  const activeId = useSessionsStore((s) => s.activeId)
  const activate = useSessionsStore((s) => s.activate)
  const togglePalette = useUiStore((s) => s.togglePalette)
  const theme = useUiStore((s) => s.theme)
  const setTheme = useUiStore((s) => s.setTheme)

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
              <span className="tab__avatar" style={{ background: tintFor(session.name) }}>
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
                ×
              </span>
            </div>
          )
        })}
        <button
          className="iconbtn"
          title="New session (⌘T)"
          onClick={() => activate(null)}
          aria-label="New session"
        >
          +
        </button>
      </div>
      <div className="titlebar__actions">
        <button className="iconbtn iconbtn--bordered" onClick={() => togglePalette()}>
          ⌘K
        </button>
        <button
          className="iconbtn"
          title="Toggle theme"
          aria-label="Toggle theme"
          onClick={() => setTheme(theme === 'dark' ? 'light' : 'dark')}
        >
          {theme === 'dark' ? '☀' : '☾'}
        </button>
      </div>
    </div>
  )
}

export { tintFor, avatarFor }
