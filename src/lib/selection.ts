/** Choosing rows in a pane: what a click does to the selection.
 *
 * Kept apart from the listing so the rules can be read and tested without rendering one.
 * Rows are named rather than numbered, because a listing is refreshed and re-sorted under
 * a selection, and a name still means the same entry afterwards where a position does not. */

/** A click on a row. A plain one makes the row the whole selection, as a click always
 * did. With the add-to-selection key held it toggles that row and leaves the rest alone —
 * files and folders alike, since everything done to a selection next (a drag, a transfer,
 * a delete) takes either. */
export function clickSelect(
  selected: readonly string[],
  name: string,
  adding: boolean,
): string[] {
  if (!adding) return [name]
  return selected.includes(name)
    ? selected.filter((item) => item !== name)
    : [...selected, name]
}

/** Whether a click adds to the selection: ⌘ on a Mac, Ctrl elsewhere.
 *
 * Not either key on both. Ctrl-click on a Mac is a right-click, so counting it would add
 * the row the inspector is being opened for; and ⌘ on Windows is the Windows key, which
 * the system takes before the page ever sees the click. */
export function addsToSelection(
  e: { metaKey: boolean; ctrlKey: boolean },
  mac: boolean,
): boolean {
  return mac ? e.metaKey : e.ctrlKey
}
