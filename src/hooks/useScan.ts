import { useCallback, useEffect, useRef, useState } from 'react'
import { useI18n, useToast } from './useContexts'
import { toAppError } from '../lib/errors'
import { EVENTS } from '../lib/events'
import { scanCancel, scanStart } from '../lib/ipc'
import { useTauriEvent } from './useTauriEvent'
import type { ScanProgressDto } from '../types/api'

/**
 * §7.3/§11.3 — one scan at a time, tracked by the `wolfs://progress/scan`
 * channel (§9.2) and resolved by the `scan_start` promise, which is what
 * `busy` waits on.
 *
 * `folder` selects the tree: `null` (or empty) is §7.3's in-place mop-up of
 * `Media/<device>/`, a path is §11.3's copy-only Disco-local ingest.
 *
 * `phase` is `idle | scanning`: the UI never guesses it is running — the first
 * progress event, not the click, is what moves it. `total: 0` carries §9.2's
 * "the walk cannot know its size yet" state, so the bar is indeterminate until
 * `total` arrives.
 */
export function useScan(device: string, folder: string | null = null) {
  const { t } = useI18n()
  const { push, pushError } = useToast()
  const [running, setRunning] = useState(false)
  const [progress, setProgress] = useState<ScanProgressDto | null>(null)
  const inFlight = useRef(false)

  useTauriEvent<ScanProgressDto>(EVENTS.scanProgress, (payload) => {
    setProgress(payload)
  })

  const start = useCallback(async () => {
    if (inFlight.current) {
      // E_CONFLICT is the backend refusing a second scan; the flag is the same
      // decision made a render earlier, so a double click has no toast.
      return
    }
    inFlight.current = true
    setRunning(true)
    setProgress(null)
    try {
      const summary = await scanStart(device, folder ?? undefined)
      if (summary.cancelled) {
        push('info', t('dash.scanCancelled'))
      } else {
        push(
          'success',
          t('dash.scanDone', {
            inserted: summary.inserted,
            duplicates: summary.duplicates,
            repaired: summary.repaired,
          }),
        )
      }
    } catch (raw) {
      pushError(toAppError(raw))
    } finally {
      inFlight.current = false
      setRunning(false)
    }
  }, [device, folder, push, pushError, t])

  const cancel = useCallback(() => {
    void scanCancel()
  }, [])

  // A scan left running when the screen unmounts keeps running (the promise
  // still resolves, the toast still lands) — nothing to clean here; only the
  // *local* state dies, not the job.
  useEffect(() => {
    return () => {
      inFlight.current = false
    }
  }, [])

  return { running, progress, start, cancel }
}
