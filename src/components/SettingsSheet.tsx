import { open as openFolderDialog } from '@tauri-apps/plugin-dialog'
import { useEffect, useState } from 'react'

import { commands } from '@/ipc/commands'
import type { ConflictAction, Density, Settings, Theme } from '@/ipc/gen'
import { faultText } from '@/lib/errors'
import { useSessionsStore } from '@/state/sessionsStore'
import { useUiStore } from '@/state/uiStore'

import { IconClose } from './Icons'

/** The 1.0 settings, and only those.
 *
 * Rust holds them and Rust clamps them, so this sheet sends a whole `Settings` and
 * renders whatever comes back rather than what it just sent. A slider that showed 40
 * because someone typed 40, while the engine ran three connections, would be a control
 * that lies about the thing it controls.
 *
 * Theme and density are mirrored into the UI store as well, because the document root
 * has to change immediately — waiting for a round trip to repaint the whole window
 * would make every toggle feel broken. */
export function SettingsSheet() {
  const open = useUiStore((s) => s.settingsOpen)
  const toggle = useUiStore((s) => s.toggleSettings)
  const toast = useUiStore((s) => s.toast)
  const setTheme = useUiStore((s) => s.setTheme)
  const setDensity = useUiStore((s) => s.setDensity)
  const setShowHiddenDefault = useSessionsStore((s) => s.setShowHiddenDefault)
  const [settings, setSettings] = useState<Settings | null>(null)

  useEffect(() => {
    if (!open) return
    commands
      .settingsGet()
      .then(setSettings)
      .catch((error: unknown) => toast('error', faultText(error)))
  }, [open, toast])

  if (!open) return null

  const save = (next: Settings) => {
    // Optimistic only for the two the document root reads. Everything else waits for
    // the value the engine actually stored.
    setSettings(next)
    setTheme(next.theme)
    setDensity(next.density)
    commands
      .settingsSet(next)
      .then((stored) => {
        setSettings(stored)
        setTheme(stored.theme)
        setDensity(stored.density)
        // The default for panes opened from here on. A pane already on screen keeps
        // its own toggle: that is the person's more recent word about that pane.
        setShowHiddenDefault(stored.showHidden)
      })
      .catch((error: unknown) => toast('error', faultText(error)))
  }

  return (
    <div className="scrim" role="dialog" aria-modal="true" aria-label="Settings">
      <div className="sheet sheet--wide">
        <div className="sheet__head">
          <h2 className="sheet__title">Settings</h2>
          <button className="iconbtn" aria-label="Close settings" onClick={() => toggle(false)}>
            <IconClose size={14} />
          </button>
        </div>

        {settings === null ? (
          <p className="sheet__body">Loading…</p>
        ) : (
          <div className="settings">
            <Row label="Appearance" hint="Follow the system, or pick one.">
              <Choice<Theme>
                value={settings.theme}
                options={[
                  ['system', 'System'],
                  ['light', 'Light'],
                  ['dark', 'Dark'],
                ]}
                onChange={(theme) => save({ ...settings, theme })}
              />
            </Row>

            <Row label="Density" hint="How tall a row in the file panes is.">
              <Choice<Density>
                value={settings.density}
                options={[
                  ['comfortable', 'Comfortable'],
                  ['compact', 'Compact'],
                ]}
                onChange={(density) => save({ ...settings, density })}
              />
            </Row>

            <Row
              label="Simultaneous transfers"
              hint="Applies to transfers already running, not just the next ones."
            >
              <div className="settings__slider">
                <input
                  type="range"
                  min={1}
                  max={8}
                  step={1}
                  value={settings.concurrency}
                  aria-label="Simultaneous transfers"
                  onChange={(e) =>
                    save({ ...settings, concurrency: Number(e.currentTarget.value) })
                  }
                />
                <span className="settings__value">{settings.concurrency}</span>
              </div>
            </Row>

            <Row
              label="When a file already exists"
              hint="Ask every time, or decide once. Resume is never a default: the engine offers it only when it has proven it safe."
            >
              <Choice<ConflictAction | 'ask'>
                value={settings.defaultConflict ?? 'ask'}
                options={[
                  ['ask', 'Ask'],
                  ['overwrite', 'Overwrite'],
                  ['skip', 'Skip'],
                  ['keepBoth', 'Keep both'],
                ]}
                onChange={(choice) =>
                  save({
                    ...settings,
                    defaultConflict: choice === 'ask' ? null : choice,
                  })
                }
              />
            </Row>

            <Row label="Downloads go to" hint="Where a download lands when you have not said.">
              <div className="settings__path">
                <span className="settings__pathtext" title={settings.downloadDir ?? undefined}>
                  {settings.downloadDir ?? 'The local pane’s current folder'}
                </span>
                <button
                  className="btn btn--small"
                  onClick={() => {
                    openFolderDialog({ directory: true })
                      .then((chosen) => {
                        if (typeof chosen === 'string') {
                          save({ ...settings, downloadDir: chosen })
                        }
                      })
                      .catch((error: unknown) => toast('error', faultText(error)))
                  }}
                >
                  Choose…
                </button>
                {settings.downloadDir !== null && (
                  <button
                    className="btn btn--small"
                    onClick={() => save({ ...settings, downloadDir: null })}
                  >
                    Clear
                  </button>
                )}
              </div>
            </Row>

            <Row label="Hidden files" hint="Dotfiles, and Windows-hidden files.">
              <label className="settings__toggle">
                <input
                  type="checkbox"
                  checked={settings.showHidden}
                  onChange={(e) => save({ ...settings, showHidden: e.currentTarget.checked })}
                />
                Show them in both panes
              </label>
            </Row>
          </div>
        )}
      </div>
    </div>
  )
}

function Row({
  label,
  hint,
  children,
}: {
  label: string
  hint: string
  children: React.ReactNode
}) {
  return (
    <div className="settings__row">
      <div className="settings__label">
        <span className="settings__name">{label}</span>
        <span className="settings__hint">{hint}</span>
      </div>
      <div className="settings__control">{children}</div>
    </div>
  )
}

/** A segmented control. Radio buttons in a row, which is what they are — a dropdown
 * for three options hides two of them behind a click for no gain. */
function Choice<T extends string>({
  value,
  options,
  onChange,
}: {
  value: T
  options: [T, string][]
  onChange: (value: T) => void
}) {
  return (
    <div className="segmented" role="radiogroup">
      {options.map(([id, label]) => (
        <button
          key={id}
          role="radio"
          aria-checked={value === id}
          className={`segmented__opt${value === id ? ' segmented__opt--on' : ''}`}
          onClick={() => onChange(id)}
        >
          {label}
        </button>
      ))}
    </div>
  )
}
