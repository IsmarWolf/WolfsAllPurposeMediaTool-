import { useState, type ReactNode } from 'react'
import { useScan } from '../hooks/useScan'
import { useUsbIngest } from '../hooks/useUsbIngest'
import { useWifiIngest } from '../hooks/useWifiIngest'
import { TransferContext, type TransferContextValue, type TransferJob } from './contexts'

/**
 * Owns the three ingest hooks for the whole app session.
 *
 * §9.3's `JobRegistry` and §7.8's listener both outlive any screen, and §9.2's
 * channels carry no job id, so exactly one subscriber has to exist for the
 * session — not one per mount of the Backup page. Before this, navigating away
 * from Backup unmounted the hooks, `useTauriEvent`'s cleanup ran, and a pass in
 * flight stopped reporting while the backend happily kept copying.
 *
 * The USB device poll is the one thing that must *not* run forever: enumerating
 * MTP spends USB round-trips every four seconds for the whole session, on a
 * screen that is not even asking for a device. So the hooks stay mounted (their
 * event subscriptions are what matter) and only the poll is gated on the Backup
 * page being open — see `useTransferPolling` below.
 */
export function TransferProvider({ children }: { children: ReactNode }) {
  const [backupOpen, setBackupOpen] = useState(false)
  const usb = useUsbIngest({ poll: backupOpen })
  const wifi = useWifiIngest()
  const [folder, setFolder] = useState<{ path: string; name: string } | null>(null)
  const scan = useScan(folder?.name ?? '', folder?.path ?? null)

  /**
   * Which job the bottom bar paints.
   *
   * Only one pass runs at a time in practice — `scan_start` is `E_CONFLICT`
   * while another scan holds the slot (§7.3.1) and each transport holds its own —
   * so the order here is a precedence, not a merge: the USB pass first because
   * it is the one that can be deleting from a phone, then the folder scan, then
   * an upload.
   *
   * The phase comes from *which field the backend last wrote*, never from the
   * click. A `pull` with `total: 0` is the listing (see `TransferPhase`), a
   * `pull` with a total is the copy, and a `progress` is §7.3's indexing pass.
   * Inferring it any other way would put the bar a phase ahead of the work.
   *
   * Deliberately not memoised. Every one of these hooks re-renders this
   * component on every progress event — that is the whole mechanism — so a
   * memo here would be invalidated by each event anyway, and the dependency list
   * it needs is the fields this body reads, spelled out by hand and then
   * re-derived by the React Compiler into something subtly different.
   */
  function jobOf(): TransferJob | null {
    if (usb.running) {
      const cancel = () => {
        void usb.stop()
      }
      const label = usb.selected ?? usb.device?.friendlyName ?? ''
      if (usb.progress) {
        return {
          kind: 'usb',
          label,
          phase: 'index',
          current: usb.progress.current,
          total: usb.progress.total,
          lastName: '',
          cancel,
        }
      }
      return {
        kind: 'usb',
        label,
        phase: usb.pull && usb.pull.total > 0 ? 'copy' : 'listing',
        current: usb.pull?.current ?? 0,
        total: usb.pull?.total ?? 0,
        lastName: usb.pull?.lastName ?? '',
        cancel,
      }
    }

    if (scan.running) {
      return {
        kind: 'local',
        label: folder?.name ?? '',
        phase: scan.progress && scan.progress.total > 0 ? 'index' : 'listing',
        current: scan.progress?.current ?? 0,
        total: scan.progress?.total ?? 0,
        lastName: '',
        cancel: () => {
          scan.cancel()
        },
      }
    }

    // The Wi-Fi hook clears `progress` on the summary, so an upload in flight is
    // exactly "there is progress and no summary yet". No cancel: the upload path
    // has no abort, and `wifi.stop` would tear down the listener the human may
    // still be using — a different action from the one this button would claim.
    if (wifi.progress && !wifi.summary) {
      return {
        kind: 'wifi',
        label: wifi.info?.lanIp ?? '',
        phase: wifi.progress.total > 0 ? 'index' : 'listing',
        current: wifi.progress.current,
        total: wifi.progress.total,
        lastName: wifi.progress.lastPath,
        cancel: null,
      }
    }

    return null
  }

  const value: TransferContextValue = {
    usb,
    wifi,
    scan,
    folder,
    setFolder,
    backupOpen,
    setBackupOpen,
    job: jobOf(),
  }

  return <TransferContext.Provider value={value}>{children}</TransferContext.Provider>
}
