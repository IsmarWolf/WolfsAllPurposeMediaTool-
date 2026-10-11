import { useCallback, useEffect, useRef, useState } from 'react'
import { toAppError } from '../lib/errors'
import { EVENTS } from '../lib/events'
import { ingestWifiStart, ingestWifiStop } from '../lib/ipc'
import { useToast } from './useContexts'
import { useTauriEvent } from './useTauriEvent'
import type { IngestEventDto, ScanProgressDto, ScanSummaryDto, WifiInfoDto } from '../types/api'

/**
 * How often the live upload counter is re-read while the server is up. §8.1
 * makes `ingest_wifi_start` idempotent *specifically* so the pane can poll it for
 * "aguardando uploads… n enviados" without every poll looking like an error.
 */
const POLL_MS = 2000

export type WifiFailure = { device: string; message: string }

/**
 * §7.8/§11.3 — the Wi-Fi server's lifecycle and the ingest it triggers.
 *
 * The server is owned by `AppState`, not by this hook: it outlives the pane, so
 * switching tabs (or the whole screen) never drops a listener. This hook is
 * mounted unconditionally by the Backup screen for the same reason `useScan` is
 * — a running job keeps reporting.
 *
 * The ingest result is shown **inline** rather than as a toast, because every
 * upload runs its own §7.3 pass and therefore emits its own summary; a toast per
 * photo would be noise. Only a device failure is pushed, because that is the one
 * outcome the human must not miss.
 */
export function useWifiIngest() {
  const { push, pushError } = useToast()
  const [info, setInfo] = useState<WifiInfoDto | null>(null)
  const [starting, setStarting] = useState(false)
  const [progress, setProgress] = useState<ScanProgressDto | null>(null)
  const [summary, setSummary] = useState<ScanSummaryDto | null>(null)
  const [failure, setFailure] = useState<WifiFailure | null>(null)

  const serving = info !== null

  /**
   * `ingest_wifi_start` doubles as the poll, so a poll already in flight when the
   * human presses Stop could land *after* the server went down and start a new
   * one — the pane would spring back to life on its own. The epoch is bumped
   * before `stop()` awaits, and a refresh only commits if its epoch is still
   * current, which discards exactly those late answers.
   */
  const epoch = useRef(0)

  useTauriEvent<IngestEventDto>(EVENTS.ingestProgress, (payload) => {
    /**
     * §9.2 puts USB and Wi-Fi on **one** channel and carries no job id, so this
     * handler and `useUsbIngest`'s both see every ingest event in the app —
     * including each other's. Without this guard a USB pass that failed while the
     * human sat on the Wi-Fi tab would write "Falha em iPhone: …" into the Wi-Fi
     * pane and pop its toast, which is a lie about a server that never touched
     * the phone.
     *
     * Liveness is the only discriminator available without changing §8.1's DTO,
     * and it is a real one: an upload can only arrive while this server is up.
     * It is not sufficient for the overlap case (a USB pass *and* an upload at
     * once), which is why the honest fix is a source tag on the event rather than
     * a bigger guard here.
     */
    if (!serving) {
      return
    }
    switch (payload.kind) {
      case 'progress':
        setProgress({
          current: payload.current,
          total: payload.total,
          lastPath: payload.lastPath,
          inserted: payload.inserted,
        })
        break
      case 'summary':
        setProgress(null)
        setSummary(payload)
        break
      case 'failed':
        setFailure({ device: payload.device, message: payload.message })
        push('error', payload.message)
        break
      case 'dbChanged':
        // Counts moved; the Dashboard has its own `wolfs://db-changed` listener.
        break
    }
  })

  const refresh = useCallback(async () => {
    const mine = epoch.current
    try {
      const next = await ingestWifiStart()
      if (epoch.current === mine) {
        setInfo(next)
      }
    } catch (raw) {
      if (epoch.current === mine) {
        pushError(toAppError(raw))
      }
    }
  }, [pushError])

  useEffect(() => {
    if (!serving) {
      return
    }
    const id = window.setInterval(() => {
      void refresh()
    }, POLL_MS)
    return () => window.clearInterval(id)
  }, [serving, refresh])

  const start = useCallback(async () => {
    if (starting) {
      return
    }
    setStarting(true)
    setSummary(null)
    setFailure(null)
    setProgress(null)
    try {
      setInfo(await ingestWifiStart())
    } catch (raw) {
      pushError(toAppError(raw))
    } finally {
      setStarting(false)
    }
  }, [pushError, starting])

  const stop = useCallback(async () => {
    // Bump first and clear `info` first: tearing the poll down synchronously is
    // what keeps the next tick from resurrecting the listener.
    epoch.current += 1
    setInfo(null)
    try {
      await ingestWifiStop()
    } catch (raw) {
      pushError(toAppError(raw))
    }
  }, [pushError])

  return { info, serving, starting, progress, summary, failure, start, stop }
}
