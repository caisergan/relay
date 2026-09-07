/** The designed pane states, transcribed from `paneMsg` in `Relay.dc.html`.
 *
 * A pane that cannot show a listing still has to say *why*, and the four reasons are
 * not interchangeable: a locked directory, a path that is gone, an empty folder and a
 * dropped connection each want a different colour, sentence and next action. What this
 * replaces printed the raw error string into a grey block, which said "something went
 * wrong" four times over.
 *
 * `denied`, `empty` and `lost` are the design's, values and all. `notFound` is not in
 * the design — phase 1 §1.5 requires it ("errors map to the designed permission-denied
 * / not-found pane states"), so it is built from the same parts: the amber the design
 * already uses for a recoverable state, and the design's own folder glyph. */

import type { Fault } from '@/lib/errors'

import { IconFolder, IconFolderMissing, IconLock, IconSignal, IconWarning } from './Icons'

export type PaneMessageKind =
  'denied' | 'notFound' | 'empty' | 'lost' | 'connecting' | 'disconnected' | 'fault'

interface Spec {
  tone: 'danger' | 'signal' | 'transit'
  Icon: typeof IconLock
  title: string
  /** `path` is whatever the fault named, already trimmed for display. */
  body: (path: string | null, side: 'local' | 'remote') => string
  /** Absent where there is nowhere to go back to — a dropped connection returns on
   * its own, and offering a button implies the user could hurry it. */
  button: string | null
}

const SPECS: Record<PaneMessageKind, Spec> = {
  denied: {
    tone: 'danger',
    Icon: IconLock,
    title: 'Permission denied',
    body: (path, side) =>
      side === 'remote'
        ? `Your account can't read ${path ?? 'this folder'} on this server. Ask the server admin for access, or connect with a different user.`
        : `macOS won't let Relay read ${path ?? 'this folder'}. Grant it access in System Settings › Privacy & Security › Files and Folders.`,
    button: 'Go back',
  },
  notFound: {
    tone: 'transit',
    Icon: IconFolderMissing,
    title: 'Folder not found',
    body: (path) =>
      `${path ?? 'This folder'} is no longer there. It may have been moved or deleted since the last listing.`,
    button: 'Go back',
  },
  empty: {
    tone: 'signal',
    Icon: IconFolder,
    title: 'This folder is empty',
    body: (_path, side) =>
      side === 'remote'
        ? 'Drop files here to upload them, or drag from the local pane.'
        : 'Drop files here, or drag from the server pane to download.',
    button: 'Go back',
  },
  lost: {
    tone: 'transit',
    Icon: IconSignal,
    title: 'Waiting for the server',
    body: () => 'The connection dropped. Listings will refresh automatically once reconnected.',
    button: null,
  },
  /** Distinct from `lost`, which promises an automatic refresh. Nothing is retrying
   * during a first connect, so the pane must not say anything is. */
  connecting: {
    tone: 'transit',
    Icon: IconSignal,
    title: 'Connecting…',
    body: () => 'Opening the session and reading the first directory.',
    button: null,
  },
  /** The session ended and is not coming back on its own. The engine's reason is more
   * use than any sentence written here, so the caller supplies it. */
  disconnected: {
    tone: 'danger',
    Icon: IconSignal,
    title: 'Not connected',
    body: () => 'The session is closed.',
    button: null,
  },
  /** For a failure the taxonomy does not give a pane of its own. The title says only
   * what is certain — the listing did not happen — and the engine's own sentence
   * supplies the rest, rather than borrowing a more specific state's wording. */
  fault: {
    tone: 'danger',
    Icon: IconWarning,
    title: 'Could not open this folder',
    body: () => 'The server did not say why.',
    button: 'Go back',
  },
}

/** Which designed state a fault draws. Anything outside the taxonomy the panes act on
 * falls through to `null`, and the caller shows the fault's own sentence instead —
 * inventing a pane for an unmapped kind would claim a diagnosis we do not have. */
export function kindForFault(fault: Fault): PaneMessageKind | null {
  if (fault.kind === 'permissionDenied') return 'denied'
  if (fault.kind === 'notFound') return 'notFound'
  return null
}

interface Props {
  kind: PaneMessageKind
  side: 'local' | 'remote'
  /** Shown in the body where the spec names a path. */
  path?: string | null
  /** Overrides the spec's sentence. Used for an unmapped fault, whose own message is
   * more specific than anything this component could say about it. */
  body?: string
  /** Absent when there is no parent to return to, which also hides the button. */
  onBack?: () => void
}

export function PaneMessage({ kind, side, path = null, body, onBack }: Props) {
  const spec = SPECS[kind]
  const { Icon } = spec

  return (
    <div className="panemsg" role={kind === 'lost' ? 'status' : 'alert'}>
      <div className={`panemsg__icon panemsg__icon--${spec.tone}`}>
        <Icon size={26} />
      </div>
      <div className="panemsg__title">{spec.title}</div>
      <div className="panemsg__body">{body ?? spec.body(path, side)}</div>
      {spec.button && onBack && (
        <button className="panemsg__btn" onClick={onBack}>
          {spec.button}
        </button>
      )}
    </div>
  )
}

/** A fault with no designed pane: the same furniture, the engine's own sentence.
 *
 * It keeps the failure inside the pane that failed rather than firing a toast that
 * outlives the pane's state, which is how a permission error used to end up describing
 * a directory the user had already navigated away from. */
export function PaneFault({
  fault,
  side,
  onBack,
}: {
  fault: Fault
  side: 'local' | 'remote'
  onBack?: () => void
}) {
  const kind = kindForFault(fault)
  return (
    <PaneMessage
      kind={kind ?? 'fault'}
      side={side}
      path={fault.path}
      {...(kind ? {} : { body: fault.message })}
      {...(onBack ? { onBack } : {})}
    />
  )
}
