import { useI18n } from '../../hooks/useContexts'
import type { FilterSpec } from '../../types/api'
import { appliedFilters, chipLabel, type AppliedFilter } from './applied'

/**
 * §10.6.1 `FilterChips` — the active-filter bar.
 *
 * Each chip shows `[Filter Label: Value] [✕ remove]`. Clear All is shown
 * ONLY when count >= 2 (C10). Chip removal is instant and re-runs the query.
 *
 * The list of active filters comes from `appliedFilters` — the same helper the
 * smart-list criteria summary reads — so the chips and a snapshot can never
 * describe one spec two ways.
 */
export function FilterChips({
  spec,
  onClear,
  onUpdate,
}: {
  spec: FilterSpec
  onClear: () => void
  onUpdate: (patch: Partial<FilterSpec>) => void
}) {
  const { t } = useI18n()

  const remove = (filter: AppliedFilter) => {
    switch (filter.field) {
      case 'search':
        onUpdate({ search: null })
        break
      case 'device':
        onUpdate({ device: null })
        break
      case 'city':
        onUpdate({ cities: spec.cities.filter((c) => c !== filter.value) })
        break
      case 'month':
        onUpdate({ month: null })
        break
      case 'fileType':
        onUpdate({ fileType: null })
        break
      case 'noMetadata':
        onUpdate({ noMetadata: false })
        break
    }
  }

  const chips = appliedFilters(spec).map((filter) => ({
    label: chipLabel(filter, t),
    remove: () => remove(filter),
  }))

  if (chips.length === 0) return null

  return (
    <div className="media-chips" role="group" aria-label={t('media.activeFilters')}>
      {chips.map((chip) => (
        <span key={chip.label} className="md-chip md-chip--active media-chips__chip">
          {chip.label}
          <button
            type="button"
            className="media-chips__remove"
            aria-label={`${t('common.remove')}: ${chip.label}`}
            onClick={chip.remove}
          >
            ×
          </button>
        </span>
      ))}
      {chips.length >= 2 ? (
        <button type="button" className="md-btn md-btn--text" onClick={onClear}>
          {t('media.clearAll')}
        </button>
      ) : null}
    </div>
  )
}
