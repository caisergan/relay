/** The square that stands for a server, in the sidebar, the rail, the tabs and the
 * session header.
 *
 * One component because the four had drifted: the tab hashed the name for its colour
 * while the sidebar used the one chosen in the editor, and all four rendered
 * `name.slice(0, 2)` — which turns `104.197.160.37` into "10". */

import { avatarMark, tintFor } from '@/lib/avatar'

import { IconServer } from './Icons'

interface Props {
  name: string
  /** The colour chosen in the editor. Falls back to the deterministic tint. */
  color?: string | null
  size?: number
  /** Draws the connected dot in the corner. */
  live?: boolean
  className?: string
}

export function ServerAvatar({ name, color, size = 26, live = false, className }: Props) {
  const mark = avatarMark(name)
  return (
    <span
      className={`avatar${className ? ` ${className}` : ''}`}
      style={{
        background: color ?? tintFor(name),
        width: size,
        height: size,
        // The monogram scales with the square; the glyph is sized by its own prop.
        fontSize: Math.round(size * 0.42),
        borderRadius: Math.max(5, Math.round(size * 0.27)),
      }}
      aria-hidden
    >
      {mark.kind === 'initials' ? mark.text : <IconServer size={Math.round(size * 0.58)} />}
      {live && <span className="avatar__live" />}
    </span>
  )
}
