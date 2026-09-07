import { create } from 'zustand'

import { commands } from '@/ipc/commands'
import type { ServerConfig } from '@/ipc/gen'

interface ServersState {
  servers: ServerConfig[]
  loading: boolean
  /** Whether `load()` has finished once. Distinct from `!loading`, which is also true
   * before the first load has started — the launch behaviour has to tell "no servers"
   * apart from "not asked yet". */
  loaded: boolean
  load: () => Promise<void>
  save: (config: ServerConfig) => Promise<void>
  remove: (id: string) => Promise<void>
}

export const useServersStore = create<ServersState>((set) => ({
  servers: [],
  loading: false,
  loaded: false,
  load: async () => {
    set({ loading: true })
    try {
      set({ servers: await commands.serversList(), loading: false, loaded: true })
    } catch {
      // Still `loaded`: the question was asked and answered, even if badly. Leaving it
      // false would hang the launch behaviour waiting for a load that will not retry.
      set({ loading: false, loaded: true })
    }
  },
  save: async (config) => {
    const saved = await commands.serversSave(config)
    set((s) => ({
      servers: s.servers.some((x) => x.id === saved.id)
        ? s.servers.map((x) => (x.id === saved.id ? saved : x))
        : [...s.servers, saved],
    }))
  },
  remove: async (id) => {
    await commands.serversDelete(id)
    set((s) => ({ servers: s.servers.filter((x) => x.id !== id) }))
  },
}))

/** Sidebar grouping, matching the design's grouped server list. */
export function groupServers(servers: ServerConfig[]): [string, ServerConfig[]][] {
  const groups = new Map<string, ServerConfig[]>()
  for (const server of servers) {
    const key = server.group ?? 'Servers'
    groups.set(key, [...(groups.get(key) ?? []), server])
  }
  return [...groups.entries()].sort(([a], [b]) => a.localeCompare(b))
}
