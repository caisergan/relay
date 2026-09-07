import { useEffect, useRef } from 'react'

import { commands } from '@/ipc/commands'
import { faultText } from '@/lib/errors'
import { PromptSheets } from '@/components/PromptSheets'
import { QueueDrawer } from '@/components/QueueDrawer'
import { SessionView } from '@/components/SessionView'
import { SettingsSheet } from '@/components/SettingsSheet'
import { Sidebar } from '@/components/Sidebar'
import { TitleBar } from '@/components/TitleBar'
import { Toasts } from '@/components/Toasts'
import { engineBridge } from '@/state/engine'
import { useServersStore } from '@/state/serversStore'
import { useSessionsStore } from '@/state/sessionsStore'
import { useUiStore } from '@/state/uiStore'
import { ConnectView } from '@/views/ConnectView'
import '@/styles/app.css'

import { useTheme } from './useTheme'

export function App() {
  useTheme()

  const loadServers = useServersStore((s) => s.load)
  const servers = useServersStore((s) => s.servers)
  const serversLoaded = useServersStore((s) => s.loaded)
  const sessionCount = useSessionsStore((s) => s.order.length)
  const activeId = useSessionsStore((s) => s.activeId)
  const connected = useUiStore((s) => s.connected)
  const toggleDrawer = useUiStore((s) => s.toggleDrawer)
  const toast = useUiStore((s) => s.toast)

  const setTheme = useUiStore((s) => s.setTheme)
  const setDensity = useUiStore((s) => s.setDensity)
  const setShowHiddenDefault = useSessionsStore((s) => s.setShowHiddenDefault)

  useEffect(() => {
    void engineBridge.start()
    void loadServers()
    // Rust holds the saved settings; the UI store is a projection of them. Without
    // this the window would open in whatever the store's defaults are and only pick up
    // the person's theme when they opened the settings sheet.
    commands
      .settingsGet()
      .then((settings) => {
        setTheme(settings.theme)
        setDensity(settings.density)
        setShowHiddenDefault(settings.showHidden)
      })
      .catch(() => {
        // Defaults are already showing, and a toast about a preference nobody has set
        // yet would be the first thing a new install said.
      })
    return () => {
      void engineBridge.stop()
    }
  }, [loadServers, setTheme, setDensity, setShowHiddenDefault])

  /** Open the saved server on launch instead of the connect form.
   *
   * The connect form is for servers Relay does not know yet. Once one is saved, being
   * asked to type an address that is already in the sidebar is a step with no purpose.
   *
   * Fires at most once per run, and the guard is set even when it decides *not* to
   * open: without that, closing the session would immediately reopen it, which is a
   * window you cannot get out of. Adding a first server later does not trigger it
   * either — that flow opens its own session. */
  const launched = useRef(false)
  useEffect(() => {
    if (launched.current || !serversLoaded) return
    launched.current = true
    // Something is already open — a restored session — so the pane is not empty and
    // there is nothing to fill.
    if (sessionCount > 0) return
    const first = servers[0]
    if (!first) return
    commands.sessionOpen(first.id).catch((error: unknown) => {
      // A failure lands the user on the connect form, which is where they would have
      // been anyway; the toast says why rather than leaving it unexplained.
      toast('error', `Could not open ${first.name}: ${faultText(error)}`)
    })
  }, [serversLoaded, servers, sessionCount, toast])

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const accel = event.metaKey || event.ctrlKey
      if (accel && event.key.toLowerCase() === 'k') {
        event.preventDefault()
        // The palette is 4.4. Saying so beats a shortcut that silently does nothing.
        toast('info', 'The command palette arrives in phase 4.')
      }
      if (accel && event.key.toLowerCase() === 'j') {
        event.preventDefault()
        toggleDrawer()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [toast, toggleDrawer])

  return (
    <div className="shell">
      <TitleBar />
      {!connected && (
        <div className="offline" role="status">
          Reconnecting to the transfer engine…
        </div>
      )}
      <div className="body">
        <Sidebar />
        <div className="main">
          {activeId ? <SessionView sessionId={activeId} /> : <ConnectView />}
          <QueueDrawer />
        </div>
      </div>
      <SettingsSheet />
      <PromptSheets />
      <Toasts />
    </div>
  )
}
