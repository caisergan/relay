/** Wires the engine stream into the stores. The single place where an `EngineEvent`
 * turns into UI state. */

import { EngineBridge } from '@/ipc/bridge'
import type { EngineEvent, EngineSnapshot } from '@/ipc/gen'

import { useQueueStore } from './queueStore'
import { useSessionsStore } from './sessionsStore'
import { useUiStore } from './uiStore'

function applySnapshot(snapshot: EngineSnapshot): void {
  useSessionsStore.getState().replaceAll(snapshot.sessions, snapshot.listings)
  useQueueStore.getState().replaceAll(snapshot.jobs, snapshot.stats)
  useUiStore.getState().setPrompts(snapshot.prompts)
}

function applyEvent(event: EngineEvent): void {
  const sessions = useSessionsStore.getState()
  const queue = useQueueStore.getState()
  const ui = useUiStore.getState()

  switch (event.kind) {
    case 'sessionOpened':
      sessions.upsert(event.session)
      break
    case 'sessionState':
      sessions.setState(event.id, event.state)
      if (event.state.kind === 'disconnected' && event.state.unexpected) {
        ui.toast('error', `Connection lost: ${event.state.reason}`)
      }
      break
    case 'sessionClosed':
      sessions.remove(event.id)
      break
    case 'latency':
      sessions.setLatency(event.id, event.ms)
      break
    case 'listing':
      sessions.setListing(event.listing)
      break
    case 'jobUpdate':
      queue.upsert(event.job)
      if (event.job.state.kind === 'failed') {
        ui.toast('error', `${event.job.remotePath}: ${event.job.state.error.kind}`)
      }
      break
    case 'queueStats':
      useQueueStore.setState({ stats: event.stats })
      break
    case 'promptOpened':
      ui.openPrompt(event.prompt)
      break
    case 'promptClosed':
      ui.closePrompt(event.id)
      break
    // Logs are fetched on demand and activity gets its panel in phase 4; neither
    // belongs in a store yet.
    case 'log':
    case 'activity':
      break
  }
}

export const engineBridge = new EngineBridge({
  onSnapshot: applySnapshot,
  onEvent: applyEvent,
  onConnectionChange: (connected) => useUiStore.getState().setConnected(connected),
})
