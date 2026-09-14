import { useEffect, useRef, useState } from 'react'

import { commands } from '@/ipc/commands'
import { faultText } from '@/lib/errors'
import { exactText } from '@/lib/exactText'
import { isPathQuery } from '@/lib/goto'
import { useServersStore } from '@/state/serversStore'
import { useSessionsStore } from '@/state/sessionsStore'
import { useUiStore } from '@/state/uiStore'

import {
  IconChevronDown,
  IconClose,
  IconGear,
  IconMonitor,
  IconPlus,
  IconRefresh,
  IconSearch,
  IconServer,
  IconSidebar,
} from './Icons'

interface Command {
  id: string
  label: string
  /** Shown faint on the right, and searched along with the label. */
  hint: string | null
  icon: React.ReactNode
  run: () => void
}

/** ⌘K: what Relay can do, found by typing part of its name.
 *
 * The design's palette, holding what the app has today: saved servers to open or switch
 * to, the session in front of you, the drawer, the sidebar and the settings. A path typed
 * into it goes there, as it does in a pane's search box. Selection actions and bookmarks
 * are in the design's list too; they arrive with the features they act on. */
export function CommandPalette() {
  const open = useUiStore((s) => s.paletteOpen)
  // Mounted only while open, so every opening starts from an empty query.
  return open ? <Palette /> : null
}

function Palette() {
  const togglePalette = useUiStore((s) => s.togglePalette)
  const drawerOpen = useUiStore((s) => s.drawerOpen)
  const toggleDrawer = useUiStore((s) => s.toggleDrawer)
  const sidebarCollapsed = useUiStore((s) => s.sidebarCollapsed)
  const toggleSidebar = useUiStore((s) => s.toggleSidebar)
  const toggleSettings = useUiStore((s) => s.toggleSettings)
  const requestGo = useUiStore((s) => s.requestGo)
  const toast = useUiStore((s) => s.toast)
  const sessions = useSessionsStore((s) => s.sessions)
  const order = useSessionsStore((s) => s.order)
  const activeId = useSessionsStore((s) => s.activeId)
  const activate = useSessionsStore((s) => s.activate)
  const servers = useServersStore((s) => s.servers)

  const [query, setQuery] = useState('')
  const [index, setIndex] = useState(0)
  const listRef = useRef<HTMLDivElement>(null)

  const close = () => togglePalette(false)
  const failed = (what: string) => (error: unknown) =>
    toast('error', `Could not ${what}: ${faultText(error)}`)

  const active = activeId ? sessions[activeId] : undefined
  const connected = active?.state.kind === 'connected'
  const text = query.trim()

  // A path is somewhere to go, not a name to look for, so it gets the destinations and
  // nothing else — no command's name has a slash in it.
  const pathCommands: Command[] = []
  if (active && isPathQuery(text, 'remote') && connected) {
    pathCommands.push({
      id: 'go-remote',
      label: `Go to ${text}`,
      hint: active.name,
      icon: <IconServer size={14} />,
      run: () => requestGo({ sessionId: active.id, side: 'remote', text }),
    })
  }
  if (active && isPathQuery(text, 'local')) {
    pathCommands.push({
      id: 'go-local',
      label: `Go to ${text}`,
      hint: 'This Mac',
      icon: <IconMonitor size={14} />,
      run: () => requestGo({ sessionId: active.id, side: 'local', text }),
    })
  }

  const all: Command[] = [
    ...servers.map((server): Command => {
      const tab = order.find((id) => sessions[id]?.serverId === server.id)
      const hint = `${server.username}@${server.host}`
      return tab
        ? {
            id: `server-${server.id}`,
            label: `Switch to ${server.name}`,
            hint,
            icon: <IconServer size={14} />,
            run: () => activate(tab),
          }
        : {
            id: `server-${server.id}`,
            label: `Connect to ${server.name}`,
            hint,
            icon: <IconServer size={14} />,
            run: () => {
              commands.sessionOpen(server.id).catch(failed(`open ${server.name}`))
            },
          }
    }),
    {
      id: 'new',
      label: 'New connection',
      hint: null,
      icon: <IconPlus size={14} />,
      run: () => activate(null),
    },
    ...(active && connected && active.remotePath
      ? [
          {
            id: 'refresh',
            label: `Refresh ${active.name}`,
            hint: active.remotePath,
            icon: <IconRefresh size={14} />,
            run: () => {
              if (active.remotePath) {
                commands
                  .sessionListDir(active.id, active.remotePath)
                  .catch(failed('refresh the folder'))
              }
            },
          },
        ]
      : []),
    ...(active
      ? [
          {
            id: 'disconnect',
            label: `Disconnect ${active.name}`,
            hint: null,
            icon: <IconClose size={14} />,
            run: () => {
              commands.sessionClose(active.id).catch(failed('disconnect'))
            },
          },
        ]
      : []),
    {
      id: 'drawer',
      label: drawerOpen ? 'Hide transfers' : 'Show transfers',
      hint: '⌘J',
      icon: <IconChevronDown size={14} />,
      run: () => toggleDrawer(),
    },
    {
      id: 'sidebar',
      label: sidebarCollapsed ? 'Show the server list' : 'Hide the server list',
      hint: null,
      icon: <IconSidebar size={14} />,
      run: () => toggleSidebar(),
    },
    {
      id: 'settings',
      label: 'Open settings',
      hint: null,
      icon: <IconGear size={14} />,
      run: () => toggleSettings(true),
    },
  ]

  const needle = text.toLowerCase()
  const shown =
    pathCommands.length > 0 || isPathQuery(text, 'local')
      ? pathCommands
      : all.filter((command) =>
          `${command.label} ${command.hint ?? ''}`.toLowerCase().includes(needle),
        )
  const at = Math.min(index, Math.max(shown.length - 1, 0))
  const current = shown[at]

  const run = (command: Command) => {
    close()
    command.run()
  }

  // Keyboard movement can walk past the bottom of a long list.
  useEffect(() => {
    listRef.current
      ?.querySelector<HTMLElement>('[aria-selected="true"]')
      ?.scrollIntoView({ block: 'nearest' })
  }, [at])

  return (
    <div
      className="palette-scrim"
      onPointerDown={(e) => {
        if (e.target === e.currentTarget) close()
      }}
    >
      <div className="palette" role="dialog" aria-modal="true" aria-label="Command palette">
        <div className="palette__head">
          <IconSearch size={17} className="palette__glass" />
          <input
            {...exactText}
            // The whole point of opening it is to type.
            autoFocus
            className="palette__input"
            placeholder="Search commands and servers, or type a path…"
            value={query}
            role="combobox"
            aria-expanded
            aria-controls="palette-list"
            aria-activedescendant={current ? `palette-${current.id}` : undefined}
            onChange={(e) => {
              setQuery(e.currentTarget.value)
              setIndex(0)
            }}
            onKeyDown={(e) => {
              if (e.key === 'ArrowDown') {
                e.preventDefault()
                setIndex(Math.min(at + 1, shown.length - 1))
              } else if (e.key === 'ArrowUp') {
                e.preventDefault()
                setIndex(Math.max(at - 1, 0))
              } else if (e.key === 'Enter' && !e.nativeEvent.isComposing) {
                e.preventDefault()
                if (current) run(current)
              } else if (e.key === 'Escape') {
                e.preventDefault()
                close()
              } else if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
                // The keys that opened it close it. Stopped here, or the window's own
                // shortcut would open it again straight after.
                e.preventDefault()
                e.stopPropagation()
                close()
              }
            }}
          />
          <span className="palette__key">esc</span>
        </div>
        <div className="palette__list" id="palette-list" role="listbox" ref={listRef}>
          {shown.map((command, i) => (
            <div
              key={command.id}
              id={`palette-${command.id}`}
              role="option"
              aria-selected={i === at}
              className={`palette__item${i === at ? ' palette__item--on' : ''}`}
              onPointerMove={() => {
                if (i !== at) setIndex(i)
              }}
              onClick={() => run(command)}
            >
              <span className="palette__icon">{command.icon}</span>
              <span className="palette__label">{command.label}</span>
              {command.hint && <span className="palette__hint">{command.hint}</span>}
            </div>
          ))}
          {shown.length === 0 && <div className="palette__empty">No matching commands</div>}
        </div>
        <div className="palette__foot">
          <span>↑↓ navigate</span>
          <span>↵ run</span>
          <span>esc close</span>
        </div>
      </div>
    </div>
  )
}
