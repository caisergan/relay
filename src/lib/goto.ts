/** Typing a path into a pane's search box, and going there.
 *
 * A file name cannot contain a `/`, so a filter that started with one matched nothing
 * and emptied the pane — when whoever typed it plainly meant a place, not a name. What
 * counts as a place, and how it is spelled once it is one, lives here as pure functions
 * so the rules can be tested without a webview. */

const DRIVE = /^[a-z]:[\\/]/i

/** Whether the search box holds a place rather than a name to filter by.
 *
 * `~` alone or followed by a slash, never `~foo`: a leading tilde is a real name prefix
 * (Office's `~$report.docx` lock files), and `~user` is nothing an SFTP server expands.
 * Drive letters only on the local side; a server's paths are POSIX. */
export function isPathQuery(text: string, side: 'local' | 'remote'): boolean {
  const t = text.trim()
  if (t.startsWith('/') || t === '~' || t.startsWith('~/')) return true
  return side === 'local' && DRIVE.test(t)
}

/** The absolute, normalised path a query names, with `~` standing for `home`.
 *
 * `.` and `..` are resolved here rather than sent as typed: the breadcrumb and the
 * history are both built from the path string, so `/srv/app/../logs` would draw a `..`
 * crumb, and back would treat it as somewhere other than `/srv/logs`. */
export function resolvePath(text: string, home: string): string {
  let t = text.trim()
  if (t === '~' || t.startsWith('~/')) t = home + t.slice(1)

  // Windows takes either separator and prints backslashes. A server takes only `/`, and
  // a backslash there is an ordinary character in a name.
  const drive = DRIVE.test(t)
  const root = drive ? `${t.slice(0, 2)}\\` : '/'
  const parts: string[] = []
  for (const part of t.slice(root.length).split(drive ? /[\\/]/ : '/')) {
    if (part === '' || part === '.') continue
    if (part === '..') parts.pop()
    else parts.push(part)
  }
  return root + parts.join(drive ? '\\' : '/')
}

/** The folder a resolved path is in, and its name there. A path to a file opens that
 * folder with the file selected. */
export function splitPath(path: string): { parent: string; name: string } {
  const index = DRIVE.test(path) ? path.lastIndexOf('\\') : path.lastIndexOf('/')
  const parent = path.slice(0, index)
  return {
    parent: parent === '' ? '/' : /^[a-z]:$/i.test(parent) ? `${parent}\\` : parent,
    name: path.slice(index + 1),
  }
}
