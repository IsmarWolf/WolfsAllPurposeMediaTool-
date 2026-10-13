import { useI18n } from '../../hooks/useContexts'
import { useCityCounts } from '../../hooks/useCityCounts'
import { formatCount } from '../../lib/format'
import type { FilterSpec } from '../../types/api'

/**
 * §10.6.1 `FilterDrawer` — the L2 filter drawer.
 *
 * Groups: Data (month), Localização (city tag-cloud), Dispositivo, Estado
 * (sem metadados). Apply mutates the shared FilterSpec; chips live under the
 * filter row. The drawer is rendered only when toggled (lazy), so the city
 * counts below are not fetched until it opens.
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
          <div className="md-drawer__section-head">
            <h3>{t('media.filterCity')}</h3>
            {spec.cities.length > 0 ? (
              <button
                type="button"
                className="md-btn md-btn--text"
                onClick={() => onApply({ cities: [] })}
              >
                {t('media.filterClear')}
              </button>
            ) : null}
          </div>
          <CityFacet spec={spec} onApply={onApply} />
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

/**
 * The `Localização` tag-cloud: one toggle per city, count-sorted, driven by the
 * real `city_counts` read (§7.2). Selecting a city replaces nothing — it adds
 * to `spec.cities`, so the backend's `LOWER(l.city) IN (...)` matches any of
 * them; the chip bar removes them one at a time.
 */
function CityFacet({
  spec,
  onApply,
}: {
  spec: FilterSpec
  onApply: (patch: Partial<FilterSpec>) => void
}) {
  const { t } = useI18n()
  const { cities } = useCityCounts(spec.scope)

  if (cities.length === 0) {
    return <p className="md-tagcloud__empty">{t('media.filterCityEmpty')}</p>
  }

  const selected = new Set(spec.cities)
  const toggle = (city: string) => {
    const next = selected.has(city) ? spec.cities.filter((c) => c !== city) : [...spec.cities, city]
    onApply({ cities: next })
  }

  return (
    <div className="md-tagcloud">
      {cities.map((node) => (
        <button
          key={node.city}
          type="button"
          className={selected.has(node.city) ? 'md-chip md-chip--active' : 'md-chip'}
          aria-pressed={selected.has(node.city)}
          aria-label={`${t('media.filterCity')}: ${node.city}`}
          onClick={() => toggle(node.city)}
        >
          {node.city}
          <span className="md-tagcloud__count">{formatCount(node.count)}</span>
        </button>
      ))}
    </div>
  )
}
