import { useI18n } from '../../hooks/useContexts'

/**
 * §11.9 `BulkToolbar` — the Zone 5 floating bar, visible only when
 * `selection >= 1`. Primary actions on the left, the isolated destructive red on
 * the right (§10.6.2).
 *
 * The actions are c8+; c4 renders the bar with its count so the shell
 * contract (appears/disappears with the selection) is real, not a mock.
 */
export function BulkToolbar({ count, onClear }: { count: number; onClear: () => void }) {
  const { t } = useI18n()

  if (count < 1) {
    return null
  }

  return (
    <div className="zone5" role="toolbar" aria-label={t('bulk.label')}>
      <span className="zone5__count">{t('bulk.count', { count: String(count) })}</span>
      <button type="button" className="md-btn md-btn--text" onClick={onClear}>
        {t('bulk.clear')}
      </button>
      <span className="zone5__spacer" />
      <button type="button" className="md-btn md-btn--text md-btn--danger">
        {t('bulk.delete')}
      </button>
    </div>
  )
}
