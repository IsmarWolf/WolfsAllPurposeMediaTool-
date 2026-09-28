import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { useEffect, useRef } from 'react'
import { useToast } from '../../hooks/useContexts'
import { EVENTS } from '../../lib/events'
import type { ToastPayload } from '../../types/api'

/**
 * The `wolfs://toast` sock (§12.4 state 4 and §9.2): background work cannot
 * open a modal, so it emits a toast. A dismiss button on every toast keeps the
 * 6s auto-dismiss honest when the user is reading something else.
 */
export function ToastSock() {
  const { toasts, dismiss, push } = useToast()
  // The listener is registered once; the newest `push` is read through a ref so
  // a new toast does not unsubscribe and resubscribe the Tauri event.
  const pushRef = useRef(push)
  useEffect(() => {
    pushRef.current = push
  }, [push])

  useEffect(() => {
    let alive = true
    let unlisten: UnlistenFn | undefined

    void listen<ToastPayload>(EVENTS.toast, ({ payload }) => {
      if (alive) {
        pushRef.current(payload.kind, payload.message)
      }
    }).then((fn) => {
      // The effect may have been cleaned up while `listen` was in flight.
      if (alive) {
        unlisten = fn
      } else {
        fn()
      }
    })

    return () => {
      alive = false
      unlisten?.()
    }
  }, [])

  if (toasts.length === 0) {
    return null
  }

  return (
    <div className="md-toasts" role="status" aria-live="polite">
      {toasts.map((toast) => (
        <div key={toast.id} className={`md-toast md-toast--${toast.kind}`}>
          <span>{toast.message}</span>
          <button
            type="button"
            className="md-toast__close"
            aria-label="Fechar"
            onClick={() => dismiss(toast.id)}
          >
            ×
          </button>
        </div>
      ))}
    </div>
  )
}
