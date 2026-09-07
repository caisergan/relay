/** Typed wrappers over the Rust command surface.
 *
 * Types come from `gen.ts`, which is generated from `relay-core`; these wrappers are
 * hand-written and deliberately thin. Tauri maps camelCase arguments onto the Rust
 * snake_case parameters, so the names here must match the command signatures. */

import { Channel, invoke } from '@tauri-apps/api/core'
import type {
  EngineEnvelope,
  EngineSnapshot,
  JobSnapshot,
  ListingSnapshot,
  LocalEntry,
  LogLine,
  PromptReply,
  QueueOp,
  RemoteEntry,
  SecretKind,
  SecretStatus,
  ServerConfig,
  ServerInfo,
  Settings,
  TransferItem,
} from './gen'

/** Rust's `()` arrives as JSON `null`. */
type Unit = null

export const commands = {
  serversList: () => invoke<ServerConfig[]>('servers_list'),
  serversSave: (config: ServerConfig) => invoke<ServerConfig>('servers_save', { config }),
  serversDelete: (id: string) => invoke<Unit>('servers_delete', { id }),

  sessionOpen: (serverId: string) => invoke<string>('session_open', { serverId }),
  /** Connect, report what the peer said, hang up. Uses the real host-key sheet.
   * `password`/`passphrase` are what is typed but not yet saved, so a credential can
   * be checked without committing it to the keychain first. */
  sessionTest: (config: ServerConfig, password?: string, passphrase?: string) =>
    invoke<ServerInfo>('session_test', {
      config,
      password: password ?? null,
      passphrase: passphrase ?? null,
    }),

  secretsStatus: (id: string) => invoke<SecretStatus>('secrets_status', { id }),
  /** An empty value clears the entry. */
  secretsSet: (id: string, kind: SecretKind, value: string) =>
    invoke<Unit>('secrets_set', { id, kind, value }),
  secretsClear: (id: string, kind: SecretKind) => invoke<Unit>('secrets_clear', { id, kind }),
  sessionClose: (id: string) => invoke<Unit>('session_close', { id }),
  sessionReconnect: (id: string) => invoke<Unit>('session_reconnect', { id }),
  sessionListDir: (id: string, path: string) =>
    invoke<RemoteEntry[]>('session_list_dir', { id, path }),
  sessionMkdir: (id: string, path: string) => invoke<Unit>('session_mkdir', { id, path }),
  sessionRename: (id: string, from: string, to: string) =>
    invoke<Unit>('session_rename', { id, from, to }),
  sessionRemove: (id: string, path: string, isDir: boolean) =>
    invoke<Unit>('session_remove', { id, path, isDir }),
  sessionLogs: (id: string, limit = 200) => invoke<LogLine[]>('session_logs', { id, limit }),

  localListDir: (path: string) => invoke<LocalEntry[]>('local_list_dir', { path }),
  localDefaultDir: () => invoke<string>('local_default_dir'),
  localRoots: () => invoke<string[]>('local_roots'),

  /** One gesture, one batch. The id comes from here so a re-sent command is
   *  recognised as the same request rather than queueing everything twice. */
  queueEnqueue: (batch: string, items: TransferItem[]) =>
    invoke<string[]>('queue_enqueue', { batch, items }),
  queueControl: (op: QueueOp) => invoke<Unit>('queue_control', { op }),

  /** Show a file in Finder or Explorer. The engine opens it; the webview is not
   *  given a general permission to open paths. */
  revealInFolder: (path: string) => invoke<Unit>('reveal_in_folder', { path }),

  resolvePrompt: (promptId: string, reply: PromptReply) =>
    invoke<Unit>('resolve_prompt', { promptId, reply }),

  settingsGet: () => invoke<Settings>('settings_get'),
  settingsSet: (settings: Settings) => invoke<Settings>('settings_set', { settings }),

  engineSubscribe: (channel: Channel<EngineEnvelope>) =>
    invoke<string>('engine_subscribe', { channel }),
  engineUnsubscribe: (id: string) => invoke<Unit>('engine_unsubscribe', { id }),
  engineSnapshot: (subscriptionId: string) =>
    invoke<EngineSnapshot>('engine_snapshot', { subscriptionId }),
}

export { Channel }
export type { EngineEnvelope, EngineSnapshot, JobSnapshot, ListingSnapshot }
