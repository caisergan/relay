import { useState } from 'react'

import { commands } from '@/ipc/commands'
import { appName, chooseApplication } from '@/lib/apps'
import { faultText } from '@/lib/errors'
import { previewWith } from '@/state/previews'
import { useUiStore, type PreviewRequest } from '@/state/uiStore'

/** "Open with…", for a server file double-clicked while no application is remembered.
 *
 * The application itself is chosen in the system's own picker; this sheet only frames
 * the question and asks whether the answer should stick. Choosing does not open the
 * file by itself, because "just this once" and "always" are different answers and the
 * picker has nowhere to ask for the difference. */
export function PreviewSheet() {
  const request = useUiStore((s) => s.previewAsk)
  if (!request) return null
  // Keyed so a second file asked about while this one is open starts from no choice.
  return <AskApplication key={`${request.session}:${request.remotePath}`} request={request} />
}

function AskApplication({ request }: { request: PreviewRequest }) {
  const toast = useUiStore((s) => s.toast)
  const askPreview = useUiStore((s) => s.askPreview)
  const [app, setApp] = useState<string | null>(null)

  const choose = () => {
    chooseApplication()
      .then((chosen) => {
        if (chosen !== null) setApp(chosen)
      })
      .catch((error: unknown) => toast('error', faultText(error)))
  }

  const open = (chosen: string, remember: boolean) => {
    askPreview(null)
    if (remember) {
      // Remembering and opening are separate: a settings file that cannot be written
      // is a reason to say so, not a reason to leave the file unopened.
      commands
        .settingsGet()
        .then((settings) => commands.settingsSet({ ...settings, previewApp: chosen }))
        .catch((error: unknown) =>
          toast('error', `Could not remember ${appName(chosen)}: ${faultText(error)}`),
        )
    }
    previewWith(request, chosen).catch((error: unknown) => toast('error', faultText(error)))
  }

  return (
    <div className="scrim" role="dialog" aria-modal="true" aria-label="Open with">
      <div className="sheet">
        <h2 className="sheet__title">Open {request.name} with…</h2>
        <p className="sheet__body">
          Relay downloads a copy to a temporary folder on this Mac and opens it there. Changes
          to the copy stay on this Mac.
        </p>
        <div className="openwith">
          <span className={`openwith__app${app === null ? ' openwith__app--none' : ''}`}>
            {app === null ? 'No application chosen' : appName(app)}
          </span>
          <button className="btn btn--small" onClick={choose}>
            {app === null ? 'Choose Application…' : 'Choose Another…'}
          </button>
        </div>
        <div className="sheet__actions">
          <button className="btn" onClick={() => askPreview(null)}>
            Cancel
          </button>
          <button
            className="btn"
            disabled={app === null}
            onClick={() => app !== null && open(app, false)}
          >
            Just This Once
          </button>
          <button
            className="btn btn--primary"
            disabled={app === null}
            onClick={() => app !== null && open(app, true)}
          >
            {app === null ? 'Always Use It' : `Always Use ${appName(app)}`}
          </button>
        </div>
      </div>
    </div>
  )
}
