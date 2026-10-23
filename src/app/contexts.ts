import { createContext } from 'react'
import type { AppError } from '../lib/errors'
import type { ToastKind } from '../types/api'

export type Locale = 'pt-BR' | 'en'

export type I18nContextValue = {
  locale: Locale
  setLocale: (locale: Locale) => void
  /** `{count}` style interpolation, so a translated sentence keeps its order. */
  t: (key: string, params?: Record<string, string | number>) => string
}

export type SkinContextValue = {
  skin: string
}

export type ToastItem = {
  id: number
  kind: ToastKind
  message: string
  /** c22/§12.4: an inline action on the toast — the front-end-only `[Desfazer]`
   *  of "added to a list". Absent on every other toast. */
  action?: { label: string; run: () => void }
}

export type ToastContextValue = {
  toasts: ToastItem[]
  /** `push` is imperative by design (§10.3): any module can report a problem. */
  push: (kind: ToastKind, message: string, action?: ToastItem['action']) => void
  dismiss: (id: number) => void
  /** Maps an `AppError` to its i18n key + pushes it, unless it is silent (§8.2). */
  pushError: (error: AppError) => void
  /**
   * §8.2: the user-visible text of an error, already translated.
   *
   * A component that renders an error must call THIS and never
   * `error.i18nKey ?? error.detail`: the former is a key (so it has to reach
   * `t`), and the latter is the backend's technical detail. Every surface
   * needed its own `t(error.i18nKey ?? ...)`, and Settings forgot the `t` —
   * which is how a raw `error.E_DB` could reach the screen.
   */
  errorText: (error: AppError) => string
}

export const I18nContext = createContext<I18nContextValue | null>(null)
export const SkinContext = createContext<SkinContextValue | null>(null)
export const ToastContext = createContext<ToastContextValue | null>(null)

/* ------------------------------------------------------- transfer (§11.3) */

/** Which source the bar is reporting. The tabs are the transport, not screens. */
export type TransferKind = 'usb' | 'wifi' | 'local'

/**
 * The two phases a pass actually has, plus the listing that precedes them.
 *
 * `listing` is its own phase because it is the one with no number: on a phone
 * whose albums `MEDIA_ROOTS` does not recognise it is two verified walks over
 * every object before the first byte moves — measured at ~258 s for 3 288 files
 * on real hardware. A bar that only knew `copy`/`index` would sit at zero with
 * no explanation for four minutes, which reads as a hang.
 */
export type TransferPhase = 'listing' | 'copy' | 'index'

/**
 * One running job, normalised. `total: 0` is §9.2's own "the walk cannot know
 * its size yet" marker, so the bar is indeterminate rather than at 0 %.
 *
 * `cancel` is null when the job has no cancel: the Wi-Fi server can be stopped
 * (which is what stops it accepting uploads), but there is no per-file abort on
 * the upload path, and offering one would be a control that does nothing.
 */
export type TransferJob = {
  kind: TransferKind
  /** The device's friendly name, or the picked folder's name. */
  label: string
  phase: TransferPhase
  current: number
  total: number
  /** The file being pulled right now — empty while listing or indexing. */
  lastName: string
  cancel: (() => void) | null
}

/**
 * The ingest hooks, owned above the router.
 *
 * They used to be mounted by the Backup screen, which read as "a running pass
 * keeps reporting while you navigate away" — but the screen *unmounts* on
 * navigation, so the subscription died with it and the pass went silent. Lifting
 * them here is what makes that promise true, and it is also what lets the shell
 * render one bar for every screen instead of one per tab.
 */
export type TransferContextValue = {
  usb: ReturnType<typeof import('../hooks/useUsbIngest').useUsbIngest>
  wifi: ReturnType<typeof import('../hooks/useWifiIngest').useWifiIngest>
  scan: ReturnType<typeof import('../hooks/useScan').useScan>
  /** The Disco-local source §11.3 picked. Lifted for the same reason. */
  folder: { path: string; name: string } | null
  setFolder: (folder: { path: string; name: string } | null) => void
  /**
   * Whether the Backup page is on screen, which is what turns the USB device
   * poll on. It is not derivable here — the router is below this component — so
   * the page reports it on mount and again on unmount.
   */
  backupOpen: boolean
  setBackupOpen: (open: boolean) => void
  /** The job the bar should paint, or null when nothing is running. */
  job: TransferJob | null
}

export const TransferContext = createContext<TransferContextValue | null>(null)
