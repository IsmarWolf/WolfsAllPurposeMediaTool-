import { useI18n } from '../../hooks/useContexts'
import type { FilterSpec } from '../../types/api'

/**
 * §10.6.1 `FilterDrawer` — the L2 filter drawer.
 *
 * Accordion groups: Data (month), Localização (city), Dispositivo, Estado
 * (sem metadados). Apply mutates the shared FilterSpec; chips live under the
 * filter row. The drawer is rendered only when toggled (lazy).
 */
export function FilterDrawer({
  open,
  onClose,
  spec,
  onApply,
}: {
  open: boolean
  onClose: () => void
  spec: FilterSpec
  onApply: (patch: Partial<FilterSpec>) => void
}) {
  const { t } = useI18n()

  if (!open) return null

  return (
    <div className="md-drawer" role="dialog" aria-modal="true" aria-label={t('media.filtersL2')}>
      <div className="md-drawer__backdrop" onClick={onClose} />
      <div className="md-drawer__panel">
        <div className="md-drawer__header">
          <h2>{t('media.filtersL2')}</h2>
          <button type="button" className="md-btn md-btn--text" onClick={onClose}>
            {t('common.close')}
          </button>
        </div>

        <div className="md-drawer__section">
          <h3>{t('media.filterDate')}</h3>
          <input
            type="month"
            className="md-input"
            value={spec.month ?? ''}
            onChange={(e) => onApply({ month: e.target.value || null })}
            aria-label={t('media.filterDate')}
          />
        </div>

        <div className="md-drawer__section">
          <h3>{t('media.filterDevice')}</h3>
          <input
            type="text"
            className="md-input"
            placeholder={t('media.filterDevicePlaceholder')}
            value={spec.device ?? ''}
            onChange={(e) => onApply({ device: e.target.value || null })}
            aria-label={t('media.filterDevice')}
          />
        </div>

        <div className="md-drawer__section">
          <h3>{t('media.filterState')}</h3>
          <label className="md-checkbox">
            <input
              type="checkbox"
              checked={spec.noMetadata}
              onChange={(e) => onApply({ noMetadata: e.target.checked })}
            />
            {t('media.filterNoMetadata')}
          </label>
        </div>
      </div>
    </div>
  )
}
