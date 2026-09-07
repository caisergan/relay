import { useMemo, useState } from 'react'

import { commands } from '@/ipc/commands'
import { parseTarget } from '@/lib/url'
import { useServersStore } from '@/state/serversStore'
import { useUiStore } from '@/state/uiStore'

/** The connect screen: one field, parsed as you type.
 *
 * The chips are not decoration — they are the app showing its work before it dials
 * anything, which is how a mistyped port stops being a mysterious timeout. */
export function ConnectView() {
  const [text, setText] = useState('')
  const toast = useUiStore((s) => s.toast)
  const save = useServersStore((s) => s.save)

  const target = useMemo(() => parseTarget(text), [text])
  const ready = target !== null && target.error === null && target.unavailable === null

  const connect = async () => {
    if (!target || !ready) return
    const config = {
      id: crypto.randomUUID(),
      name: target.host,
      host: target.host,
      port: target.port,
      proto: target.proto,
      username: target.username ?? '',
      auth: { kind: 'agent' } as const,
      color: null,
      group: null,
      bookmarks: [],
      initialRemotePath: target.path,
    }
    try {
      await save(config)
      await commands.sessionOpen(config.id)
      setText('')
    } catch (error) {
      toast('error', `Could not connect: ${String(error)}`)
    }
  }

  return (
    <div className="connect">
      <div className="connect__card">
        <h1 style={{ fontFamily: 'var(--font-display)', fontSize: 22, margin: '0 0 4px' }}>
          Connect
        </h1>
        <p style={{ color: 'var(--ink-soft)', margin: '0 0 16px' }}>
          Type an address, or pick a saved server from the sidebar.
        </p>
        <input
          className="connect__field"
          placeholder="sftp://user@host"
          spellCheck={false}
          autoFocus
          value={text}
          onChange={(e) => setText(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void connect()
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
          <button className="btn btn--primary" disabled={!ready} onClick={() => void connect()}>
            Connect
          </button>
        </div>
      </div>
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
