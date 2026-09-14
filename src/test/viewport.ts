/** Give the virtualiser a viewport, so rows actually mount.
 *
 * jsdom reports every element as zero by zero, and @tanstack/react-virtual computes its
 * window from the scroll element's size — so without real dimensions it decides no row
 * is visible and mounts none of them. For a test file's `beforeAll`. */
export function giveViewport(): void {
  // Assigned rather than defaulted: the DOM types say `ResizeObserver` always exists,
  // so `??=` reads as dead code to the linter, and jsdom does not actually ship one.
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  for (const [prop, value] of [
    ['clientHeight', 800],
    ['clientWidth', 600],
    ['offsetHeight', 800],
    ['offsetWidth', 600],
  ] as const) {
    Object.defineProperty(HTMLElement.prototype, prop, { configurable: true, value })
  }
}
