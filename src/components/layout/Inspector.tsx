import { useEffect, useId, useRef } from 'react'
import { useI18n } from '../../hooks/useContexts'
import { SkeletonBlock } from '../ui'

/**
 * §10.6.3 `InspectorPanel` — the Zone 4 shell with its three render modes,
 * strictly driven by the selection count. It collapses (width 0) on screens with
 * no selection model, and it is never empty chrome.
 *
 * Below 1280px the panel becomes an **overlay drawer** (human-confirmed
 * 2026-09-29) instead of vanishing with no way back: the window is not wide enough
 * for the three columns, but the inspector still has to be reachable. The
 * decision about *which* size class applies stays in `shell.css` — this component
 * only carries the state, the toggle and the drawer semantics, so the panel is a
 * plain grid column again as soon as the window is wide enough (§10.6.3's rule
 * that the size classes are not measured in JS).
 */
export function Inspector({
  mode,
  count,
  onClose,
  drawerOpen = false,
  onDrawerChange,
}: {
  mode: 'summary' | 'single' | 'bulk' | 'none'
  count: number
  onClose: () => void
  /** Narrow-window drawer state, owned by the shell (it owns the Esc order). */
  drawerOpen?: boolean
  onDrawerChange?: (open: boolean) => void
}) {
  const { t } = useI18n()
  const panelId = useId()
  const panelRef = useRef<HTMLElement | null>(null)
  const toggleRef = useRef<HTMLButtonElement | null>(null)
  // Restoring focus only matters when the drawer actually took it, and on a wide
  // window the toggle does not even exist.
  const tookFocus = useRef(false)

  const setDrawer = onDrawerChange ?? (() => {})

  useEffect(() => {
    if (!drawerOpen || mode === 'none') {
      return
    }
    panelRef.current?.focus()
    tookFocus.current = true
  }, [drawerOpen, mode])

  useEffect(() => {
    if (drawerOpen || !tookFocus.current) {
      return
    }
    tookFocus.current = false
    toggleRef.current?.focus()
  }, [drawerOpen])

  if (mode === 'none') {
    return null
  }

  return (
    <>
      {/* The gutter tab: it exists for the narrow window only, and `shell.css`
          hides it from 1280px up, where the panel is already on screen. */}
      <button
        ref={toggleRef}
        type="button"
        className="zone4__toggle"
        aria-expanded={drawerOpen}
        aria-controls={panelId}
        aria-label={drawerOpen ? t('inspector.closeDrawer') : t('inspector.open')}
        // `summary` is the dashboard's totals: there is nothing to inspect yet,
        // and a tab that opens a panel with no data is a dead control (the same
        // rule the close button follows).
        disabled={mode === 'summary'}
        onClick={() => setDrawer(!drawerOpen)}
      >
        <span aria-hidden="true">›</span>
      </button>

      {drawerOpen ? (
        <div
          className="zone4__scrim"
          // The scrim is decoration: the Escape key and the toggle are the real
          // ways out, so it is hidden from assistive tech and not focusable.
          aria-hidden="true"
          onClick={() => setDrawer(false)}
        />
      ) : null}

      <aside
        id={panelId}
        ref={panelRef}
        className={`zone4${drawerOpen ? ' zone4--drawer-open' : ''}`}
        aria-label={t('zone.inspector')}
        // Only a drawer is a dialog: inline (>= 1280px) it is a complementary
        // region, and `role="dialog"` there would trap focus for no reason.
        {...(drawerOpen ? { role: 'dialog', 'aria-modal': true, tabIndex: -1 } : {})}
      >
        <div className="zone4__header">
          <h2>{t(`inspector.${mode}`)}</h2>
          {/* A close control only when it closes something (human-confirmed
              2026-09-28): the X clears the selection, so it can only appear where
              there *is* a selection. In `summary` it was a button wired to a
              no-op that read as "closes Resumo". The drawer's own way out is the
              gutter tab (and Escape, in AppShell). */}
          {mode === 'summary' ? null : (
            <button
              type="button"
              className="md-btn md-btn--text"
              onClick={onClose}
              aria-label={t('common.close')}
            >
              ×
            </button>
          )}
        </div>

        {mode === 'summary' ? (
          // c4 skeleton: the totals arrive in c8, with the first query that has
          // something to show. It says so instead of pretending to be empty.
          <>
            <SkeletonBlock height={12} width="60%" />
            <SkeletonBlock height={12} width="40%" />
            <p className="zone4__hint">{t('inspector.selectToEdit')}</p>
          </>
        ) : null}

        {mode === 'single' ? (
          <>
            <SkeletonBlock height={160} />
            <SkeletonBlock height={12} width="70%" />
            <p className="zone4__hint">{t('inspector.singleComing')}</p>
          </>
        ) : null}

        {mode === 'bulk' ? (
          <>
            <p className="zone4__bulk-count">
              {t('inspector.bulkCount', { count: String(count) })}
            </p>
            <SkeletonBlock height={12} width="80%" />
            <p className="zone4__hint">{t('inspector.bulkComing')}</p>
          </>
        ) : null}
      </aside>
    </>
  )
}
