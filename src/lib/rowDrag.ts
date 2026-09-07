/** Dragging a row from one pane to the other, without the HTML drag-and-drop model.
 *
 * Relay's rows used to be `draggable` and the panes listened for `dragenter`,
 * `dragover` and `drop`. In a browser that works. In the app it never can: Tauri's
 * native drag handler is on, because that is how files dragged in from Finder arrive
 * with real paths, and wry implements it by overriding the WKWebView's
 * `draggingEntered:` / `draggingUpdated:` / `performDragOperation:` and returning
 * without ever calling WebKit's own. Tauri's listener answers `true` unconditionally,
 * so every drag — an internal one included — is swallowed at the AppKit layer and the
 * page sees `dragstart` (a source-side event, which WebKit still raises) and then
 * nothing at all until `dragend`. No MIME type, no `preventDefault`, no CSS order
 * would ever have changed that.
 *
 * So a drag between panes is a pointer gesture instead: `pointerdown` on a row,
 * movement past a threshold, `pointerup` over a target. Pointer events are ordinary
 * mouse input and nothing native sits between them and the page. The target is found
 * by hit test — `elementFromPoint` and the markers below — which is what the drag
 * model was doing on our behalf anyway, minus the parts that never fired. */

export type Pane = 'local' | 'remote'

/** What is in flight. */
export interface RowDrag {
  /** The pane the gesture started in. It cannot receive its own drag. */
  from: Pane
  /** The entries being carried, as named in the source directory. */
  entries: { name: string; isDir: boolean }[]
}

/** Where the pointer is, in the terms a drop cares about. */
export interface DropTarget {
  pane: Pane
  /** A folder row under the pointer, to land inside. `null` lands in the open
   * directory. */
  folder: string | null
  /** Whether the pointer is over the landing area itself. */
  zone: boolean
}

/** Set on a pane's listing while it can receive, valued with the pane's name. */
export const PANE_ATTR = 'data-drop-pane'
/** Set on the landing area, valued with its pane's name. */
export const ZONE_ATTR = 'data-drop-zone'
/** Every row carries its name and whether it is a directory, so a hit test can name
 * the folder under the pointer without asking React. */
export const ROW_NAME_ATTR = 'data-row-name'
export const ROW_DIR_ATTR = 'data-row-dir'

function asPane(value: string | null): Pane | null {
  return value === 'local' || value === 'remote' ? value : null
}

/** The drop target for the element under the pointer, or `null` when there is none.
 *
 * The landing area floats over the listing, so it is checked first: a pointer over it
 * is over it, whatever row is underneath. Then the listing, and inside it a folder
 * row. A file row is not a destination — a drop on one lands beside it, in the
 * directory on show — and neither is anything in the pane the drag came from. */
export function resolveTarget(el: Element | null, from: Pane): DropTarget | null {
  if (!el) return null

  const zone = el.closest(`[${ZONE_ATTR}]`)
  if (zone) {
    const pane = asPane(zone.getAttribute(ZONE_ATTR))
    return pane && pane !== from ? { pane, folder: null, zone: true } : null
  }

  const listing = el.closest(`[${PANE_ATTR}]`)
  if (!listing) return null
  const pane = asPane(listing.getAttribute(PANE_ATTR))
  if (!pane || pane === from) return null

  const row = el.closest(`[${ROW_DIR_ATTR}="true"]`)
  const folder = row && listing.contains(row) ? row.getAttribute(ROW_NAME_ATTR) : null
  return { pane, folder, zone: false }
}

export function sameTarget(a: DropTarget | null, b: DropTarget | null): boolean {
  if (a === null || b === null) return a === b
  return a.pane === b.pane && a.folder === b.folder && a.zone === b.zone
}
