import { useCallback, useEffect, useRef, useState } from 'react'
import { AppError, toAppError } from '../lib/errors'
import { EVENTS } from '../lib/events'
import { devicesList, ingestUsbStart, ingestUsbStop } from '../lib/ipc'
import { useAsync } from './useAsync'
import { useToast } from './useContexts'
import { useTauriEvent } from './useTauriEvent'
import type {
  DeviceDto,
  IngestEventDto,
  IngestUsbStartDto,
  ScanProgressDto,
  ScanSummaryDto,
  UsbModeDto,
} from '../types/api'

export type UsbFailure = { device: string; message: string }

/** How often the attached-device list is refreshed while the pane is open. A
 *  phone is plugged in *after* the pane is already on screen, so a list read only
 *  on mount would show "no device" for the whole session. */
const POLL_MS = 4000

/**
 * §7.7/§11.3 (c12) — the USB (MTP/PTP) ingest job.
 *
 * Mounted for the whole session by `TransferProvider`, above the router, and
 * never by the Backup page: the job lives in the Rust `JobRegistry` (§9.3) and
 * §9.2's channels carry no job id, so the *subscription* has to survive
 * navigation or a pass in flight goes silent while the backend keeps copying.
 *
 * `poll` is separate from that on purpose. `devices_list` enumerates MTP over
 * USB every four seconds, and doing that for the whole session — on screens
 * nobody is asking for a phone, and against hardware that can throw `E_DEVICE`
 * — spends round-trips and can raise a toast about a cable from the Dashboard.
 * So the hook stays mounted and the *poll* is what the Backup page turns on.
 *
 * `useAsync` covers the one-shot calls and their loading state; the event
 * subscription is hand-rolled because it is not a promise.
 */
export function useUsbIngest({ poll = true }: { poll?: boolean } = {}) {
  const { pushError } = useToast()
  const [selected, setSelected] = useState<string | null>(null)
  const [mode, setMode] = useState<UsbModeDto>('copy')
  const [onlyNew, setOnlyNew] = useState(false)
  const [starting, setStarting] = useState(false)
  const [pull, setPull] = useState<{ current: number; total: number; lastName: string } | null>(
    null,
  )
  const [progress, setProgress] = useState<ScanProgressDto | null>(null)
  const [summary, setSummary] = useState<ScanSummaryDto | null>(null)
  const [failure, setFailure] = useState<UsbFailure | null>(null)
  const [running, setRunning] = useState(false)
  /** Last `devices_list` failure reported to the toast, so the poll cannot repeat
   *  itself every few seconds. `null` means "healthy". */
  const lastPollError = useRef<string | null>(null)
  /** The most recent read failure, kept apart from `initial.error` because it is
   *  *cleared* by the next success — see the `error`/`stale` split below. */
  const [listError, setListError] = useState<AppError | null>(null)
  /** True while an explicitly requested read is in flight (the pane's
   *  "Buscar agora"), so the button can say so instead of looking inert. */
  const [reloading, setReloading] = useState(false)

  /**
   * The first read is `useAsync`'s job (§10.3): it mounts already loading, reads
   * exactly once, and keeps the error as state so the pane can render it instead
   * of pretending an empty list means "no phone". The poll below is a *second*
   * source, kept in its own state so a transient failure never wipes a list that
   * was already good.
   */
  const initial = useAsync<DeviceDto[]>(() => devicesList(), [])
  const [polled, setPolled] = useState<DeviceDto[] | null>(null)

  /**
   * A pass is over once it reports either end: a `summary` (it finished) or a
   * `failed` (it could not). Both must clear `running`, or the Stop button stays
   * up forever and the next Start is refused with `E_CONFLICT`.
   */
  const finish = useCallback(() => {
    setPull(null)
    setProgress(null)
    setRunning(false)
  }, [])

  useTauriEvent<IngestEventDto>(EVENTS.ingestProgress, (payload) => {
    /**
     * §9.2 puts USB and Wi-Fi on **one** channel with no job id, so this handler
     * and `useWifiIngest`'s both see every ingest event in the app. Without this
     * guard a Wi-Fi upload would fill the USB pane's meter and, on failure, would
     * push this hook's `E_DEVICE` toast — a phone failure that never happened.
     *
     * `running` is read from the latest render through `useTauriEvent`'s handler
     * ref, so it is current without re-subscribing. A §7.3 pass only emits while
     * its `JobRegistry` slot is held, so this is the discriminator the DTO does
     * not carry; see the note in `useWifiIngest` about the overlap case.
     */
    if (!running) {
      return
    }
    switch (payload.kind) {
      case 'pull':
        setPull({ current: payload.current, total: payload.total, lastName: payload.lastName })
        break
      case 'progress':
        setProgress({
          current: payload.current,
          total: payload.total,
          lastPath: payload.lastPath,
          inserted: payload.inserted,
        })
        break
      case 'summary':
        setSummary(payload)
        finish()
        break
      case 'failed':
        setFailure({ device: payload.device, message: payload.message })
        // `E_DEVICE` is the code §8.2 gives a WPD/PTP failure, and this event is
        // exactly that: a phone that was busy, unplugged, or refused a file. The
        // pane also renders it inline, so this is the "you must not miss it" copy.
        pushError(new AppError('E_DEVICE', payload.message))
        finish()
        break
      case 'dbChanged':
        // Counts moved; the Dashboard has its own `wolfs://db-changed` listener.
        break
    }
  })

  /**
   * The attached device the pass would use, resolved during render.
   *
   * Auto-selecting the only phone (one click saved, and the overwhelmingly common
   * case) used to be an effect that called `setState`, which is a second render
   * for something derivable — and it left `selected` pointing at a phone that has
   * since been unplugged. Deriving it means a vanished device resolves to `null`
   * and the Start button disables itself instead of asking the backend about a
   * phone that is not there.
   *
   * Deliberately *not* forced when several are attached: picking the wrong phone is
   * how files land in the wrong `Media/<label>/`.
   */
  const devices = polled ?? initial.data ?? []

  /**
   * Two read failures, because they mean opposite things (§10.6.4).
   *
   * `error` — there is **no** list and the last read failed, so nothing can be
   *   shown at all. It replaces the list rather than stacking on it: a phone
   *   that is plugged in and unreadable must never read as "no phone".
   * `stale` — there *is* a list from an earlier read and a later one failed. The
   *   list stays, with a note that it may be out of date. Replacing a working,
   *   already-selected device because one poll timed out would be a worse lie
   *   than a stale list, and §8.2's rule cuts both ways.
   *
   * Both clear themselves: `refresh` nulls `listError` on the next success, so a
   * transient MTP error does not stick to the pane for the whole session.
   */
  const error = devices.length === 0 ? (listError ?? initial.error) : null
  const stale = devices.length > 0 ? listError : null

  const device = devices.find((entry) => entry.friendlyName === selected) ?? null
  const resolved = device ?? (devices.length === 1 ? devices[0] : null)

  const refresh = useCallback(async () => {
    try {
      setPolled(await devicesList())
      setListError(null)
      lastPollError.current = null
    } catch (raw) {
      // Polling every few seconds must not become a toast machine: the same
      // failure would otherwise re-appear four times a minute for as long as the
      // pane is open. One report per outage, cleared by the next success.
      const error = toAppError(raw)
      setListError(error)
      if (lastPollError.current !== error.code) {
        lastPollError.current = error.code
        pushError(error)
      }
    }
  }, [pushError])

  /**
   * §11.3's "look again" — the human opened Backup and plugged the phone in, or
   * the phone only published its MTP content once it woke and was unlocked. The
   * poll below would notice within `POLL_MS` on its own, so this is a shortcut and
   * not a second mechanism: it calls the same `refresh`, keeps the same toast
   * dedupe, and lands in the same state.
   */
  const reload = useCallback(async () => {
    setReloading(true)
    try {
      await refresh()
    } finally {
      setReloading(false)
    }
  }, [refresh])

  useEffect(() => {
    // Only the interval lives here; the first read is `useAsync`'s. Nothing in
    // the effect body sets state, so this is a plain external-system subscription.
    //
    // Two things switch it off. `poll` is the Backup page's answer to "is anyone
    // looking for a device" — the event subscription above is unaffected, so a
    // pass started here keeps reporting after the human navigates away. And a
    // running pass pauses it (§7.7): enumerating MTP spends USB round-trips on a
    // phone already streaming gigabytes, and the device selection must not move
    // under a live job. It resumes by itself when the pass ends.
    if (running || !poll) {
      return
    }
    const id = window.setInterval(() => {
      void refresh()
    }, POLL_MS)
    return () => window.clearInterval(id)
  }, [refresh, running, poll])

  const start = useCallback(async () => {
    if (starting || running || resolved === null) {
      return
    }
    setStarting(true)
    setSummary(null)
    setFailure(null)
    setPull(null)
    setProgress(null)
    // Set *before* the await, deliberately. The job thread can emit `summary` for
    // an empty device before this promise resolves, and setting `running` after
    // the await would resurrect a pass that already finished.
    setRunning(true)
    const payload: IngestUsbStartDto = {
      deviceFriendly: resolved.friendlyName,
      mode,
      onlyNew,
    }
    try {
      await ingestUsbStart(payload)
    } catch (raw) {
      setRunning(false)
      pushError(toAppError(raw))
    } finally {
      setStarting(false)
    }
  }, [mode, onlyNew, pushError, resolved, running, starting])

  const stop = useCallback(async () => {
    try {
      await ingestUsbStop()
      // `running` deliberately stays true: `ingest_usb_stop` only flips the
      // backend's cancel flag, and the pass ends when it sends its terminal
      // summary (`cancelled: true`). Clearing it here would put the Stop button
      // back while the phone is still being read.
    } catch (raw) {
      pushError(toAppError(raw))
    }
  }, [pushError])

  return {
    devices,
    device: resolved,
    loadingDevices: initial.loading,
    /** We have no device list *and* the last read failed — it replaces the list.
     *  Kept apart from `failure`, which is about a *pass* failing; this one is
     *  about not being able to see the cable. */
    error,
    /** A read failed but an earlier list is still on screen. */
    stale,
    /** True while the human's own "look again" read is in flight. */
    reloading,
    reload,
    selected: resolved?.friendlyName ?? null,
    select: setSelected,
    mode,
    setMode,
    onlyNew,
    setOnlyNew,
    starting,
    running,
    pull,
    progress,
    summary,
    failure,
    start,
    stop,
  }
}
