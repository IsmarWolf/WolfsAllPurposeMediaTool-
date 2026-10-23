import { useEffect, useCallback, useRef, useState } from 'react'
import { useI18n, useToast } from '../../hooks/useContexts'
import { useAsync } from '../../hooks/useAsync'
import { useSetFavorite } from '../../hooks/useFavorites'
import { toAppError } from '../../lib/errors'
import { mediaDetail, mediaReveal, mediaScopeVideo } from '../../lib/ipc'
import { formatBytes } from '../../lib/format'
import type { MediaDto } from '../../types/api'
import { mediaUrl, thumbUrl } from './assetUrl'

/**
 * The root-relative `Media/...` folder that contains this media, or `null` at
 * the root (nothing to hide). `vault_hide_folder` addresses a folder, not an
 * id, so `[C-4j]`'s folder sibling has to hand it the parent path (§12.5.6).
 */
function parentFolder(relativePath: string): string | null {
  const normalized = relativePath.replace(/\\/g, '/')
  const slash = normalized.lastIndexOf('/')
  return slash > 0 ? normalized.slice(0, slash) : null
}

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
  vault,
}: {
  id: string
  root: string | null
  onClose: () => void
  onPrev?: () => void
  onNext?: () => void
  /**
   * §11.6 `[C-4j Ocultar no cofre]` (+ the folder-hide sibling the human asked
   * for, 2026-10-15). The controls are rendered only when the vault is
   * configured; clicking while locked routes to the gate through the shell's
   * bridge (`vault.onHide`).
   */
  vault?: {
    configured: boolean
    onHide: (id: string) => void
    onHideFolder: (folder: string) => void
  }
}) {
  const { t } = useI18n()
  const { pushError } = useToast()
  const { data, loading, error } = useAsync<MediaDto | null>(() => mediaDetail(id), [id])
  const [videoScoped, setVideoScoped] = useState(false)
  const videoRef = useRef<HTMLVideoElement>(null)
  const setFavorite = useSetFavorite()
  // c22/§24.2 [C-4l]: `mediaDetail` is not refetched on toggle (the command
  // resolves the new count, not the row), so the button keeps a same-session
  // override over the fetched flag. A `←/→` step remounts the lightbox by id
  // and reads the fresh row anyway.
  const [favOverride, setFavOverride] = useState<boolean | null>(null)
  const favoriteOn = favOverride ?? data?.isFavorite ?? false

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

  const toggleFavorite = useCallback(async () => {
    const next = !favoriteOn
    const ok = await setFavorite([id], next)
    // Only a confirmed write flips the button: an optimistic flip on failure
    // would leave the heart lying until the next remount.
    if (ok) setFavOverride(next)
  }, [favoriteOn, id, setFavorite])

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
            {/* c22/§24.2 [C-4l] — the lightbox's own favorite toggle. Tonal so
                it never competes with the hide controls' danger role. */}
            <button
              type="button"
              className="md-btn md-btn--tonal"
              data-testid="c-4l-fav"
              aria-pressed={favoriteOn}
              onClick={() => void toggleFavorite()}
            >
              {favoriteOn ? t('media.unfavorite') : t('media.favorite')}
            </button>
            <button type="button" className="md-btn md-btn--tonal" onClick={handleReveal}>
              {t('media.openFolder')}
            </button>
            {vault?.configured ? (
              <>
                <button
                  type="button"
                  className="md-btn md-btn--tonal"
                  data-testid="c-4j-hide"
                  onClick={() => vault.onHide(data.id)}
                >
                  {t('media.hideToVault')}
                </button>
                {parentFolder(data.relativePath) ? (
                  <button
                    type="button"
                    className="md-btn md-btn--text"
                    data-testid="c-4j-hide-folder"
                    onClick={() => {
                      const folder = parentFolder(data.relativePath)
                      if (folder) vault.onHideFolder(folder)
                    }}
                  >
                    {t('media.hideFolder')}
                  </button>
                ) : null}
              </>
            ) : null}
            <button type="button" className="md-btn md-btn--text" onClick={onClose}>
              {t('common.close')}
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}
