import { useCallback, useMemo, useState } from 'react'
import { open } from '@tauri-apps/plugin-dialog'
import { Button, PageHeader } from '../../components/ui'
import { useI18n } from '../../hooks/useContexts'
import { useScan } from '../../hooks/useScan'
import { toAppError } from '../../lib/errors'
import { formatCount, ratioOf } from '../../lib/format'

type Source = 'usb' | 'wifi' | 'local'

type PickedFolder = {
  path: string
  name: string
}

/**
 * §11.4/§12.1 Backup — **one** page with USB | Wi-Fi | Disco local as source
 * tabs (C-15 / C-16 amendment, human-confirmed 2026-09-29 and built in c8).
 * The tabs are the transport, not separate screens: the USB/Wi-Fi machinery is
 * c11/c12, and the Disco local pane reuses [C-3f]/[C-3g]/[C-3h] against
 * the §7.3 pipeline in its copy-only form (§11.3, §14 — the source is the
 * human's tree and is never mutated).
 */
export function BackupScreen() {
  const { t } = useI18n()
  const [source, setSource] = useState<Source>('usb')
  const [folder, setFolder] = useState<PickedFolder | null>(null)
  const [pickError, setPickError] = useState<string | null>(null)

  const device = useMemo(() => folder?.name ?? '', [folder])
  // `useScan` is mounted for every tab (the hook's contract: a scan started on
  // this screen keeps running and reporting even while another tab is shown).
  const scan = useScan(device, folder?.path ?? null)

  /**
   * The native Windows folder dialog (human-confirmed 2026-10-01, after the
   * in-app picker proved worse): the OS owns the browsing — sidebar, search,
   * "Selecionar Pasta" — so the human picks a folder instead of walking into
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
  }, [t])

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

      <section className="md-card backup-section" data-testid="c-3n-section">
        <h2>{t(`backup.${source}`)}</h2>
        <p>{t('backup.localCopyOnly')}</p>

        {source === 'local' ? (
          <div>
            {folder ? (
              <>
                <p className="md-card__hint md-card__hint--path" title={folder.path}>
                  {folder.path}
                </p>

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
          </div>
        ) : source === 'usb' ? (
          <div>
            <p className="md-card__hint">Device: iPhone (via USB)</p>
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
          </div>
        ) : source === 'wifi' ? (
          <div>
            <p className="md-card__hint">Device: iPhone (via Wi-Fi)</p>
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
          </div>
        ) : null}
      </section>
    </div>
  )
}

/**
 * The `Media/<label>/` name (§11.3): the picked folder's own name, trimmed and
 * trimmed of trailing separators. `C:\` is not a sensible label, so the volume
 * root falls back to the drive letter itself.
 */
function nameOf(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, '')
  const leaf = trimmed.split(/[\\/]/).pop()
  return leaf && leaf.length > 0 ? leaf : trimmed || path
}
