/** The gaps in `@react-symbols/icons`, and what to draw in them.
 *
 * The library picks an icon from a filename by itself, over a table of some 800 names
 * and a few hundred extensions, and it is very good at the things a code editor sees:
 * it knows `webpack.config.prod.babel.cts` and every Docker Compose variant by name.
 * Relay does not browse a checkout, though — it browses a server, and a server is also
 * full of keys, tarballs, database dumps and compiled objects.
 *
 * Measured against the extensions Relay actually cares about, the library answers 61%
 * of them (132 of 217) and returns its blank page for the rest. The misses are not
 * scattered; they arrive as whole categories:
 *
 *     keys      0/13     pem key crt cer der p12 pfx pub asc gpg csr jks ppk
 *     binary    0/11     so dylib a o dll wasm class jar bin pyc rlib
 *     archive   5/12     tgz bz2 xz zst cab sit lz4
 *
 * Every icon below already exists in the pack — none of this is new artwork, and none
 * of it overrides a choice the library already makes. `editFileExtensionData` and
 * `editFileNameData` merge into the library's tables rather than replacing them, so
 * this file is purely additive: `main.rs` keeps the Rust icon it always had.
 *
 * Three categories are deliberately absent, for two different reasons.
 *
 * Office documents and presentations (`docx`, `odt`, `pages`, `pptx`, `odp`) are left
 * alone because the pack's fallback is already the right drawing: its `Document`
 * component and its default icon are the same page outline, byte for byte, so an entry
 * pointing at it would change nothing. A page is what a `.docx` should look like, and
 * it gets one without help. `fileIcon.test.tsx` refuses any entry that renders
 * identically to the default, which is the check that caught this.
 *
 * Mail and calendar formats (`eml`, `mbox`, `ics`, `vcf`) and partial or backup files
 * (`.relaypart`, `.crdownload`, `.bak`, `.swp`) are left alone because the pack has
 * nothing honest for either. Its blank page says "a file" without claiming something
 * false, which is the better of the two wrong answers. */

import {
  Audio,
  CLang,
  Clojure,
  Compressed,
  Cplus,
  Csv,
  Exe,
  Gear,
  Image,
  Lock,
  Markdown,
  PHP,
  Perl,
  Python,
  Shell,
  Text,
  Vue,
} from '@react-symbols/icons/files'
import type { ExtensionType } from '@react-symbols/icons/utils'

type Icon = ExtensionType[string]

/** One entry per icon rather than one per extension, so the collapsing stays visible:
 * thirteen key formats are one padlock, and that is the point of the grouping. */
const BY_ICON: [Icon, string][] = [
  // Secrets. The whole category was blank, and on a server it is never rare.
  [Lock, 'pem key crt cer der p7b p7c p12 pfx jks keystore pub ppk asc gpg pgp sig csr kdbx'],
  // Compiled output. `Exe` is the pack's executable mark; a loose fit for a `.dylib`,
  // but "this is a built artefact, not something you can read" is the distinction that
  // matters in a listing, and it is the one it makes.
  [Exe, 'so dylib a o obj ko dll wasm class jar war ear bin elf out pyc pyo rlib rmeta node'],
  // Installers, alongside the exe/msi/deb/rpm the library already knows.
  [Exe, 'pkg mpkg apk aab ipa appimage snap flatpak msix appx'],
  [Csv, 'numbers'],
  // Archives beyond the zip/tar/gz/rar/7z the library covers.
  [
    Compressed,
    'tgz tbz tbz2 txz tzst bz2 xz zst lz lzma lz4 br cab arj lha lzh sit sitx xip cpio',
  ],
  [Image, 'icns ai eps psb xcf tga jfif jpe pbm ppm pgm'],
  [Audio, 'aac opus mid midi aif aifc alac ape amr ac3'],
  // Prose. `md` is already the pack's; these are its relatives.
  [Text, 'text log rst adoc asciidoc nfo'],
  [Markdown, 'markdown'],
  // Configuration that is not YAML, TOML or JSON. A cog reads as "settings" at row
  // size, which is what these are.
  [Gear, 'ini cfg conf properties'],
  // Languages the pack draws but never wired to an extension.
  [PHP, 'php phtml'],
  [Vue, 'vue'],
  [Perl, 'pl pm'],
  [Clojure, 'clj cljs cljc edn'],
  [Python, 'pyi pyw pyx'],
  [Cplus, 'hpp hh hxx'],
  [CLang, 'm mm'],
]

function spread(pairs: [Icon, string][]): ExtensionType {
  return Object.fromEntries(
    pairs.flatMap(([icon, keys]) =>
      keys
        .split(/\s+/)
        .filter(Boolean)
        .map((key) => [key, icon] as const),
    ),
  )
}

/** Extensions the library does not place. Merged into its own table, never over it. */
export const EXTENSIONS: ExtensionType = spread(BY_ICON)

/** Whole filenames the library does not place.
 *
 * Short, because its own name table is enormous and already covers `Dockerfile`,
 * `LICENSE`, `.gitignore`, `.env` and several hundred others. These are the ones a
 * server has that a code editor does not. Matching by name at all requires `autoAssign`
 * on the component, so the two travel together. */
export const NAMES: ExtensionType = spread([
  [Shell, '.zshrc .bashrc .bash_profile .zprofile .profile .kshrc .cshrc'],
  [Gear, 'makefile gnumakefile justfile procfile crontab fstab hosts resolv.conf'],
  [Gear, 'sshd_config ssh_config nginx.conf httpd.conf my.cnf php.ini .htaccess'],
  [Text, 'readme changelog todo install version authors contributors news'],
  [Lock, 'id_rsa id_ed25519 id_ecdsa id_dsa authorized_keys known_hosts .netrc .htpasswd'],
])
