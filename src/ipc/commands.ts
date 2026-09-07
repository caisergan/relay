/** Typed wrappers over the Rust command surface.
 *
 * Types come from `gen.ts`, which is generated from `relay-core`; these wrappers are
 * hand-written and deliberately thin. Tauri maps camelCase arguments onto the Rust
 * snake_case parameters, so the names here must match the command signatures. */

import { Channel, invoke } from '@tauri-apps/api/core'
import type {
  Direction,
  EngineEnvelope,
  EngineSnapshot,
  JobSnapshot,
  ListingSnapshot,
  LocalEntry,
  LogLine,
  PromptReply,
  QueueOp,
  RemoteEntry,
  ServerConfig,
  ServerInfo,
  Settings,
} from './gen'

/** Rust's `()` arrives as JSON `null`. */
type Unit = null

export const commands = {
  serversList: () => invoke<ServerConfig[]>('servers_list'),
  serversSave: (config: ServerConfig) => invoke<ServerConfig>('servers_save', { config }),
  serversDelete: (id: string) => invoke<Unit>('servers_delete', { id }),

  sessionOpen: (serverId: string) => invoke<string>('session_open', { serverId }),
  /** Connect, report what the peer said, hang up. Uses the real host-key sheet. */
  sessionTest: (config: ServerConfig) => invoke<ServerInfo>('session_test', { config }),
  sessionClose: (id: string) => invoke<Unit>('session_close', { id }),
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

  queueEnqueue: (
    session: string,
    serverId: string,
    direction: Direction,
    remotePath: string,
    localPath: string,
  ) => invoke<string>('queue_enqueue', { session, serverId, direction, remotePath, localPath }),
  queueControl: (op: QueueOp) => invoke<Unit>('queue_control', { op }),

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
