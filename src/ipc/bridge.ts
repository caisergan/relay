/** The client half of the engine's recovery protocol.
 *
 * Subscribe, buffer, snapshot, then replay only what the snapshot did not already
 * contain. Anything that could have cost us an update — a sequence gap, a changed
 * epoch, a subscription the engine has dropped — resnapshots rather than guesses.
 *
 * This is the only module that talks to the engine stream. Components subscribe to
 * stores; they never see an envelope. */

import type { EngineEnvelope, EngineEvent, EngineSnapshot } from './gen'
import { Channel, commands } from './commands'

/** How often to confirm the engine still knows about our subscription. The Rust side
 * drops a subscriber that falls behind, and a dropped channel is otherwise silent. */
const LIVENESS_INTERVAL_MS = 5000
/** Backoff after a failed reconnect, so a broken engine does not spin the UI. */
const RECONNECT_DELAY_MS = 1000

export interface BridgeHandlers {
  onSnapshot: (snapshot: EngineSnapshot) => void
  onEvent: (event: EngineEvent) => void
  /** Told when the stream drops, so the shell can show a reconnecting state. */
  onConnectionChange?: (connected: boolean) => void
}

type Phase = 'idle' | 'buffering' | 'live'

export class EngineBridge {
  private handlers: BridgeHandlers
  private phase: Phase = 'idle'
  private buffer: EngineEnvelope[] = []
  private subscriptionId: string | null = null
  private epoch: string | null = null
  private watermark = 0
  private liveness: ReturnType<typeof setInterval> | null = null
  private stopped = false
  private generation = 0

  constructor(handlers: BridgeHandlers) {
    this.handlers = handlers
  }

  async start(): Promise<void> {
    this.stopped = false
    await this.connect()
  }

  async stop(): Promise<void> {
    this.stopped = true
    this.clearLiveness()
    const id = this.subscriptionId
    this.subscriptionId = null
    this.phase = 'idle'
    if (id) await commands.engineUnsubscribe(id).catch(() => undefined)
  }

  private async connect(): Promise<void> {
    if (this.stopped) return
    const generation = ++this.generation
    this.phase = 'buffering'
    this.buffer = []

    const channel = new Channel<EngineEnvelope>()
    channel.onmessage = (envelope) => {
      // A late message from a superseded subscription is not ours to apply.
      if (generation !== this.generation) return
      this.receive(envelope)
    }

    try {
      // Order matters: subscribe first so nothing published between the subscribe and
      // the snapshot can fall down the gap between them.
      const subscriptionId = await commands.engineSubscribe(channel)
      if (generation !== this.generation) {
        await commands.engineUnsubscribe(subscriptionId).catch(() => undefined)
        return
      }
      this.subscriptionId = subscriptionId

      const snapshot = await commands.engineSnapshot(subscriptionId)
      if (generation !== this.generation) return
      this.applySnapshot(snapshot)
      this.startLiveness()
      this.handlers.onConnectionChange?.(true)
    } catch {
      this.handlers.onConnectionChange?.(false)
      this.scheduleReconnect()
    }
  }

  private applySnapshot(snapshot: EngineSnapshot): void {
    this.epoch = snapshot.epoch
    this.watermark = snapshot.seq
    this.handlers.onSnapshot(snapshot)

    // Replay only what the snapshot could not already have included.
    const buffered = this.buffer
      .filter((e) => e.epoch === snapshot.epoch && e.seq > snapshot.seq)
      .sort((a, b) => a.seq - b.seq)
    this.buffer = []
    for (const envelope of buffered) this.deliver(envelope)
    this.phase = 'live'
  }

  private receive(envelope: EngineEnvelope): void {
    if (this.phase === 'buffering') {
      this.buffer.push(envelope)
      return
    }
    if (this.phase !== 'live') return

    // A new epoch means the engine restarted: every watermark we hold is meaningless.
    if (this.epoch !== null && envelope.epoch !== this.epoch) {
      void this.resnapshot()
      return
    }
    // A gap means an update was lost. Recovering is cheap; guessing is not.
    if (envelope.seq !== this.watermark + 1) {
      void this.resnapshot()
      return
    }
    this.deliver(envelope)
  }

  private deliver(envelope: EngineEnvelope): void {
    this.watermark = envelope.seq
    this.handlers.onEvent(envelope.update)
  }

  private startLiveness(): void {
    this.clearLiveness()
    this.liveness = setInterval(() => {
      const id = this.subscriptionId
      if (!id || this.phase !== 'live') return
      // `engine_snapshot` rejects a subscription the engine has dropped, which is how
      // an overflowed or closed stream becomes visible to us at all.
      commands.engineSnapshot(id).catch(() => {
        this.handlers.onConnectionChange?.(false)
        void this.connect()
      })
    }, LIVENESS_INTERVAL_MS)
  }

  private clearLiveness(): void {
    if (this.liveness !== null) {
      clearInterval(this.liveness)
      this.liveness = null
    }
  }

  private async resnapshot(): Promise<void> {
    this.clearLiveness()
    const id = this.subscriptionId
    if (id) await commands.engineUnsubscribe(id).catch(() => undefined)
    this.subscriptionId = null
    await this.connect()
  }

  private scheduleReconnect(): void {
    if (this.stopped) return
    setTimeout(() => void this.connect(), RECONNECT_DELAY_MS)
  }
}
