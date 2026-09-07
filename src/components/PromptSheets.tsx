import { useState } from 'react'

import { commands } from '@/ipc/commands'
import type { PromptReply, PromptRequest } from '@/ipc/gen'
import { formatBytes, formatWhen } from '@/lib/format'
import { useUiStore } from '@/state/uiStore'

/** Renders whatever the engine says is unanswered.
 *
 * Keyed by prompt id and driven entirely from store state, so a reload rebuilds the
 * open sheet rather than opening a second one — and answering twice is harmless
 * because the broker rejects the duplicate. */
export function PromptSheets() {
  const prompts = useUiStore((s) => s.prompts)
  const entries = Object.values(prompts)
  const first = entries[0]
  if (!first) return null
  return <PromptSheet key={first.id} request={first} />
}

function PromptSheet({ request }: { request: PromptRequest }) {
  const [password, setPassword] = useState('')
  const [remember, setRemember] = useState(true)
  const toast = useUiStore((s) => s.toast)

  const answer = (reply: PromptReply) => {
    commands.resolvePrompt(request.id, reply).catch(() => {
      // Already answered elsewhere, or the session closed. Nothing to recover.
      toast('info', 'That request was already handled.')
    })
  }

  const prompt = request.prompt
  const danger = prompt.kind === 'hostKey' && prompt.changed

  return (
    <div className="scrim" role="dialog" aria-modal="true">
      <div className={`sheet${danger ? ' sheet--danger' : ''}`}>
        {prompt.kind === 'hostKey' && (
          <>
            <h2 className="sheet__title">
              {prompt.changed ? 'This server’s key has changed' : 'Trust this server?'}
            </h2>
            <p className="sheet__body">
              {prompt.changed
                ? `The host key for ${prompt.host} does not match the one Relay pinned. This can mean the server was rebuilt — or that something is intercepting the connection.`
                : `Relay has not seen ${prompt.host} before. Check the fingerprint against one you trust.`}
            </p>
            <p className="fingerprint">
              {prompt.algo} · {prompt.sha256}
            </p>
            <label style={{ display: 'flex', gap: 8, marginBottom: 14 }}>
              <input
                type="checkbox"
                checked={remember}
                onChange={(e) => setRemember(e.currentTarget.checked)}
              />
              Remember this key
            </label>
            <div className="sheet__actions">
              <button className="btn" onClick={() => answer({ kind: 'deny' })}>
                Cancel
              </button>
              <button
                className={`btn ${danger ? 'btn--danger' : 'btn--primary'}`}
                onClick={() => answer({ kind: 'accept', remember })}
              >
                {prompt.changed ? 'Accept the new key' : 'Trust and connect'}
              </button>
            </div>
          </>
        )}

        {prompt.kind === 'password' && (
          <>
            <h2 className="sheet__title">Password required</h2>
            <p className="sheet__body">{prompt.hint}</p>
            <input
              className="connect__field"
              type="password"
              autoFocus
              value={password}
              onChange={(e) => setPassword(e.currentTarget.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') answer({ kind: 'password', value: password })
              }}
            />
            <div className="sheet__actions" style={{ marginTop: 16 }}>
              <button className="btn" onClick={() => answer({ kind: 'deny' })}>
                Cancel
              </button>
              <button
                className="btn btn--primary"
                onClick={() => answer({ kind: 'password', value: password })}
              >
                Continue
              </button>
            </div>
          </>
        )}

        {prompt.kind === 'conflict' && (
          <>
            <h2 className="sheet__title">That file already exists</h2>
            <p className="sheet__body">
              {prompt.local.path} — {formatBytes(prompt.local.size)} ·{' '}
              {formatWhen(prompt.local.modified)}
              <br />
              {prompt.remote.path} — {formatBytes(prompt.remote.size)} ·{' '}
              {formatWhen(prompt.remote.modified)}
            </p>
            <div className="sheet__actions">
              {(['skip', 'keepBoth', 'overwrite'] as const).map((action) => (
                <button
                  key={action}
                  className={`btn${action === 'overwrite' ? ' btn--primary' : ''}`}
                  onClick={() => answer({ kind: 'conflict', action, applyToRemaining: false })}
                >
                  {action === 'keepBoth' ? 'Keep both' : action}
                </button>
              ))}
              {prompt.resumeAllowed && (
                <button
                  className="btn"
                  onClick={() =>
                    answer({ kind: 'conflict', action: 'resume', applyToRemaining: false })
                  }
                >
                  Resume
                </button>
              )}
            </div>
          </>
        )}

        {prompt.kind === 'tlsCert' && (
          <>
            <h2 className="sheet__title">Certificate not trusted</h2>
            <p className="sheet__body">{prompt.host}</p>
            <p className="fingerprint">{prompt.sha256}</p>
            <div className="sheet__actions">
              <button className="btn" onClick={() => answer({ kind: 'deny' })}>
                Cancel
              </button>
              <button
                className="btn btn--primary"
                onClick={() => answer({ kind: 'accept', remember })}
              >
                Trust
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  )
}
