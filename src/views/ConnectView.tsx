import { useMemo, useState } from 'react'

import type { ServerConfig } from '@/ipc/gen'
import { parseTarget } from '@/lib/url'

import { ServerEditor, blankServer } from '@/components/ServerEditor'

/** The connect screen: one field, parsed as you type.
 *
 * The chips are not decoration — they are the app showing its work before it dials
 * anything, which is how a mistyped port stops being a mysterious timeout. */
export function ConnectView() {
  const [text, setText] = useState('')
  const [editing, setEditing] = useState<ServerConfig | null>(null)

  const target = useMemo(() => parseTarget(text), [text])
  const ready = target !== null && target.error === null && target.unavailable === null

  /// Hand off to the editor rather than dialling.
  ///
  /// An address is not a set of credentials. This used to assume the SSH agent and
  /// connect immediately, which meant anyone whose server wanted a key file or a
  /// password got "the server rejected these credentials" and no field to fix it in.
  /// One field cannot express an auth method, so it stops at the point where it would
  /// have to guess.
  const proceed = () => {
    if (!target || !ready) return
    setEditing(
      blankServer({
        name: target.host,
        host: target.host,
        port: target.port,
        proto: target.proto,
        username: target.username ?? '',
        initialRemotePath: target.path,
      }),
    )
  }

  return (
    <div className="connect">
      <div className="connect__card">
        <h1 style={{ fontFamily: 'var(--font-display)', fontSize: 22, margin: '0 0 4px' }}>
          Connect
        </h1>
        <p style={{ color: 'var(--ink-soft)', margin: '0 0 16px' }}>
          Type an address to fill in the details, or pick a saved server from the sidebar.
        </p>
        <input
          className="connect__field"
          placeholder="sftp://user@host"
          spellCheck={false}
          autoFocus
          value={text}
          onChange={(e) => setText(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') proceed()
          }}
          aria-label="Server address"
        />

        {target && (
          <div className="chips">
            {target.error ? (
              <span className="chip chip--error">{target.error}</span>
            ) : (
              <>
                <Chip label="protocol" value={target.proto.toUpperCase()} />
                {target.username && <Chip label="user" value={target.username} />}
                <Chip label="host" value={target.host} />
                <Chip label="port" value={String(target.port)} />
                {target.path && <Chip label="path" value={target.path} />}
                {target.protoInferred && target.portExplicit && (
                  <span className="chip chip--warn">inferred from port</span>
                )}
                {target.unavailable && (
                  <span className="chip chip--error">{target.unavailable}</span>
                )}
              </>
            )}
          </div>
        )}

        <div style={{ marginTop: 18 }}>
          <button className="btn btn--primary" disabled={!ready} onClick={proceed}>
            Continue
          </button>
        </div>
      </div>
      {editing && (
        <ServerEditor
          key={editing.id}
          server={editing}
          isNew
          connectOnSave
          onClose={() => {
            setEditing(null)
            setText('')
          }}
        />
      )}
    </div>
  )
}

function Chip({ label, value }: { label: string; value: string }) {
  return (
    <span className="chip">
      <span className="chip__key">{label}</span>
      {value}
    </span>
  )
}
