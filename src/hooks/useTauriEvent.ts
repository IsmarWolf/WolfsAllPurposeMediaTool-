import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { useEffect, useRef, useState } from 'react'
import { AppError } from '../lib/errors'
import { EVENTS } from '../lib/events'
import { statsGet } from '../lib/ipc'
import type { RootInfoDto, StatsDto } from '../types/api'
import { useAsync } from './useAsync'
import { useToast } from './useContexts'

/** §10.3 `useDashStats()` — the Dashboard numbers, refreshed by `wolfs://db-changed`. */
export function useDashStats() {
  const stats = useAsync<StatsDto>(() => statsGet(), [])
  const bump = useDbChanged(stats.rerun)
  return { ...stats, bump }
}

/**
 * §10.3 `useTauriEvent<T>(channel, handler?)` — subscription with cleanup.
 *
 * A handler is optional because `wolfs://db-changed` is only a bump signal; the
 * screen that cares re-reads its own data.
 */
export function useTauriEvent<T>(channel: string, handler?: (payload: T) => void): number {
  const [bump, setBump] = useState(0)
  const handlerRef = useRef(handler)

  useEffect(() => {
    handlerRef.current = handler
  }, [handler])

  useEffect(() => {
    let alive = true
    let unlisten: UnlistenFn | undefined

    void listen<T>(channel, ({ payload }) => {
      if (!alive) {
        return
      }
      setBump((current) => current + 1)
      handlerRef.current?.(payload)
    }).then((fn) => {
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
  }, [channel])

  return bump
}

/** The invalidation signal itself: any command that writes emits it. */
export function useDbChanged(onChanged: () => void) {
  return useTauriEvent<null>(EVENTS.dbChanged, onChanged)
}

/**
 * §11.0 `[Z1d]` — the resolved root of this boot. Read once, then displayed.
 *
 * An unresolvable root is the degraded-boot case: Zone 1 falls back to "unknown"
 * and the failure still has to reach the user, so the read goes through the
 * §10.3 hook and its error goes to the toast (§8.2). A bare `void resolve()`
 * would leak the rejection to the window and leave the badge as the only clue.
 */
export function useRootInfo(resolve: () => Promise<RootInfoDto>) {
  const { data, error } = useAsync<RootInfoDto>(resolve, [resolve])
  const { pushError } = useToast()
  const reported = useRef<AppError | null>(null)

  useEffect(() => {
    if (error && error !== reported.current) {
      reported.current = error
      pushError(error)
    }
  }, [error, pushError])

  return data
}
