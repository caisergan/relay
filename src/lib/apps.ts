/** Applications on this computer, as the preview feature needs them: what one is called,
 * and a way to pick one. */

import { open } from '@tauri-apps/plugin-dialog'

import { isMac } from '@/app/useTheme'

import { baseName } from './format'

/** What an application is called, from where it is installed: `Sublime Text` for
 * `/Applications/Sublime Text.app`. */
export function appName(path: string): string {
  return baseName(path).replace(/\.(app|exe)$/i, '')
}

/** Ask for an application with the system's own picker — on a Mac, the panel Finder's
 * "Open With › Other…" uses, opened on Applications and offering only applications.
 * Null when it was closed without a choice. */
export async function chooseApplication(): Promise<string | null> {
  const chosen = await open({
    title: 'Choose an application to open server files in',
    multiple: false,
    directory: false,
    ...(isMac()
      ? {
          defaultPath: '/Applications',
          filters: [{ name: 'Applications', extensions: ['app'] }],
        }
      : {}),
  })
  return typeof chosen === 'string' ? chosen : null
}
