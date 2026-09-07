import { useEffect } from 'react'

import { PromptSheets } from '@/components/PromptSheets'
import { QueueDrawer } from '@/components/QueueDrawer'
import { SessionView } from '@/components/SessionView'
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
  const activeId = useSessionsStore((s) => s.activeId)
  const connected = useUiStore((s) => s.connected)
  const toggleDrawer = useUiStore((s) => s.toggleDrawer)
  const toast = useUiStore((s) => s.toast)

  useEffect(() => {
    void engineBridge.start()
    void loadServers()
    return () => {
      void engineBridge.stop()
    }
  }, [loadServers])

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
      <PromptSheets />
      <Toasts />
    </div>
  )
}
