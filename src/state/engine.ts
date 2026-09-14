/** Wires the engine stream into the stores. The single place where an `EngineEvent`
 * turns into UI state. */

import { EngineBridge } from '@/ipc/bridge'
import { commands } from '@/ipc/commands'
import type { EngineEvent, EngineSnapshot } from '@/ipc/gen'
import { faultText } from '@/lib/errors'

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
    case 'sessionState': {
      // Compared before the store is written, so "it is connected now and was not a
      // moment ago" is answerable. Afterwards both say connected.
      const was = sessions.sessions[event.id]?.state.kind
      sessions.setState(event.id, event.state)
      if (event.state.kind === 'disconnected' && event.state.unexpected) {
        ui.toast('error', `Connection lost: ${event.state.reason}`)
      }
      // Only from an outage. Saying "reconnected" after an ordinary first connect
      // would be announcing something that did not happen.
      if (
        event.state.kind === 'connected' &&
        (was === 'reconnecting' || was === 'disconnected')
      ) {
        ui.toast('ok', 'Reconnected. The queue is moving again.')
      }
      break
    }
    case 'sessionClosed':
      sessions.remove(event.id)
      break
    case 'latency':
      sessions.setLatency(event.id, event.ms)
      break
    case 'listing':
      sessions.setListing(event.listing)
      break
    case 'jobUpdate': {
      // Likewise: the previous state is what distinguishes a transition from a repeat.
      // Progress arrives ten times a second, and a toast per tick would be a wall.
      const before = queue.jobs[event.job.id]?.state.kind
      queue.upsert(event.job)
      const now = event.job.state
      const name = event.job.remotePath.split('/').pop() ?? event.job.remotePath
      if (now.kind !== before) {
        if (now.kind === 'failed') {
          // The retry is offered where the failure is announced. Finding the row in
          // the drawer to press the same button is a step with no purpose.
          ui.toast('error', `${name}: ${faultText(now.error)}`, {
            label: 'Retry',
            run: () => {
              commands
                .queueControl({ kind: 'retry', job: event.job.id })
                .catch((error: unknown) => ui.toast('error', faultText(error)))
            },
          })
        }
        // Folders only, and only when they carried something. One toast for a
        // directory is useful; one per file in it is what a queue drawer is for.
        if (now.kind === 'done' && !now.skipped && event.job.kind === 'folder') {
          // A folder is done when every file in it has *stopped*, which is not the same
          // as every file having arrived. Saying "finished" over failures — with a
          // Reveal for a folder that may not exist — is what made them look like noise.
          const children = Object.values(queue.jobs).filter(
            (job) => job.parent === event.job.id,
          )
          const failed = children.filter((job) => job.state.kind === 'failed').length
          if (failed > 0) {
            ui.toast(
              'error',
              `${name}: ${failed} of ${children.length} ${children.length === 1 ? 'file' : 'files'} failed.`,
              {
                label: 'Show',
                run: () => {
                  ui.toggleDrawer(true)
                  ui.setDrawerTab('failed')
                },
              },
            )
          } else {
            const at = event.job.localPath
            ui.toast('ok', `${name} finished.`, {
              label: 'Reveal',
              run: () => {
                commands
                  .revealInFolder(at)
                  .catch((error: unknown) => ui.toast('error', faultText(error)))
              },
            })
          }
        }
      }
      break
    }
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
