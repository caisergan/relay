/** That a filename reaches an icon, and that the supplement is doing its job.
 *
 * The failure being guarded against is silent. `FileIcon` renders its blank page for
 * anything it cannot place and throws nothing, so a supplement entry pointing at a
 * component the pack no longer exports, or a name the pack quietly stopped matching,
 * would show up as an empty square in the pane and a green test run everywhere else.
 *
 * Every assertion therefore compares against the blank page rather than against a
 * particular drawing: pinning icons by their path data would fail on any upstream
 * redraw, which is a change we do not care about, while saying nothing about the one
 * we do. */

import {
  DefaultFileIcon,
  DefaultFolderIcon,
  FileIcon,
  getIconForFolder,
} from '@react-symbols/icons/utils'
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'

import { EXTENSIONS, FOLDERS, NAMES } from './fileIcon'

const SIZE = 20

/** What the row actually renders, as markup. */
function draw(fileName: string): string {
  return renderToStaticMarkup(
    <FileIcon
      fileName={fileName}
      autoAssign
      editFileExtensionData={EXTENSIONS}
      editFileNameData={NAMES}
      width={SIZE}
      height={SIZE}
    />,
  )
}

const BLANK = renderToStaticMarkup(<DefaultFileIcon width={SIZE} height={SIZE} />)

/** Did this name find an icon of its own, or fall through to the blank page? */
const placed = (fileName: string) => draw(fileName) !== BLANK

/** Names the supplement is responsible for — the categories the pack answered with a
 * blank page before this file existed. If any of these regress, the gap is back. */
const CLOSED: Record<string, string[]> = {
  keys: ['server.pem', 'tls.crt', 'store.p12', 'id_ed25519', 'authorized_keys', 'known_hosts'],
  binaries: ['libssl.dylib', 'libc.so', 'mod.wasm', 'App.class', 'core.o', 'lib.rlib'],
  installers: ['Relay.pkg', 'app.apk', 'game.ipa', 'tool.appimage'],
  archives: ['backup.tgz', 'dump.bz2', 'src.xz', 'snap.zst', 'old.sit'],
  prose: ['CHANGELOG', 'README', 'notes.log', 'guide.rst'],
  config: ['php.ini', 'app.cfg', 'nginx.conf', 'Makefile', 'crontab'],
  shells: ['.zshrc', '.bashrc', '.profile'],
  languages: ['index.php', 'App.vue', 'script.pl', 'core.clj', 'types.pyi', 'util.hpp'],
}

describe('the supplement', () => {
  for (const [group, names] of Object.entries(CLOSED)) {
    it(`places ${group}`, () => {
      for (const name of names) {
        expect(placed(name), `"${name}" fell through to the blank page`).toBe(true)
      }
    })
  }

  /** Every component referenced by the supplement has to still exist upstream. A
   * removed export arrives as `undefined`, which React renders as nothing at all. */
  it('points only at components the pack still exports', () => {
    for (const [key, icon] of [...Object.entries(EXTENSIONS), ...Object.entries(NAMES)]) {
      expect(typeof icon, `"${key}" maps to something that is not a component`).toBe('function')
    }
  })

  /** An entry pointing at a component that draws the default is worse than no entry:
   * it reads as a decision and changes nothing. This is not hypothetical — 23 office
   * and presentation extensions were first mapped to the pack's `Document`, which is
   * the default icon byte for byte, and this is what found them. */
  it('has no entry that draws the same thing as the fallback', () => {
    for (const [key, Icon] of [...Object.entries(EXTENSIONS), ...Object.entries(NAMES)]) {
      const drawn = renderToStaticMarkup(<Icon width={SIZE} height={SIZE} />)
      expect(drawn, `"${key}" maps to a component identical to the default`).not.toBe(BLANK)
    }
  })

  /** Purely additive: the pack's own answers must survive the merge. If a supplement
   * entry ever collided with one of them, this is where it would show. */
  it('does not disturb what the pack already knew', () => {
    for (const name of [
      'main.rs',
      'app.ts',
      'server.py',
      'go.mod',
      'package.json',
      'Dockerfile',
    ]) {
      expect(placed(name), `"${name}" lost its icon`).toBe(true)
    }
  })
})

describe('the pane', () => {
  it('draws at the size the rows ask for', () => {
    expect(draw('main.rs')).toContain(`width="${SIZE}"`)
    expect(draw('main.rs')).toContain(`height="${SIZE}"`)
  })

  it('tells different file types apart', () => {
    const kinds = [
      'main.rs',
      'server.py',
      'report.pdf',
      'photo.png',
      'archive.zip',
      'server.pem',
    ]
    expect(new Set(kinds.map(draw)).size).toBe(kinds.length)
  })

  /** A name nobody can place still has to render something rather than a hole. */
  it('draws a blank page for a name nobody can place', () => {
    expect(draw('mystery.qqq')).toBe(BLANK)
    expect(BLANK).toContain('<svg')
  })

  /** Left to the fallback on purpose, and the fallback is a page outline — which is
   * the correct drawing for a document, so this is a decision rather than a gap. */
  it('lets office documents fall through to the page outline', () => {
    for (const name of ['thesis.docx', 'contract.odt', 'deck.pptx', 'notes.pages']) {
      expect(draw(name), `"${name}" should be the page outline`).toBe(BLANK)
    }
  })

  it('survives names that are barely names at all', () => {
    for (const odd of ['', '.', '..', 'file.', '.hidden']) {
      expect(() => draw(odd), `"${odd}" threw`).not.toThrow()
    }
  })
})

/** Did the pack place this folder, or hand back the default it uses for anything it
 * does not recognise? Identity, not markup — this is exactly the check `FileList`
 * makes to decide whether to substitute Relay's own blue mark. */
const look = (folderName: string) =>
  getIconForFolder({ folderName, editFolderNameData: FOLDERS, width: SIZE, height: SIZE })

/** Mirrors `FolderRowIcon`: the given name, then its lowercase form. */
const knows = (name: string) =>
  look(name).type !== DefaultFolderIcon || look(name.toLowerCase()).type !== DefaultFolderIcon

describe('folders', () => {
  it('places the server directories the supplement adds', () => {
    for (const name of [
      'secrets',
      'certs',
      '.ssh',
      'migrations',
      'etc',
      'logs',
      'backups',
      'vendor',
      '__pycache__',
      'templates',
      'media',
    ]) {
      expect(knows(name), `"${name}" was not placed`).toBe(true)
    }
  })

  it('does not disturb the folders the pack already knew', () => {
    for (const name of [
      'src',
      'dist',
      'node_modules',
      'docs',
      'services',
      'shared',
      'config',
    ]) {
      expect(knows(name), `"${name}" lost its icon`).toBe(true)
    }
  })

  /** The fallback has to stay reachable. If everything matched, `FileList` would never
   * draw Relay's blue mark and the substitution would be dead code. */
  it('leaves an unrecognised name to the fallback', () => {
    for (const name of ['ui-design', 'ml', 'wibble', '2026-07-17-standard']) {
      expect(knows(name), `"${name}" should have fallen back`).toBe(false)
    }
  })

  it('has no folder entry that draws the same thing as the fallback', () => {
    const blank = renderToStaticMarkup(<DefaultFolderIcon width={SIZE} height={SIZE} />)
    for (const [key, Icon] of Object.entries(FOLDERS)) {
      const drawn = renderToStaticMarkup(<Icon width={SIZE} height={SIZE} />)
      expect(drawn, `"${key}" maps to a component identical to the default`).not.toBe(blank)
    }
  })

  it('points only at components the pack still exports', () => {
    for (const [key, icon] of Object.entries(FOLDERS)) {
      expect(typeof icon, `"${key}" maps to something that is not a component`).toBe('function')
    }
  })

  /** The library matches folder names case-sensitively — unlike file names, which it
   * lowercases — so `Documents` and `Logs` find nothing on their own. `Downloads` and
   * `Documents` are ordinary in a Mac home folder, and `Logs` and `Backups` are
   * ordinary on a server, so the call site asks a second time in lowercase. */
  it('recognises a capitalised directory', () => {
    for (const name of ['Docs', 'Src', 'Secrets', 'Logs', 'Documents', 'DIST']) {
      expect(knows(name), `"${name}" was not placed`).toBe(true)
    }
  })

  it('survives names that are barely names at all', () => {
    for (const odd of ['', '.', '..', '.hidden']) {
      expect(() => knows(odd), `"${odd}" threw`).not.toThrow()
    }
  })
})
