import { useState } from 'react'

import { commands } from '@/ipc/commands'
import type { AuthMethod, ServerConfig } from '@/ipc/gen'
import { useServersStore } from '@/state/serversStore'
import { useUiStore } from '@/state/uiStore'

/** A server that does not exist yet. Agent auth by default: it is the method that
 * needs no secret stored anywhere, so it is the safest thing to suggest. */
export function blankServer(): ServerConfig {
  return {
    id: crypto.randomUUID(),
    name: '',
    host: '',
    port: 22,
    proto: 'sftp',
    username: '',
    auth: { kind: 'agent' },
    color: null,
    group: null,
    bookmarks: [],
    initialRemotePath: null,
  }
}

type TestState =
  | { kind: 'idle' }
  | { kind: 'testing' }
  | { kind: 'ok'; detail: string }
  | { kind: 'failed'; detail: string }

interface Props {
  server: ServerConfig
  onClose: () => void
}

/** The server form, including "Test connection".
 *
 * Testing runs a real connect through the engine, which means a first-contact host key
 * opens the ordinary trust sheet on top of this one — deliberately. Testing a server
 * is exactly the moment a person is prepared to check a fingerprint. */
export function ServerEditor({ server, onClose }: Props) {
  const [draft, setDraft] = useState<ServerConfig>(server)
  const [test, setTest] = useState<TestState>({ kind: 'idle' })
  const save = useServersStore((s) => s.save)
  const remove = useServersStore((s) => s.remove)
  const toast = useUiStore((s) => s.toast)

  const patch = (fields: Partial<ServerConfig>) => setDraft((d) => ({ ...d, ...fields }))
  const ready = draft.host.trim() !== '' && draft.username.trim() !== ''

  const runTest = async () => {
    setTest({ kind: 'testing' })
    try {
      const info = await commands.sessionTest(draft)
      setTest({
        kind: 'ok',
        detail: [info.hostKeyAlgo, info.cipher, `home ${info.homePath}`]
          .filter((part) => part !== null && part !== '')
          .join(' · '),
      })
    } catch (error) {
      setTest({ kind: 'failed', detail: String(error) })
    }
  }

  const commit = async () => {
    try {
      await save({ ...draft, name: draft.name.trim() || draft.host })
      onClose()
    } catch (error) {
      toast('error', `Could not save: ${String(error)}`)
    }
  }

  return (
    <div className="scrim" role="dialog" aria-modal="true">
      <div className="sheet sheet--wide">
        <h2 className="sheet__title">{server.name === '' ? 'New server' : server.name}</h2>

        <div className="form">
          <Field label="Name">
            <input
              className="input"
              value={draft.name}
              placeholder={draft.host || 'staging'}
              onChange={(e) => patch({ name: e.currentTarget.value })}
              autoFocus
            />
          </Field>
          <Field label="Host">
            <input
              className="input"
              value={draft.host}
              placeholder="sftp.example.com"
              spellCheck={false}
              onChange={(e) => patch({ host: e.currentTarget.value })}
            />
          </Field>
          <Field label="Port">
            <input
              className="input"
              type="number"
              min={1}
              max={65535}
              value={draft.port}
              onChange={(e) => patch({ port: Number(e.currentTarget.value) || 22 })}
            />
          </Field>
          <Field label="Username">
            <input
              className="input"
              value={draft.username}
              spellCheck={false}
              onChange={(e) => patch({ username: e.currentTarget.value })}
            />
          </Field>
          <Field label="Authentication">
            <select
              className="input"
              value={draft.auth.kind}
              onChange={(e) => patch({ auth: authFor(e.currentTarget.value, draft.auth) })}
            >
              <option value="agent">SSH agent</option>
              <option value="keyFile">Key file</option>
              <option value="password">Password (saved in the keychain)</option>
              <option value="ask">Ask every time</option>
            </select>
          </Field>
          {draft.auth.kind === 'keyFile' && (
            <Field label="Key file">
              <input
                className="input"
                value={draft.auth.path}
                placeholder="~/.ssh/id_ed25519"
                spellCheck={false}
                onChange={(e) =>
                  patch({ auth: { kind: 'keyFile', path: e.currentTarget.value } })
                }
              />
            </Field>
          )}
          <Field label="Start in">
            <input
              className="input"
              value={draft.initialRemotePath ?? ''}
              placeholder="the server’s home directory"
              spellCheck={false}
              onChange={(e) =>
                patch({ initialRemotePath: e.currentTarget.value.trim() || null })
              }
            />
          </Field>
        </div>

        {/* Relay never stores a secret here: passwords and passphrases go to the OS
            keychain when they are first entered, keyed by this server's id. */}
        <p className="sheet__note">
          Passwords and key passphrases are kept in the system keychain, never in Relay’s server
          file.
        </p>

        {test.kind !== 'idle' && (
          <p className={`test test--${test.kind}`} role="status">
            {test.kind === 'testing' && 'Connecting…'}
            {test.kind === 'ok' && `Connected. ${test.detail}`}
            {test.kind === 'failed' && test.detail}
          </p>
        )}

        <div className="sheet__actions">
          {server.name !== '' && (
            <button
              className="btn btn--danger"
              onClick={() => {
                void remove(server.id).then(onClose)
              }}
            >
              Delete
            </button>
          )}
          <span style={{ flex: 1 }} />
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button
            className="btn"
            disabled={!ready || test.kind === 'testing'}
            onClick={() => void runTest()}
          >
            Test connection
          </button>
          <button className="btn btn--primary" disabled={!ready} onClick={() => void commit()}>
            Save
          </button>
        </div>
      </div>
    </div>
  )
}

/** Keep a typed key path when switching away and back, so a mistyped selection does
 * not silently discard what was entered. */
function authFor(kind: string, previous: AuthMethod): AuthMethod {
  switch (kind) {
    case 'keyFile':
      return previous.kind === 'keyFile' ? previous : { kind: 'keyFile', path: '' }
    case 'password':
      return { kind: 'password' }
    case 'ask':
      return { kind: 'ask' }
    default:
      return { kind: 'agent' }
  }
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="field">
      <span className="field__label">{label}</span>
      {children}
    </label>
  )
}
