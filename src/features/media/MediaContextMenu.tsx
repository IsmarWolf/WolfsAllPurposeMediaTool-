import { useEffect, useRef, useState } from 'react'
import { useI18n, useToast } from '../../hooks/useContexts'
import { toAppError } from '../../lib/errors'
import { mediaReveal } from '../../lib/ipc'

/** Where a right-click landed: the tile id plus the viewport point to open at. */
export interface ContextTarget {
  id: string
  x: number
  y: number
}

const VIEWPORT_MARGIN = 8

/**
 * §10.6.2 / mockup `#ctxmenu` — the tile context menu (right-click).
 *
 * c19 (2026-10-17, human): the frozen mockup has a right-click menu that the app
 * was missing, and the human asked for "Ocultar no cofre" to be reachable from
 * it. Only actions that actually work today are listed — Rename / Duplicate /
 * Export are v1.1 and would be dead controls: "Ver metadados" opens the lightbox,
 * "Abrir local do arquivo" is `media_reveal`, and "Ocultar no cofre" appears only
 * when the vault is configured (the mockup's own `if (S.vault.configured)`).
 */
export function MediaContextMenu({
  target,
  vaultConfigured,
  isFavorite = false,
  onOpen,
  onHide,
  onToggleFavorite,
  onClose,
}: {
  target: ContextTarget
  vaultConfigured: boolean
  /** c22/§24.2: the item's current flag, so the menu can say Favoritar vs Desfavoritar. */
  isFavorite?: boolean
  onOpen: (id: string) => void
  onHide: (id: string) => void
  /** Absent → the favorite item is not rendered (dead-control rule, as ever). */
  onToggleFavorite?: (id: string) => void
  onClose: () => void
}) {
  const { t } = useI18n()
  const { pushError } = useToast()
  const ref = useRef<HTMLDivElement>(null)
  const [pos, setPos] = useState({ left: target.x, top: target.y })

  // Clamp inside the viewport once the menu has measured itself, so a click near
  // the right/bottom edge never opens a menu half off-screen.
  useEffect(() => {
    const el = ref.current
    if (!el) return
    const rect = el.getBoundingClientRect()
    setPos({
      left: Math.max(
        VIEWPORT_MARGIN,
        Math.min(target.x, window.innerWidth - rect.width - VIEWPORT_MARGIN),
      ),
      top: Math.max(
        VIEWPORT_MARGIN,
        Math.min(target.y, window.innerHeight - rect.height - VIEWPORT_MARGIN),
      ),
    })
  }, [target.x, target.y])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    const onPointer = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose()
    }
    window.addEventListener('keydown', onKey)
    window.addEventListener('mousedown', onPointer)
    // `true` so a scroll anywhere (the virtual grid's own scroller included)
    // closes it rather than leaving the menu pinned to a stale point.
    window.addEventListener('scroll', onClose, true)
    return () => {
      window.removeEventListener('keydown', onKey)
      window.removeEventListener('mousedown', onPointer)
      window.removeEventListener('scroll', onClose, true)
    }
  }, [onClose])

  const reveal = async () => {
    try {
      await mediaReveal(target.id)
    } catch (raw) {
      pushError(toAppError(raw))
    }
  }

  return (
    <div
      ref={ref}
      className="md-ctxmenu"
      role="menu"
      data-testid="media-context-menu"
      style={{ left: pos.left, top: pos.top }}
      onContextMenu={(e) => e.preventDefault()}
    >
      <button
        type="button"
        role="menuitem"
        className="md-ctxmenu__item"
        onClick={() => {
          onClose()
          onOpen(target.id)
        }}
      >
        {t('media.viewMetadata')}
      </button>
      <button
        type="button"
        role="menuitem"
        className="md-ctxmenu__item"
        onClick={() => {
          onClose()
          void reveal()
        }}
      >
        {t('media.openFolder')}
      </button>
      {/* c22/§24.2 — Favoritar / Desfavoritar, mirroring the mockup's star. */}
      {onToggleFavorite ? (
        <button
          type="button"
          role="menuitem"
          className="md-ctxmenu__item"
          data-testid="ctx-favorite"
          onClick={() => {
            onClose()
            onToggleFavorite(target.id)
          }}
        >
          {isFavorite ? t('media.unfavorite') : t('media.favorite')}
        </button>
      ) : null}
      {vaultConfigured ? (
        <button
          type="button"
          role="menuitem"
          className="md-ctxmenu__item"
          data-testid="ctx-hide"
          onClick={() => {
            onClose()
            onHide(target.id)
          }}
        >
          {t('media.hideToVault')}
        </button>
      ) : null}
    </div>
  )
}
