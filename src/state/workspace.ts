/** Where the person left off: recorded as it changes, reopened on launch when the
 * setting asks for it. The engine keeps the file; see `relay_core::workspace`. */

import { commands } from '@/ipc/commands'
import type { ServerConfig, Workspace } from '@/ipc/gen'

import { useSessionsStore } from './sessionsStore'

type SessionsState = ReturnType<typeof useSessionsStore.getState>

/** The workspace as the tab strip shows it now. */
export function currentWorkspace(state: SessionsState): Workspace {
  return {
    tabs: state.order.flatMap((id) => {
      const session = state.sessions[id]
      if (!session) return []
      return [
        {
          serverId: session.serverId,
          // Empty until the pane has listed something; nothing to return to yet.
          localPath: state.panes[id]?.localPath || null,
          remotePath: session.remotePath,
          active: id === state.activeId,
        },
      ]
    }),
  }
}

/** Keep the engine's copy current, from now until the returned function is called.
 *
 * Started only once a launch has decided what to open. Before that the tab strip is the
 * empty one the app starts with, and recording it would erase the workspace that was
 * about to be restored. Sends only when something a restore would use has changed —
 * a progress tick or a sort is not a reason to write a file. */
export function recordWorkspace(): () => void {
  let last = ''
  const send = (state: SessionsState) => {
    const workspace = currentWorkspace(state)
    const json = JSON.stringify(workspace)
    if (json === last) return
    last = json
    // A lost write costs a restore its latest folder, not the app anything; there is
    // nobody to tell mid-navigation.
    commands.workspaceSet(workspace).catch(() => undefined)
  }
  send(useSessionsStore.getState())
  return useSessionsStore.subscribe(send)
}

/** Reopen the tabs a workspace names, each in the folders it was in. Returns whether
 * anything opened, so a launch with nothing to restore can open the way it always did.
 *
 * Servers deleted since are skipped. Tabs open one after another, so they come back in
 * the order they were in, and the tab that was showing is shown again. */
export async function restoreWorkspace(
  workspace: Workspace,
  servers: ServerConfig[],
): Promise<boolean> {
  const store = useSessionsStore.getState()
  const known = new Set(servers.map((server) => server.id))
  let opened = false
  for (const tab of workspace.tabs) {
    if (!known.has(tab.serverId)) continue
    // Queued before the session exists, so its pane finds the folder waiting for it
    // however early it mounts.
    store.queueLocalStart(tab.serverId, tab.localPath)
    try {
      const id = await commands.sessionOpen(tab.serverId, tab.remotePath)
      opened = true
      if (tab.active) store.activate(id)
    } catch {
      // One server that will not open is no reason to lose the others.
    }
  }
  return opened
}
