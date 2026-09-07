import { open as openFileDialog } from '@tauri-apps/plugin-dialog'
import { useEffect, useState } from 'react'

import { commands } from '@/ipc/commands'
import type { AuthMethod, Proto, ServerConfig } from '@/ipc/gen'
import { faultText } from '@/lib/errors'
import { useServersStore } from '@/state/serversStore'
import { useUiStore } from '@/state/uiStore'

/** The design's palette for server tags. */
const SWATCHES = ['#2456E6', '#7C4DDB', '#E8A03C', '#2E9E5B', '#0FA3A3', '#D6453C']

const AUTH_METHODS = [
  { value: 'password', label: 'Password' },
  { value: 'keyFile', label: 'Key file' },
  { value: 'agent', label: 'Agent' },
  { value: 'ask', label: 'Ask' },
] as const

const PROTOCOLS: Proto[] = ['sftp', 'ftps', 'ftp']

/** A server that does not exist yet. */
export function blankServer(fields: Partial<ServerConfig> = {}): ServerConfig {
  return {
    id: crypto.randomUUID(),
    name: '',
    host: '',
    port: 22,
    proto: 'sftp',
    username: '',
    auth: { kind: 'password' },
    color: SWATCHES[0] ?? null,
    group: null,
    bookmarks: [],
    initialRemotePath: null,
    ...fields,
  }
}

type TestState =
  | { kind: 'idle' }
  | { kind: 'testing' }
  | { kind: 'ok'; detail: string }
  | { kind: 'failed'; detail: string }

interface Props {
  server: ServerConfig
  /** True for a server that has never been saved, which changes the title and hides
   * Delete. Kept explicit rather than inferred from an empty name. */
  isNew: boolean
  /** Connect as soon as it saves — the path in from Quick Connect. */
  connectOnSave?: boolean
  onClose: () => void
}

/** Connection details and credentials, per the design's server editor.
 *
 * The rules that shaped it:
 *
 * - **A secret is never read back into the webview.** The editor asks whether one is
 *   saved and shows a placeholder; the field starts empty, and an empty field on save
 *   means "leave what is stored alone". Clearing is an explicit button.
 * - **Test works before Save.** Otherwise the only way to check a password is to
 *   commit it to the keychain first, which is exactly backwards.
 * - **The key path comes from a native picker.** A webview file input yields a
 *   sandboxed handle, not a path, and the engine needs a path it can open. */
export function ServerEditor({ server, isNew, connectOnSave = false, onClose }: Props) {
  const [draft, setDraft] = useState<ServerConfig>(server)
  const [password, setPassword] = useState('')
  const [passphrase, setPassphrase] = useState('')
  const [showPassword, setShowPassword] = useState(false)
  const [stored, setStored] = useState({ password: false, passphrase: false })
  const [test, setTest] = useState<TestState>({ kind: 'idle' })
  const [busy, setBusy] = useState(false)

  const save = useServersStore((s) => s.save)
  const remove = useServersStore((s) => s.remove)
  const toast = useUiStore((s) => s.toast)

  useEffect(() => {
    if (isNew) return
    commands
      .secretsStatus(server.id)
      .then(setStored)
      .catch(() => undefined)
  }, [isNew, server.id])

  const patch = (fields: Partial<ServerConfig>) => {
    setDraft((d) => ({ ...d, ...fields }))
    // Any edit invalidates the previous result; a green tick beside changed settings
    // is worse than no tick at all.
    setTest({ kind: 'idle' })
  }

  const auth = draft.auth.kind
  const ready = draft.host.trim() !== '' && draft.username.trim() !== ''

  const browse = async () => {
    const picked = await openFileDialog({
      multiple: false,
      directory: false,
      title: 'Choose a private key',
      // OpenSSH keys usually have no extension at all, so the default filter has to
      // allow everything or it hides exactly what people are looking for.
      filters: [
        { name: 'Private keys', extensions: ['pem', 'ppk', 'key', 'rsa', 'ed25519'] },
        { name: 'All files', extensions: ['*'] },
      ],
    })
    if (typeof picked === 'string') patch({ auth: { kind: 'keyFile', path: picked } })
  }

  const runTest = async () => {
    setTest({ kind: 'testing' })
    try {
      const info = await commands.sessionTest(draft, password, passphrase)
      setTest({
        kind: 'ok',
        detail: [info.hostKeyAlgo, info.cipher, info.homePath]
          .filter((part): part is string => typeof part === 'string' && part !== '')
          .join(' · '),
      })
    } catch (error) {
      setTest({ kind: 'failed', detail: faultText(error) })
    }
  }

  const commit = async () => {
    setBusy(true)
    try {
      const saved = { ...draft, name: draft.name.trim() || draft.host }
      await save(saved)
      // Only write what was actually typed. An untouched field must not wipe a stored
      // credential just because the editor could not show it.
      if (password !== '') await commands.secretsSet(saved.id, 'password', password)
      if (passphrase !== '') await commands.secretsSet(saved.id, 'passphrase', passphrase)
      onClose()
      if (connectOnSave) {
        await commands.sessionOpen(saved.id)
      }
    } catch (error) {
      toast('error', `Could not save: ${faultText(error)}`)
    } finally {
      setBusy(false)
    }
  }

  const clearSecret = async (kind: 'password' | 'passphrase') => {
    await commands.secretsClear(server.id, kind).catch(() => undefined)
    setStored((s) => ({ ...s, [kind]: false }))
    if (kind === 'password') setPassword('')
    else setPassphrase('')
    toast('ok', 'Removed from the keychain.')
  }

  return (
    <div className="scrim" role="dialog" aria-modal="true" onClick={onClose}>
      <div className="editor" onClick={(e) => e.stopPropagation()}>
        <header className="editor__head">
          <span className="editor__badge" style={{ background: draft.color ?? SWATCHES[0] }}>
            <ServerGlyph />
          </span>
          <div style={{ minWidth: 0 }}>
            <div className="editor__title">{isNew ? 'New server' : 'Edit server'}</div>
            <div className="editor__subtitle">Connection details &amp; credentials</div>
          </div>
          <span style={{ flex: 1 }} />
          <button className="iconbtn" onClick={onClose} aria-label="Close">
            ×
          </button>
        </header>

        <div className="editor__body">
          <Field label="Display name">
            <input
              className="input"
              value={draft.name}
              placeholder={draft.host || 'My server'}
              onChange={(e) => patch({ name: e.currentTarget.value })}
              autoFocus
            />
          </Field>

          <div className="editor__split">
            <Field label="Host">
              <input
                className="input input--mono"
                value={draft.host}
                placeholder="sftp.example.com"
                spellCheck={false}
                onChange={(e) => patch({ host: e.currentTarget.value })}
              />
            </Field>
            <Field label="Port" width={84}>
              <input
                className="input input--mono"
                inputMode="numeric"
                value={String(draft.port)}
                onChange={(e) => patch({ port: Number(e.currentTarget.value) || 0 })}
              />
            </Field>
          </div>

          <Field label="Protocol">
            <div className="seg" role="group" aria-label="Protocol">
              {PROTOCOLS.map((proto) => (
                <button
                  key={proto}
                  className={`seg__opt${draft.proto === proto ? ' seg__opt--on' : ''}`}
                  aria-pressed={draft.proto === proto}
                  // Only SFTP ships in 1.0. Disabled and labelled beats a selectable
                  // option that fails at connect time.
                  disabled={proto !== 'sftp'}
                  title={proto === 'sftp' ? undefined : 'Arrives after the SFTP release'}
                  onClick={() => patch({ proto })}
                >
                  {proto.toUpperCase()}
                </button>
              ))}
            </div>
          </Field>

          <hr className="editor__rule" />
          <div className="editor__section">
            <LockGlyph />
            <span>Authentication</span>
          </div>

          <div className="seg" role="group" aria-label="Authentication method">
            {AUTH_METHODS.map((method) => (
              <button
                key={method.value}
                className={`seg__opt${auth === method.value ? ' seg__opt--on' : ''}`}
                aria-pressed={auth === method.value}
                onClick={() => patch({ auth: authFor(method.value, draft.auth) })}
              >
                {method.label}
              </button>
            ))}
          </div>

          <Field label="Username">
            <input
              className="input input--mono"
              value={draft.username}
              placeholder="deploy"
              spellCheck={false}
              onChange={(e) => patch({ username: e.currentTarget.value })}
            />
          </Field>

          {auth === 'password' && (
            <Field label="Password">
              <div className="input input--group">
                <input
                  className="input__inner"
                  type={showPassword ? 'text' : 'password'}
                  value={password}
                  placeholder={stored.password ? 'Saved in the keychain' : '••••••••'}
                  onChange={(e) => setPassword(e.currentTarget.value)}
                />
                <button
                  className="iconbtn"
                  onClick={() => setShowPassword((v) => !v)}
                  aria-label={showPassword ? 'Hide the password' : 'Show the password'}
                >
                  {showPassword ? '🙈' : '👁'}
                </button>
                {stored.password && (
                  <button
                    className="iconbtn"
                    title="Remove from the keychain"
                    aria-label="Remove the saved password"
                    onClick={() => void clearSecret('password')}
                  >
                    ✕
                  </button>
                )}
              </div>
            </Field>
          )}

          {draft.auth.kind === 'keyFile' && (
            <>
              <Field label="Private key file">
                <div className="editor__browse">
                  <input
                    className="input input--mono"
                    value={draft.auth.path}
                    placeholder="~/.ssh/id_ed25519"
                    spellCheck={false}
                    onChange={(e) =>
                      patch({ auth: { kind: 'keyFile', path: e.currentTarget.value } })
                    }
                  />
                  <button className="btn" onClick={() => void browse()}>
                    Browse…
                  </button>
                </div>
              </Field>
              <p className="editor__hint">
                OpenSSH, PEM, and PuTTY <code>.ppk</code> keys all work.
              </p>
              <Field label="Key passphrase (optional)">
                <div className="input input--group">
                  <input
                    className="input__inner"
                    type="password"
                    value={passphrase}
                    placeholder={stored.passphrase ? 'Saved in the keychain' : '••••••••'}
                    onChange={(e) => setPassphrase(e.currentTarget.value)}
                  />
                  {stored.passphrase && (
                    <button
                      className="iconbtn"
                      title="Remove from the keychain"
                      aria-label="Remove the saved passphrase"
                      onClick={() => void clearSecret('passphrase')}
                    >
                      ✕
                    </button>
                  )}
                </div>
              </Field>
            </>
          )}

          {(auth === 'agent' || auth === 'ask') && (
            <p className="editor__note">
              {auth === 'agent'
                ? 'Relay will offer identities from your running SSH agent — nothing is stored here. Check what it holds with `ssh-add -l`.'
                : 'You will be asked for your password each time you connect. Nothing is saved.'}
            </p>
          )}

          <Field label="Start in">
            <input
              className="input input--mono"
              value={draft.initialRemotePath ?? ''}
              placeholder="the server’s home directory"
              spellCheck={false}
              onChange={(e) =>
                patch({ initialRemotePath: e.currentTarget.value.trim() || null })
              }
            />
          </Field>

          <hr className="editor__rule" />
          <Field label="Colour tag">
            <div className="swatches">
              {SWATCHES.map((colour) => (
                <button
                  key={colour}
                  className={`swatch${draft.color === colour ? ' swatch--on' : ''}`}
                  style={{ background: colour }}
                  aria-label={`Tag colour ${colour}`}
                  aria-pressed={draft.color === colour}
                  onClick={() => patch({ color: colour })}
                />
              ))}
            </div>
          </Field>

          <p className="editor__where">
            <b>Where credentials live:</b> passwords and passphrases go straight into your
            system keychain, never plain text. Key files stay on disk — Relay only remembers the
            path.
          </p>

          {test.kind !== 'idle' && (
            <p className={`test test--${test.kind}`} role="status">
              {test.kind === 'testing' && 'Connecting…'}
              {test.kind === 'ok' && `Connected. ${test.detail}`}
              {test.kind === 'failed' && test.detail}
            </p>
          )}
        </div>

        <footer className="editor__foot">
          <button
            className={`btn btn--test btn--test-${test.kind}`}
            disabled={!ready || test.kind === 'testing'}
            onClick={() => void runTest()}
          >
            {test.kind === 'testing' && <span className="spinner" aria-hidden />}
            {test.kind === 'ok' && '✓ '}
            {test.kind === 'failed' && '✕ '}
            {test.kind === 'testing'
              ? 'Testing…'
              : test.kind === 'ok'
                ? 'Connection OK'
                : test.kind === 'failed'
                  ? 'Test failed'
                  : 'Test connection'}
          </button>
          <span style={{ flex: 1 }} />
          {!isNew && (
            <button
              className="btn btn--danger"
              onClick={() => {
                void remove(server.id).then(onClose)
              }}
            >
              Delete
            </button>
          )}
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button
            className="btn btn--primary"
            disabled={!ready || busy}
            onClick={() => void commit()}
          >
            {connectOnSave ? 'Save & connect' : 'Save server'}
          </button>
        </footer>
      </div>
    </div>
  )
}

/** Keep a typed key path when switching away and back, so a mis-click does not
 * silently discard what was entered. */
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

function Field({
  label,
  width,
  children,
}: {
  label: string
  width?: number
  children: React.ReactNode
}) {
  return (
    <label className="field" style={width === undefined ? undefined : { width, flex: 'none' }}>
      <span className="field__label">{label}</span>
      {children}
    </label>
  )
}

function ServerGlyph() {
  return (
    <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="#fff" strokeWidth="2">
      <rect x="2" y="3" width="20" height="8" rx="2" />
      <rect x="2" y="13" width="20" height="8" rx="2" />
      <path d="M6 7h.01M6 17h.01" />
    </svg>
  )
}

function LockGlyph() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      aria-hidden
    >
      <rect x="3" y="11" width="18" height="10" rx="2" />
      <path d="M7 11V7a5 5 0 0 1 10 0v4" />
    </svg>
  )
}
