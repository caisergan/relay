/** jsdom does not implement `matchMedia`, which the theme hook calls on mount.
 * Stubbing it here keeps the mount test honest: it should fail on our bugs, not on
 * jsdom's gaps. Reports light so `system` resolves deterministically. */
window.matchMedia = (query: string) => ({
  matches: false,
  media: query,
  onchange: null,
  addEventListener: () => undefined,
  removeEventListener: () => undefined,
  dispatchEvent: () => false,
  addListener: () => undefined,
  removeListener: () => undefined,
})

/** jsdom lays nothing out and so has no `scrollIntoView`, which the command palette calls
 * to keep the highlighted command in view. Assigned rather than defaulted: the DOM types
 * say it always exists, so `??=` reads as dead code to the linter. */
Element.prototype.scrollIntoView = () => undefined

/** React 19 asks for this flag before it will let `act` flush effects; without it
 * every mount test prints "the current testing environment is not configured to
 * support act(...)" and the noise buries a real failure. */
declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean
}
globalThis.IS_REACT_ACT_ENVIRONMENT = true

export {}
