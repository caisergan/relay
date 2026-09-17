/** Typed wrappers over the Rust command surface.
 *
 * Types come from `gen.ts`, which is generated from `relay-core`; these wrappers are
 * hand-written and deliberately thin. Tauri maps camelCase arguments onto the Rust
 * snake_case parameters, so the names here must match the command signatures. */

import { Channel, invoke } from '@tauri-apps/api/core'
import type {
  DirSize,
  EngineEnvelope,
  EngineSnapshot,
  InterfaceLayout,
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
  Workspace,
} from './gen'

/** Rust's `()` arrives as JSON `null`. */
type Unit = null

export const commands = {
  serversList: () => invoke<ServerConfig[]>('servers_list'),
  serversSave: (config: ServerConfig) => invoke<ServerConfig>('servers_save', { config }),
  serversDelete: (id: string) => invoke<Unit>('servers_delete', { id }),

  /** `startPath` opens the remote pane somewhere other than the server's own starting
   *  folder — a restored tab. The engine falls back to home when it is gone. */
  sessionOpen: (serverId: string, startPath?: string | null) =>
    invoke<string>('session_open', { serverId, startPath: startPath ?? null }),
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
  /** Lists `path` again only if the pane has not moved on from it by the time the session
   * gets there; resolves to whether it listed. For refreshes nobody asked for. */
  sessionRelist: (id: string, path: string) => invoke<boolean>('session_relist', { id, path }),
  /** What is at a path, following a link; null when nothing is. */
  sessionStat: (id: string, path: string) =>
    invoke<RemoteEntry | null>('session_stat', { id, path }),
  /** Walks a remote folder to total it up. Slow by nature: one listing per directory. */
  sessionMeasure: (id: string, path: string) =>
    invoke<DirSize>('session_measure', { id, path }),
  sessionMkdir: (id: string, path: string) => invoke<Unit>('session_mkdir', { id, path }),
  sessionRename: (id: string, from: string, to: string) =>
    invoke<Unit>('session_rename', { id, from, to }),
  sessionRemove: (id: string, path: string, isDir: boolean) =>
    invoke<Unit>('session_remove', { id, path, isDir }),
  sessionLogs: (id: string, limit = 200) => invoke<LogLine[]>('session_logs', { id, limit }),

  localListDir: (path: string) => invoke<LocalEntry[]>('local_list_dir', { path }),
  /** A link is described as a link, with its target's kind; null when nothing is there. */
  localStat: (path: string) => invoke<LocalEntry | null>('local_stat', { path }),
  localMeasure: (path: string) => invoke<DirSize>('local_measure', { path }),
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
  /** A fresh local path to download a preview of a server file called `name` to. */
  previewPrepare: (name: string) => invoke<string>('preview_prepare', { name }),
  /** Open a downloaded preview in an application; refused for any other path. */
  previewOpen: (path: string, app: string) => invoke<Unit>('preview_open', { path, app }),

  resolvePrompt: (promptId: string, reply: PromptReply) =>
    invoke<Unit>('resolve_prompt', { promptId, reply }),

  settingsGet: () => invoke<Settings>('settings_get'),
  settingsSet: (settings: Settings) => invoke<Settings>('settings_set', { settings }),

  layoutGet: () => invoke<InterfaceLayout>('layout_get'),
  layoutSet: (layout: InterfaceLayout) => invoke<Unit>('layout_set', { layout }),

  workspaceGet: () => invoke<Workspace>('workspace_get'),
  workspaceSet: (workspace: Workspace) => invoke<Unit>('workspace_set', { workspace }),

  engineSubscribe: (channel: Channel<EngineEnvelope>) =>
    invoke<string>('engine_subscribe', { channel }),
  engineUnsubscribe: (id: string) => invoke<Unit>('engine_unsubscribe', { id }),
  engineSnapshot: (subscriptionId: string) =>
    invoke<EngineSnapshot>('engine_snapshot', { subscriptionId }),
}

export { Channel }
export type { EngineEnvelope, EngineSnapshot, JobSnapshot, ListingSnapshot }
