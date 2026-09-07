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
