import { useEffect } from 'react'

import { useUiStore } from '@/state/uiStore'

/** Applies theme and density to the document root.
 *
 * `system` follows the OS and keeps following it, so a person who changes their
 * appearance setting while Relay is open sees it happen. */
export function useTheme(): void {
  const theme = useUiStore((s) => s.theme)
  const density = useUiStore((s) => s.density)
  const setResolved = useUiStore((s) => s.setResolved)

  useEffect(() => {
    const root = document.documentElement
    const media = window.matchMedia('(prefers-color-scheme: dark)')

    const apply = () => {
      const resolved = theme === 'system' ? (media.matches ? 'dark' : 'light') : theme
      root.dataset.theme = resolved
      // Mirrored into the store because components need to know which theme is
      // actually showing — `system` is not an answer the theme toggle can act on.
      setResolved(resolved)
    }

    apply()
    if (theme !== 'system') return
    media.addEventListener('change', apply)
    return () => media.removeEventListener('change', apply)
  }, [theme, setResolved])

  useEffect(() => {
    document.documentElement.dataset.density = density
  }, [density])
}

/** macOS keeps its traffic lights; Windows gets drawn controls. Phase 0 §0.4 wants
 * this exercised on both webviews, so the branch is explicit rather than implied. */
export function isMac(): boolean {
  return /mac/i.test(navigator.userAgent)
}
