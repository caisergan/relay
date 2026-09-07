/** The design's icons, transcribed from `claude-design-relay/Relay.dc.html`.
 *
 * Every one of these is a 24-unit stroked outline drawn at whatever `size` the call
 * site needs, so they inherit weight and colour from context instead of shipping a
 * font or a sprite sheet. The paths are the design's; the wrapper is ours.
 *
 * Glyphs (`▸`, `✎`, `␡`) were the placeholder — they render differently on every
 * platform, they cannot take a colour that means anything, and at 13px they read as
 * punctuation rather than as an affordance. */

interface IconProps {
  size?: number
  className?: string
  /** Overrides `currentColor`. Only the file-type tints use it. */
  stroke?: string
  strokeWidth?: number
}

function Svg({
  size = 14,
  className,
  stroke = 'currentColor',
  strokeWidth = 2,
  children,
}: IconProps & { children: React.ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke={stroke}
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden
      focusable="false"
    >
      {children}
    </svg>
  )
}

export function IconSearch(props: IconProps) {
  return (
    <Svg {...props}>
      <circle cx="11" cy="11" r="7" />
      <path d="M21 21l-4.3-4.3" />
    </Svg>
  )
}

export function IconFolder(props: IconProps) {
  return (
    <Svg strokeWidth={1.8} {...props}>
      <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
    </Svg>
  )
}

export function IconFile(props: IconProps) {
  return (
    <Svg strokeWidth={1.7} {...props}>
      <path d="M6 2h8l4 4v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2z" />
      <path d="M14 2v4h4" />
    </Svg>
  )
}

export function IconMonitor(props: IconProps) {
  return (
    <Svg {...props}>
      <rect x="2" y="4" width="20" height="14" rx="2" />
      <path d="M8 21h8M12 18v3" />
    </Svg>
  )
}

export function IconServer(props: IconProps) {
  return (
    <Svg {...props}>
      <rect x="2" y="3" width="20" height="8" rx="2" />
      <rect x="2" y="13" width="20" height="8" rx="2" />
      <path d="M6 7h.01M6 17h.01" />
    </Svg>
  )
}

export function IconRefresh(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M21 12a9 9 0 1 1-2.6-6.3M21 4v5h-5" />
    </Svg>
  )
}

export function IconChevronDown(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M6 9l6 6 6-6" />
    </Svg>
  )
}

export function IconChevronRight(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M9 6l6 6-6 6" />
    </Svg>
  )
}

export function IconChevronLeft(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M15 18l-6-6 6-6" />
    </Svg>
  )
}

export function IconSun(props: IconProps) {
  return (
    <Svg {...props}>
      <circle cx="12" cy="12" r="4" />
      <path d="M12 3v2M12 19v2M5 12H3M21 12h-2M6.3 6.3 4.9 4.9M19.1 19.1l-1.4-1.4M17.7 6.3l1.4-1.4M4.9 19.1l1.4-1.4" />
    </Svg>
  )
}

export function IconMoon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M21 12.8A9 9 0 1 1 11.2 3 7 7 0 0 0 21 12.8z" />
    </Svg>
  )
}

export function IconActivity(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M4 4h16v12H5.2L4 17.5z" />
    </Svg>
  )
}

export function IconArrowRight(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M5 12h14M13 6l6 6-6 6" />
    </Svg>
  )
}

export function IconArrowLeft(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M19 12H5M11 18l-6-6 6-6" />
    </Svg>
  )
}

export function IconArrowUp(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M12 19V5M6 11l6-6 6 6" />
    </Svg>
  )
}

export function IconArrowDown(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M12 5v14M6 13l6 6 6-6" />
    </Svg>
  )
}

export function IconPlus(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M12 5v14M5 12h14" />
    </Svg>
  )
}

/** New folder. The plus is centred in the folder's *body* — below the tab, not in the
 * middle of the whole 24-unit box — and drawn at the same 1.8 weight as `IconFolder`,
 * which it otherwise reads as a heavier, differently-shaped folder beside. */
export function IconFolderPlus(props: IconProps) {
  return (
    <Svg strokeWidth={1.8} {...props}>
      <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
      <path d="M12 11.5v5M9.5 14h5" />
    </Svg>
  )
}

export function IconPencil(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M12 20h9" />
      <path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z" />
    </Svg>
  )
}

export function IconTrash(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6" />
    </Svg>
  )
}

export function IconClose(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M18 6 6 18M6 6l12 12" />
    </Svg>
  )
}

export function IconWarning(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M12 9v4M12 17h.01M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z" />
    </Svg>
  )
}

/** Dotfiles are listed.
 *
 * An eye was the first attempt and it says the wrong thing: it means visibility in
 * general, which is every row in the pane. What separates these files from the rest is
 * the leading dot, so the dot is the icon — the design's own file glyph carrying the
 * mark that makes it hidden in the first place. */
export function IconDotfile(props: IconProps) {
  return (
    <Svg strokeWidth={1.7} {...props}>
      <path d="M6 2h8l4 4v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2z" />
      <path d="M14 2v4h4" />
      <circle cx="8.5" cy="16" r="1.8" fill="currentColor" stroke="none" />
      <path d="M12 16h4.5" />
    </Svg>
  )
}

/** Dotfiles are being held back: the same file, struck through. */
export function IconDotfileOff(props: IconProps) {
  return (
    <Svg strokeWidth={1.7} {...props}>
      <path d="M6 2h8l4 4v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2z" />
      <path d="M14 2v4h4" />
      <circle cx="8.5" cy="16" r="1.8" fill="currentColor" stroke="none" />
      <path d="M12 16h4.5" />
      <path d="M3 3l18 18" />
    </Svg>
  )
}

/** Expands the collapsed sidebar, from the design's floating rail button. */
export function IconMenu(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M4 6h16M4 12h16M4 18h16" />
    </Svg>
  )
}

/** The permission-denied pane's glyph. */
export function IconLock(props: IconProps) {
  return (
    <Svg strokeWidth={1.8} {...props}>
      <path d="M12 2a5 5 0 0 0-5 5v3H6a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-7a2 2 0 0 0-2-2h-1V7a5 5 0 0 0-5-5zm-3 8V7a3 3 0 0 1 6 0v3z" />
    </Svg>
  )
}

/** The connection-lost pane's glyph: reception arcs over a dot. */
export function IconSignal(props: IconProps) {
  return (
    <Svg strokeWidth={1.8} {...props}>
      <path d="M12 20h.01M8.5 16.4a5 5 0 0 1 7 0M5 12.9a10 10 0 0 1 14 0M2 9.5a15 15 0 0 1 20 0" />
    </Svg>
  )
}

/** The not-found pane's glyph. The design has no not-found state, so this is the
 * design's own folder path under a slash rather than a new shape invented for it. */
export function IconFolderMissing(props: IconProps) {
  return (
    <Svg strokeWidth={1.8} {...props}>
      <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
      <path d="M4 4l16 16" />
    </Svg>
  )
}

/** A volume in the breadcrumb's root menu. */
export function IconDrive(props: IconProps) {
  return (
    <Svg strokeWidth={1.8} {...props}>
      <rect x="2" y="7" width="20" height="10" rx="2" />
      <path d="M6 12h.01M10 12h6" />
    </Svg>
  )
}

/** Home, the root menu's first entry. */
export function IconHome(props: IconProps) {
  return (
    <Svg strokeWidth={1.8} {...props}>
      <path d="M3 11l9-8 9 8" />
      <path d="M6 10v10h12V10" />
    </Svg>
  )
}

export function IconSidebar(props: IconProps) {
  return (
    <Svg {...props}>
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <path d="M9 4v16" />
    </Svg>
  )
}

export function IconGear(props: IconProps) {
  return (
    <Svg {...props}>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </Svg>
  )
}

/** The Relay mark: bytes moving right. Also the sidebar's wordmark glyph. */
export function IconRelay(props: IconProps) {
  return (
    <Svg strokeWidth={2.4} {...props}>
      <path d="M4 12h16M14 6l6 6-6 6" />
    </Svg>
  )
}

/** File-type tint, from the design's `fileIcon` extension map.
 *
 * The design hard-codes hexes here; these are tokens instead, so the tints stay
 * legible in the dark theme rather than staying pinned to light-theme values. */
const TYPE_TINTS: Record<string, string> = {
  md: 'var(--type-code)',
  ts: 'var(--type-code)',
  tsx: 'var(--type-code)',
  css: 'var(--type-code)',
  html: 'var(--type-code)',
  rs: 'var(--type-code)',
  json: 'var(--type-data)',
  js: 'var(--type-data)',
  jsx: 'var(--type-data)',
  yml: 'var(--type-data)',
  yaml: 'var(--type-data)',
  toml: 'var(--type-data)',
  php: 'var(--type-php)',
}

export function tintForFile(name: string): string {
  const dot = name.lastIndexOf('.')
  const ext = dot > 0 ? name.slice(dot + 1).toLowerCase() : ''
  return TYPE_TINTS[ext] ?? 'var(--type-plain)'
}
