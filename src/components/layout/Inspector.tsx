import { useI18n } from '../../hooks/useContexts'
import { SkeletonBlock } from '../ui'

/**
 * §10.6.3 `InspectorPanel` — the Zone 4 shell with its three render modes,
 * strictly driven by the selection count. It collapses (width 0) on screens with
 * no selection model, and it is never empty chrome.
 */
export function Inspector({
  mode,
  count,
  onClose,
}: {
  mode: 'summary' | 'single' | 'bulk' | 'none'
  count: number
  onClose: () => void
}) {
  const { t } = useI18n()

  if (mode === 'none') {
    return null
  }

  return (
    <aside className="zone4" aria-label={t('zone.inspector')}>
      <div className="zone4__header">
        <h2>{t(`inspector.${mode}`)}</h2>
        {/* A close control only when it closes something (human-confirmed
            2026-09-28): the X clears the selection, so it can only appear where
            there *is* a selection. In `summary` it was a button wired to a
            no-op that read as "closes Resumo". The panel itself has no manual
            collapse yet - that is the open Zone 4 decision for c8. */}
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
          <p className="zone4__bulk-count">{t('inspector.bulkCount', { count: String(count) })}</p>
          <SkeletonBlock height={12} width="80%" />
          <p className="zone4__hint">{t('inspector.bulkComing')}</p>
        </>
      ) : null}
    </aside>
  )
}
