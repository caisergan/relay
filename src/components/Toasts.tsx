import { useEffect } from 'react'

import { useUiStore } from '@/state/uiStore'

const DISMISS_AFTER_MS = 6000

export function Toasts() {
  const toasts = useUiStore((s) => s.toasts)
  const dismiss = useUiStore((s) => s.dismissToast)

  useEffect(() => {
    if (toasts.length === 0) return
    const timers = toasts.map((toast) => setTimeout(() => dismiss(toast.id), DISMISS_AFTER_MS))
    return () => timers.forEach(clearTimeout)
  }, [toasts, dismiss])

  return (
    <div className="toasts" role="status" aria-live="polite">
      {toasts.map((toast) => (
        <div key={toast.id} className={`toast toast--${toast.kind}`}>
          <span style={{ flex: 1 }}>{toast.text}</span>
          <button className="iconbtn" onClick={() => dismiss(toast.id)} aria-label="Dismiss">
            ×
          </button>
        </div>
      ))}
    </div>
  )
}
