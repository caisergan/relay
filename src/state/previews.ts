/** Opening a server's file in an application on this computer.
 *
 * Nothing here can open a file where it is, so a preview is a download first: into a
 * folder of Relay's own under the temporary directory (see `relay_core::preview`), then
 * handed to the application. The download is an ordinary queued transfer, which is what
 * gives a large file a progress bar and lets it survive a dropped connection. */

import { commands } from '@/ipc/commands'
import type { JobSnapshot } from '@/ipc/gen'
import { appName } from '@/lib/apps'
import { faultText } from '@/lib/errors'

import { useUiStore, type PreviewRequest } from './uiStore'

/** Previews on their way, by the path each is downloading to.
 *
 * Keyed by path rather than by job, because the path is known before the job is — and a
 * small file can arrive before the call that queued it has returned its id. */
const waiting = new Map<string, { app: string; request: PreviewRequest }>()

/** Open a file in the application remembered for previews, or ask which one. */
export async function preview(request: PreviewRequest): Promise<void> {
  const { previewApp } = await commands.settingsGet()
  if (previewApp) await previewWith(request, previewApp)
  else useUiStore.getState().askPreview(request)
}

/** Download a file for previewing, and open it in `app` once it has arrived. */
export async function previewWith(request: PreviewRequest, app: string): Promise<void> {
  const localPath = await commands.previewPrepare(request.name)
  waiting.set(localPath, { app, request })
  try {
    await commands.queueEnqueue(crypto.randomUUID(), [
      {
        session: request.session,
        serverId: request.serverId,
        direction: 'down',
        remotePath: request.remotePath,
        localPath,
        isDir: false,
      },
    ])
  } catch (error) {
    waiting.delete(localPath)
    throw error
  }
}

/** A job's state changed. A preview that has arrived opens; one that will not arrive is
 * forgotten. A failure is not that: the toast announcing it offers a retry, and a retry
 * that succeeds should still end with the file open. */
export function notePreviewState(job: JobSnapshot): void {
  const entry = waiting.get(job.localPath)
  if (!entry) return
  const state = job.state
  if (state.kind === 'cancelled') waiting.delete(job.localPath)
  if (state.kind !== 'done') return
  waiting.delete(job.localPath)
  if (state.skipped) return
  commands.previewOpen(job.localPath, entry.app).catch((error: unknown) => {
    // Most often an application that has since been moved or removed, so the way out
    // is to pick another rather than to try the same one again.
    const ui = useUiStore.getState()
    ui.toast(
      'error',
      `Could not open ${entry.request.name} in ${appName(entry.app)}: ${faultText(error)}`,
      { label: 'Choose app', run: () => ui.askPreview(entry.request) },
    )
  })
}
