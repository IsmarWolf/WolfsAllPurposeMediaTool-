import { useEffect, useCallback } from 'react'
import { useI18n, useToast } from '../../hooks/useContexts'
import { useAsync } from '../../hooks/useAsync'
import { toAppError } from '../../lib/errors'
import { mediaDetail, mediaReveal } from '../../lib/ipc'
import { formatBytes } from '../../lib/format'
import type { MediaDto } from '../../types/api'
import { thumbUrl } from './assetUrl'

/**
 * §11.4 `MediaLightbox` — the media detail overlay.
 *
 * Shows the 400px preview, filename, metadata table, and location. Keyboard:
 * `Esc` close, `←/→` navigate (placeholder for now — single-item only).
 */
export function MediaLightbox({
  id,
  root,
  onClose,
}: {
  id: string
  root: string | null
  onClose: () => void
}) {
  const { t } = useI18n()
  const { pushError } = useToast()
  const { data, loading, error } = useAsync<MediaDto | null>(() => mediaDetail(id), [id])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  const handleReveal = useCallback(async () => {
    try {
      await mediaReveal(id)
    } catch (raw) {
      pushError(toAppError(raw))
    }
  }, [id, pushError])

  if (loading) {
    return (
      <div className="md-lightbox" role="dialog" aria-modal="true" aria-label={t('media.lightbox')}>
        <div className="md-lightbox__content">
          <div className="md-skeleton" style={{ height: 320, width: '100%' }} />
        </div>
      </div>
    )
  }

  if (error || !data) {
    return (
      <div className="md-lightbox" role="dialog" aria-modal="true" aria-label={t('media.lightbox')}>
        <div className="md-lightbox__content">
          <p className="md-error__message">{t('error.unavailable')}</p>
          <button type="button" className="md-btn md-btn--tonal" onClick={onClose}>
            {t('common.close')}
          </button>
        </div>
      </div>
    )
  }

  return (
    <div className="md-lightbox" role="dialog" aria-modal="true" aria-label={t('media.lightbox')}>
      <div className="md-lightbox__backdrop" onClick={onClose} />
      <div className="md-lightbox__content">
        <div className="md-lightbox__main">
          <img
            src={thumbUrl(root, data.thumb400)}
            alt={data.relativePath}
            className="md-lightbox__image"
            onError={(e) => {
              ;(e.target as HTMLImageElement).style.display = 'none'
            }}
          />
        </div>
        <div className="md-lightbox__panel">
          <h2 className="md-lightbox__title">{data.relativePath.split('/').pop()}</h2>
          <dl className="kv">
            <dt>{t('media.fileType')}</dt>
            <dd>{data.fileType}</dd>
            <dt>{t('media.fileSize')}</dt>
            <dd>{formatBytes(data.fileSize)}</dd>
            <dt>{t('media.capturedAt')}</dt>
            <dd>{data.capturedAt ?? '—'}</dd>
            <dt>{t('media.device')}</dt>
            <dd>{data.deviceName ?? '—'}</dd>
            <dt>{t('media.location')}</dt>
            <dd>{data.location?.city ? `${data.location.city}, ${data.location.country}` : '—'}</dd>
          </dl>
          <div className="md-lightbox__actions">
            <button type="button" className="md-btn md-btn--tonal" onClick={handleReveal}>
              {t('media.openFolder')}
            </button>
            <button type="button" className="md-btn md-btn--text" onClick={onClose}>
              {t('common.close')}
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}
