import { useState } from 'react'

import { FileIcon } from '@react-symbols/icons/utils'

import { commands } from '@/ipc/commands'
import type { FileFacts, PromptReply, PromptRequest } from '@/ipc/gen'
import { EXTENSIONS, NAMES } from '@/lib/fileIcon'
import { formatBytes, formatWhen } from '@/lib/format'
import { useUiStore } from '@/state/uiStore'

import { IconArrowLeft, IconArrowRight } from './Icons'

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
  const [applyToRemaining, setApplyToRemaining] = useState(false)
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
            <div className="sheet__actions">
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
            <h2 className="sheet__title">{fileName(destination(prompt))} already exists</h2>
            <p className="sheet__body">
              in <span className="conflict__where">{parentOf(destination(prompt).path)}</span>
            </p>
            {/* Local on the left and remote on the right, always — the same sides they
                occupy in the panes behind the sheet. What swaps is the arrow between
                them, which is the only thing that has to say which way the bytes go.
                The previous version said it in the two headings instead, so answering
                "which of these am I about to lose?" meant reading the word
                "Replacing" and working out which card it sat above. */}
            <div className="conflict">
              <ConflictSide
                place="On this Mac"
                facts={prompt.local}
                doomed={prompt.direction === 'down'}
                newer={newerIs(prompt.local.modified, prompt.remote.modified) === 'local'}
                differs={differing(prompt.local, prompt.remote)}
              />
              <div className="conflict__flow" aria-hidden>
                {prompt.direction === 'down' ? (
                  <IconArrowLeft size={16} />
                ) : (
                  <IconArrowRight size={16} />
                )}
              </div>
              <ConflictSide
                place="On the server"
                facts={prompt.remote}
                doomed={prompt.direction === 'up'}
                newer={newerIs(prompt.local.modified, prompt.remote.modified) === 'remote'}
                differs={differing(prompt.local, prompt.remote)}
              />
            </div>
            {prompt.remaining > 0 && (
              <label className="sheet__check">
                <input
                  type="checkbox"
                  checked={applyToRemaining}
                  onChange={(e) => setApplyToRemaining(e.currentTarget.checked)}
                />
                Do this for the other {prompt.remaining}{' '}
                {prompt.remaining === 1 ? 'file' : 'files'}
              </label>
            )}
            <div className="sheet__actions">
              <button
                className="btn"
                onClick={() => answer({ kind: 'conflict', action: 'skip', applyToRemaining })}
              >
                Skip
              </button>
              <button
                className="btn"
                onClick={() =>
                  answer({ kind: 'conflict', action: 'keepBoth', applyToRemaining })
                }
              >
                Keep both
              </button>
              {/* Offered only when the engine has already proven a resume would be
                  safe. A button that might refuse is worse than no button. */}
              {prompt.resumeAllowed && (
                <button
                  className="btn"
                  onClick={() =>
                    answer({ kind: 'conflict', action: 'resume', applyToRemaining })
                  }
                >
                  Resume
                </button>
              )}
              <button
                className="btn btn--primary"
                onClick={() =>
                  answer({ kind: 'conflict', action: 'overwrite', applyToRemaining })
                }
              >
                Overwrite
              </button>
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

/** One half of the comparison.
 *
 * The two facts are on their own rows with their own labels rather than run together
 * as `4.9 KB · 7 Eyl 00:04`. Both cards nearly always show the same filename, so the
 * only thing worth reading is where the two disagree, and a single line of
 * dot-separated values makes finding that a diffing exercise. Rows line up across the
 * gap; a value that matches the other side is dimmed and one that does not is not. */
function ConflictSide({
  place,
  facts,
  doomed,
  newer,
  differs,
}: {
  place: string
  facts: FileFacts
  /** This is the copy about to be replaced. */
  doomed: boolean
  newer: boolean
  differs: { size: boolean; modified: boolean }
}) {
  return (
    <div className={`conflict__side${doomed ? ' conflict__side--doomed' : ''}`}>
      <div className="conflict__head">
        <span className="conflict__label">{place}</span>
        {doomed && <span className="conflict__tag">replaced</span>}
        {newer && <span className="conflict__badge">newer</span>}
      </div>
      <div className="conflict__file">
        <FileIcon
          fileName={fileName(facts)}
          autoAssign
          editFileExtensionData={EXTENSIONS}
          editFileNameData={NAMES}
          width={18}
          height={18}
        />
        <span className="conflict__path" title={facts.path}>
          {fileName(facts)}
        </span>
      </div>
      <dl className="conflict__facts">
        <dt>Size</dt>
        <dd className={differs.size ? 'conflict__val conflict__val--differs' : 'conflict__val'}>
          {formatBytes(facts.size)}
        </dd>
        <dt>Modified</dt>
        <dd
          className={
            differs.modified ? 'conflict__val conflict__val--differs' : 'conflict__val'
          }
        >
          {/* Never a bare dash. A missing timestamp is the one fact most likely to
              decide the answer, and "—" reads as a rendering failure rather than as
              "the server did not say". */}
          {facts.modified === null ? (
            <span className="conflict__unknown">not known</span>
          ) : (
            formatWhen(facts.modified)
          )}
        </dd>
      </dl>
    </div>
  )
}

/** The last segment of a path, which is what both cards show as the name. */
function fileName(facts: FileFacts): string {
  return facts.path.split('/').pop() ?? facts.path
}

/** The directory holding a path, for the line under the title. */
function parentOf(path: string): string {
  const cut = path.lastIndexOf('/')
  return cut > 0 ? path.slice(0, cut) : '/'
}

/** The copy that is about to be overwritten — the one the question is really about. */
function destination(prompt: {
  local: FileFacts
  remote: FileFacts
  direction: string
}): FileFacts {
  return prompt.direction === 'down' ? prompt.local : prompt.remote
}

/** Which facts disagree between the two sides.
 *
 * Drives the emphasis: matching values recede so the differing ones are what the eye
 * lands on. Two files of identical size and time are worth showing as exactly that,
 * because it usually means the transfer already happened. */
function differing(local: FileFacts, remote: FileFacts): { size: boolean; modified: boolean } {
  return {
    size: local.size !== remote.size,
    modified: local.modified !== remote.modified,
  }
}

/** Which side was modified more recently, when both are known.
 *
 * `null` rather than a guess when either timestamp is missing: a server that withholds
 * mtimes would otherwise have every file marked "newer" on the side that has one.
 */
function newerIs(local: string | null, remote: string | null): 'local' | 'remote' | null {
  if (local === null || remote === null) return null
  if (local === remote) return null
  return local > remote ? 'local' : 'remote'
}
