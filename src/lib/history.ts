/** Per-pane navigation history, the way a browser does it.
 *
 * A pane is a place you move around in, and the breadcrumb only goes *up*. Getting
 * back to a sibling directory you were just in meant walking up and back down it by
 * name. Back and forward are the two moves the breadcrumb cannot express.
 *
 * Kept as pure functions over a value rather than as store methods, because the
 * interesting behaviour is the branch rule below and that is worth testing directly. */

/** Long enough to cover a session's browsing, bounded so a script clicking through
 * directories cannot grow it without limit. */
const LIMIT = 100

export interface History {
  entries: string[]
  /** Index of the entry currently being shown. */
  at: number
}

export const emptyHistory: History = { entries: [], at: -1 }

/** Record a navigation.
 *
 * Navigating after going back **discards** the forward entries, which is the rule that
 * makes forward mean anything: keeping them would leave a trail that no longer leads
 * anywhere from here. Re-entering the current directory is not a navigation at all —
 * a refresh must not push a duplicate that back would then step through. */
export function push(history: History, path: string): History {
  if (history.entries[history.at] === path) return history
  const kept = [...history.entries.slice(0, history.at + 1), path]
  const trimmed = kept.length > LIMIT ? kept.slice(kept.length - LIMIT) : kept
  return { entries: trimmed, at: trimmed.length - 1 }
}

export function canGoBack(history: History): boolean {
  return history.at > 0
}

export function canGoForward(history: History): boolean {
  return history.at >= 0 && history.at < history.entries.length - 1
}

/** The path `delta` steps away, or null if there is nothing there. Returning the path
 * rather than mutating keeps the caller in charge of whether the move actually
 * happened — a listing can still fail after the click. */
export function peek(history: History, delta: number): string | null {
  return history.entries[history.at + delta] ?? null
}
