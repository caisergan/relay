import { Component, type ErrorInfo, type ReactNode } from 'react'

/** Keeps a render or effect failure from becoming an empty window.
 *
 * React tears the whole tree out of the DOM when nothing catches an error, and with a
 * dark `body` background that looks exactly like an app that never started — which is
 * how an infinite-render bug in a store selector once cost a debugging session. A
 * legible panel names the failure instead. Phase 5 adds the Rust-side crash dialog;
 * this is its frontend half. */
interface Props {
  children: ReactNode
}

interface State {
  error: Error | null
}

export class ErrorBoundary extends Component<Props, State> {
  override state: State = { error: null }

  static getDerivedStateFromError(error: Error): State {
    return { error }
  }

  override componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error('Relay crashed:', error, info.componentStack)
  }

  override render(): ReactNode {
    const { error } = this.state
    if (!error) return this.props.children

    return (
      <div className="crash" role="alert">
        <div className="crash__card">
          <h1 className="crash__title">Relay stopped drawing</h1>
          <p className="crash__body">
            The interface hit an error it could not recover from. Transfers already running in
            the engine are unaffected; reloading reconnects to them.
          </p>
          <pre className="crash__detail">{error.message}</pre>
          <button className="btn btn--primary" onClick={() => window.location.reload()}>
            Reload
          </button>
        </div>
      </div>
    )
  }
}
