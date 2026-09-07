/** The Relay mark — the application's own icon, not a redrawing of it.
 *
 * This was a stroked SVG approximation: first a single right arrow, then a two-arrow
 * version closer to the real thing. Both were thin outlines where the actual logo is a
 * pair of solid block arrows, so the mark in the window never quite matched the one in
 * the Dock, and at 22px the strokes read as spindly.
 *
 * Importing `src-tauri/icons` is deliberate. That directory is where the icon a build
 * actually ships is defined, so taking it from there means the two cannot drift: change
 * the app icon and this changes with it. A copy under `src/assets` would be a second
 * source of truth that silently goes stale.
 *
 * The blue rounded square is part of the artwork, so there is no tinted container here
 * — the image is the whole mark. */

import markUrl from '../../src-tauri/icons/128x128@2x.png'

interface Props {
  size?: number
  /** Rendered flush; the artwork carries its own corner radius at this proportion. */
  className?: string
}

export function RelayMark({ size = 22, className }: Props) {
  return (
    <img
      src={markUrl}
      width={size}
      height={size}
      alt=""
      aria-hidden
      className={className}
      // The source is 256px, so every size we draw it at is a downscale.
      style={{ width: size, height: size, display: 'block' }}
      draggable={false}
    />
  )
}
