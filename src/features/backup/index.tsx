import { useCallback, useEffect, useState } from 'react'
import { open } from '@tauri-apps/plugin-dialog'
import { Button, PageHeader } from '../../components/ui'
import { useI18n, useTransfer } from '../../hooks/useContexts'
import { toAppError } from '../../lib/errors'
import { formatCount, ratioOf } from '../../lib/format'
import { UsbPane } from './UsbPane'
import { WifiPane } from './WifiPane'

type Source = 'usb' | 'wifi' | 'local'

/**
 * §11.4/§12.1 Backup — **one** page with USB | Wi-Fi | Disco local as source
 * tabs (C-15 / C-16 amendment, human-confirmed 2026-09-29 and built in c8).
 * The tabs are the transport, not separate screens: USB and Wi-Fi are their own
 * panes (c12/c13) over their own commands, while the Disco local pane reuses
 * `[C-3f]`/`[C-3g]`/`[C-3h]` against the §7.3 pipeline in its **copy-only** form
 * (§11.3, §14 — the source is the human's tree and is never mutated).
 *
 * **Both transports are built** (c12/c13): USB over §7.7's real WPD/MTP
 * transport (`winmtp`) and Wi-Fi over c11's real listener. Neither tab is wired to
 * `scan.start` — a transport tab must never point at the folder scanner.
 *
 * None of the three ingest hooks are mounted here any more. They belong to
 * `TransferProvider`, above the router, and this page reads them through
 * `useTransfer()`: §9.2's channels carry no job id, so a second subscriber would
 * be a second copy of the same state rather than a second view of it, and a pass
 * started here has to keep reporting after the human navigates away — which is
 * also what the shell's bottom bar paints (§11.3). The one thing this page still
 * owns is the *USB device poll*, because it is the only thing here that asks
 * "is a phone attached"; it reports its own visibility on mount.
 *
 * They used to be wired to `scan.start` here, which was wrong in a way that
 * only looked right: `scan_start(device, folder)` with no folder is §7.3's
 * **in-place mop-up** of `Media/<device>/`, and on those tabs `device` was the
 * empty string — so "Iniciar importação" would have re-scanned `Media//`.
 * Never point a transport tab at the folder scanner.
 */
export function BackupScreen() {
  const { t } = useI18n()
  const [source, setSource] = useState<Source>('usb')
  const [pickError, setPickError] = useState<string | null>(null)
  const { usb, wifi, scan, folder, setFolder, setBackupOpen } = useTransfer()

  const device = folder?.name ?? ''

  useEffect(() => {
    setBackupOpen(true)
    return () => setBackupOpen(false)
  }, [setBackupOpen])

  /**
   * The native Windows folder dialog (human-confirmed 2026-10-01, after the
   * in-app picker proved worse): the OS owns the browsing — sidebar, search,
   * "Selecionar Pasta" — so the human picks a folder instead of walking into a
   * one to escape it. Cancelling resolves `null` and changes nothing; the
   * returned path is display state passed to `scan_start` and never persisted
   * (§6.1).
   */
  const pickFolder = useCallback(async () => {
    setPickError(null)
    try {
      const picked = await open({
        directory: true,
        multiple: false,
        title: t('backup.pickerTitle'),
      })
      if (typeof picked !== 'string') {
        return
      }
      setFolder({ path: picked, name: nameOf(picked) })
    } catch (raw) {
      setPickError(toAppError(raw).message)
    }
  }, [setFolder, t])

  const tabLabel: Record<Source, string> = {
    usb: t('backup.usb'),
    wifi: t('backup.wifi'),
    local: t('backup.local'),
  }

  return (
    <div className="screen">
      <PageHeader title={t('nav.backup')} subtitle={t('backup.subtitle')} />

      <div className="source-tabs" role="tablist" aria-label={t('backup.source')}>
        {(['usb', 'wifi', 'local'] as const).map((entry) => (
          <button
            key={entry}
            type="button"
            role="tab"
            aria-selected={source === entry}
            className={`source-tabs__tab${source === entry ? ' source-tabs__tab--active' : ''}`}
            onClick={() => setSource(entry)}
          >
            {tabLabel[entry]}
          </button>
        ))}
      </div>

      {source === 'usb' ? (
        <UsbPane ingest={usb} />
      ) : source === 'wifi' ? (
        <WifiPane ingest={wifi} />
      ) : (
        <section className="md-card backup-local" data-testid="c-3n-local">
          <h2>{t('backup.local')}</h2>
          <p>{t('backup.localCopyOnly')}</p>

          {folder ? (
            <>
              <p className="md-card__hint md-card__hint--path" title={folder.path}>
                {folder.path}
              </p>

              {/* C-3n2: where the copy lands. The label is the folder's own
                  name; §5.4 sanitizes it again on the Rust side, so what is
                  shown here is a preview of `Media/<label>/`, not the promise
                  of an exact path. */}
              <p className="md-card__hint">{t('backup.destination', { label: device })}</p>

              {/* C-3n3 (Mover) is deferred — §7.3.1 — so the mode is fixed and
                  stated rather than offered: the source is the human's library
                  and is never mutated (§14). */}
              <p className="md-card__hint">{t('backup.modeCopyOnly')}</p>

              <div className="md-card__row">
                {scan.running ? (
                  <>
                    <Button variant="tonal" onClick={scan.cancel}>
                      {t('dash.scanCancel')}
                    </Button>
                    <span className="md-card__hint">
                      {scan.progress?.total
                        ? `${formatCount(scan.progress.current)} / ${formatCount(scan.progress.total)}`
                        : t('dash.scanCollecting')}
                    </span>
                  </>
                ) : (
                  <Button variant="filled" onClick={scan.start} data-testid="c-3n4-start">
                    {t('backup.importStart')}
                  </Button>
                )}
                <Button variant="text" onClick={pickFolder}>
                  {t('backup.importChange')}
                </Button>
              </div>

              {scan.progress && scan.progress.total > 0 ? (
                <div className="md-meter" aria-label={t('dash.scanProgressLabel')}>
                  <div
                    className="md-meter__fill md-meter__fill--images"
                    style={{
                      width: `${ratioOf(scan.progress.current, scan.progress.total) * 100}%`,
                    }}
                  />
                </div>
              ) : null}
            </>
          ) : (
            <div className="md-card__row">
              <Button variant="tonal" onClick={pickFolder} data-testid="c-3n1-pick">
                {t('backup.pickFolder')}
              </Button>
            </div>
          )}

          {pickError ? <p className="md-card__hint md-card__hint--error">{pickError}</p> : null}
        </section>
      )}
    </div>
  )
}

/**
 * The `Media/<label>/` name (§11.3): the picked folder's own name, trimmed of
 * trailing separators. `C:\` is not a sensible label, so the volume root falls
 * back to the drive letter itself.
 */
function nameOf(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, '')
  const leaf = trimmed.split(/[\\/]/).pop()
  return leaf && leaf.length > 0 ? leaf : trimmed || path
}
