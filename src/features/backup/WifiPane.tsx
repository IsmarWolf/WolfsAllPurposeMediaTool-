import { useState } from 'react'
import { Button } from '../../components/ui'
import { useI18n } from '../../hooks/useContexts'
import type { useWifiIngest } from '../../hooks/useWifiIngest'
import { formatCount, ratioOf } from '../../lib/format'
import { QrCode } from './QrCode'

export type WifiIngest = ReturnType<typeof useWifiIngest>

/**
 * §11.3 Backup → Wi-Fi, §12.2's `[C-3i Iniciar servidor]` / `[C-3k Stop]`.
 *
 * The pane is presentational on purpose: `useWifiIngest` is mounted by the
 * Backup screen and passed in, because the server belongs to `AppState` and
 * outlives this component — a listener started here must survive a tab switch,
 * and the subscription must keep reporting while another tab is shown.
 *
 * Copy (pt-BR) is the UI's language; the code, keys and comments are English.
 */
export function WifiPane({ ingest }: { ingest: WifiIngest }) {
  const { t } = useI18n()
  const { info, serving, starting, progress, summary, failure, start, stop } = ingest
  const [copied, setCopied] = useState(false)

  const url = info ? `http://${info.lanIp}:${info.port}` : ''

  /**
   * Typing a LAN address on a phone keyboard is miserable, so the address is
   * selectable and copyable. The QR is the good path; this is the fallback when
   * the camera will not focus, and it costs no dependency because the webview
   * already ships the async clipboard API. A refused permission is not an error
   * worth a toast — the `<code>` element stays selectable either way.
   */
  const copyAddress = async () => {
    try {
      await navigator.clipboard?.writeText(url)
      setCopied(true)
      window.setTimeout(() => setCopied(false), 2000)
    } catch {
      setCopied(false)
    }
  }

  return (
    <section className="md-card backup-wifi" data-testid="c-3i-wifi">
      <h2>{t('backup.wifi')}</h2>
      <p>{t('backup.wifiIdle')}</p>

      {serving && info ? (
        <>
          <div className="wifi-serve">
            <div className="wifi-serve__code">
              <QrCode matrix={info.qrMatrix} label={t('backup.wifiQrLabel')} />
            </div>
            <div className="wifi-serve__guide">
              <ol className="wifi-serve__steps">
                <li>{t('backup.wifiStep1')}</li>
                <li>{t('backup.wifiStep2')}</li>
              </ol>

              <div className="wifi-serve__address">
                <code className="wifi-serve__url" title={url}>
                  {url}
                </code>
                <Button variant="text" onClick={() => void copyAddress()}>
                  {copied ? t('backup.wifiCopied') : t('backup.wifiCopy')}
                </Button>
              </div>

              <p className="md-card__hint" data-testid="c-3i-waiting">
                {t('backup.wifiWaiting', { n: formatCount(info.uploads) })}
              </p>
              {/* iOS gates local-network access behind its own prompt and Android
                  can gate it behind an install-time permission. Saying so up
                  front beats a phone that simply never connects. */}
              <p className="md-card__hint">{t('backup.wifiLocalNetNote')}</p>
            </div>
          </div>

          {progress ? (
            <>
              <p className="md-card__hint">
                {progress.total
                  ? `${formatCount(progress.current)} / ${formatCount(progress.total)}`
                  : t('dash.scanCollecting')}
              </p>
              {progress.total > 0 ? (
                <div className="md-meter" aria-label={t('backup.wifiProgressLabel')}>
                  <div
                    className="md-meter__fill md-meter__fill--images"
                    style={{ width: `${ratioOf(progress.current, progress.total) * 100}%` }}
                  />
                </div>
              ) : null}
            </>
          ) : null}

          {summary ? (
            <p className="md-card__hint" data-testid="c-3i-summary">
              {t('backup.wifiResult', {
                inserted: formatCount(summary.inserted),
                duplicates: formatCount(summary.duplicates),
              })}
            </p>
          ) : null}
        </>
      ) : null}

      {failure ? (
        <p className="md-card__hint md-card__hint--error">
          {t('backup.wifiFailed', { device: failure.device, message: failure.message })}
        </p>
      ) : null}

      <div className="md-card__row">
        {serving ? (
          <Button variant="tonal" onClick={() => void stop()} data-testid="c-3k-stop">
            {t('backup.wifiStop')}
          </Button>
        ) : (
          <Button
            variant="filled"
            onClick={() => void start()}
            disabled={starting}
            data-testid="c-3i-start"
          >
            {starting ? t('backup.wifiStarting') : t('backup.wifiStart')}
          </Button>
        )}
      </div>
    </section>
  )
}
