/** Where one dragged entry comes from and where it ends up.
 *
 * Small enough to have been three lines inside the drop handler, and it was — but the
 * rule it encodes is asymmetric in a way that is easy to get backwards, and getting it
 * backwards does not throw. It silently writes to the wrong directory, or reads from
 * one, and the first anybody knows is a file that landed somewhere nobody chose.
 *
 * The rule: **only the receiving side moves.** A drop chooses a destination; it cannot
 * change where the bytes already live. Dragging `notes.txt` from the server onto the
 * local `backups/` folder reads it from the remote directory on show and writes it to
 * `<local>/backups/notes.txt`. Dragging the other way does the mirror image. So
 * `intoFolder` applies to the local side of a download and the remote side of an
 * upload, and to nothing else. */

import { joinPath } from './format'

export interface TransferPathsArgs {
  /** `down` is server to client, `up` is client to server. */
  direction: 'up' | 'down'
  /** The directory the remote pane is showing. */
  remoteDir: string
  /** The directory the local pane is showing. */
  localDir: string
  /** The entry being transferred, as it is named in the source directory. */
  name: string
  /** A folder in the destination pane that the drop was aimed at, if any. Relative to
   * that pane's current directory, because that is the only place it can have come
   * from — it is a row in the listing on screen. */
  intoFolder?: string | undefined
}

export interface TransferPaths {
  remotePath: string
  localPath: string
}

export function transferPaths({
  direction,
  remoteDir,
  localDir,
  name,
  intoFolder,
}: TransferPathsArgs): TransferPaths {
  const into = (base: string) => (intoFolder ? joinPath(base, intoFolder) : base)
  return {
    remotePath: joinPath(direction === 'up' ? into(remoteDir) : remoteDir, name),
    localPath: joinPath(direction === 'down' ? into(localDir) : localDir, name),
  }
}
