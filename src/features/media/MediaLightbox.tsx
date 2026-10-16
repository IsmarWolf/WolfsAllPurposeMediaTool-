import { useEffect, useCallback, useRef, useState } from 'react'
import { useI18n, useToast } from '../../hooks/useContexts'
import { useAsync } from '../../hooks/useAsync'
import { toAppError } from '../../lib/errors'
import { mediaDetail, mediaReveal, mediaScopeVideo } from '../../lib/ipc'
import { formatBytes } from '../../lib/format'
import type { MediaDto } from '../../types/api'
import { mediaUrl, thumbUrl } from './assetUrl'

/**
 * §11.4 `MediaLightbox` — the media detail overlay.
 *
 * Shows the 400px preview, filename, metadata table, and location. Keyboard:
 * `Esc` close, `←/→` navigate. The arrows are the visible half of that map
 * (`[C-4e]/[C-4g]`); the keys are bound by `MediaScreen`, which owns the page
 * order. `onPrev`/`onNext` are absent at the ends of the page, so the control
 * disappears rather than becoming a dead button.
 *
 * c13 fix (human-approved 2026-10-05): a video renders as a real `<video>`
 * player, fed from its `Media/` file — which requires the per-file scope grant
 * `media_scope_video` to land first, so the element mounts only after the
 * promise resolves. Images keep the `Thumbnails/`-scoped preview.
 *
 * 2026-10-11 (human): opening a video starts playback on the spot. The poster is
 * a frame until it begins, never a static result the human has to press play on.
 */
export function MediaLightbox({
  id,
  root,
  onClose,
  onPrev,
  onNext,
}: {
  id: string
  root: string | null
  onClose: () => void
  onPrev?: () => void
  onNext?: () => void
}) {
  const { t } = useI18n()
  const { pushError } = useToast()
  const { data, loading, error } = useAsync<MediaDto | null>(() => mediaDetail(id), [id])
  const [videoScoped, setVideoScoped] = useState(false)
  const videoRef = useRef<HTMLVideoElement>(null)

  useEffect(() => {
    // The lightbox mounts fresh for every open (MediaScreen keys it by id), so
    // `videoScoped` starts false here; only a video ever grants.
    if (data?.fileType !== 'video') {
      return
    }
    let alive = true
    mediaScopeVideo(data.id)
      .then(() => {
        if (alive) {
          setVideoScoped(true)
        }
      })
      .catch(() => {
        // A failed grant leaves the poster/placeholder up; the detail still
        // reads. Scoping is additive and re-attempts on the next open.
      })
    return () => {
      alive = false
    }
  }, [data])

  useEffect(() => {
    if (!videoScoped || data?.fileType !== 'video') return
    const video = videoRef.current
    if (!video) return
    let alive = true
    const play = async () => {
      try {
        await video.play()
      } catch {
        if (!alive) return
        // Opening the tile is a user gesture, so the unmuted play is normally
        // allowed; if the policy still refuses, a muted play shows the frame
        // and the controls let the human unmute.
        video.muted = true
        try {
          await video.play()
        } catch {
          // Leave the poster and the controls up.
        }
      }
    }
    void play()
    return () => {
      alive = false
      video.pause()
    }
  }, [videoScoped, data])

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
          {data.fileType === 'video' && videoScoped ? (
            <video
              ref={videoRef}
              className="md-lightbox__image"
              src={mediaUrl(root, data.relativePath)}
              poster={thumbUrl(root, data.thumb400) || undefined}
              controls
              playsInline
              preload="auto"
            />
          ) : (
            <img
              src={thumbUrl(root, data.thumb400)}
              alt={data.relativePath}
              className="md-lightbox__image"
              onError={(e) => {
                ;(e.target as HTMLImageElement).style.display = 'none'
              }}
            />
          )}
        </div>
        <div className="md-lightbox__panel">
          {/* [C-4e]/[C-4g] — the arrow row of §11.4. Each control only exists
              while there is somewhere to go, so neither is ever a no-op. */}
          <div className="md-lightbox__nav">
            {onPrev ? (
              <button
                type="button"
                className="md-btn md-btn--text"
                aria-label={t('media.prev')}
                onClick={onPrev}
              >
                ←
              </button>
            ) : null}
            <h2 className="md-lightbox__title">{data.relativePath.split('/').pop()}</h2>
            {onNext ? (
              <button
                type="button"
                className="md-btn md-btn--text"
                aria-label={t('media.next')}
                onClick={onNext}
              >
                →
              </button>
            ) : null}
          </div>
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
