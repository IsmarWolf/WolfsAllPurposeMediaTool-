import { useI18n } from '../../hooks/useContexts'
import type { FilterSpec } from '../../types/api'

/**
 * §10.6.1 `FilterChips` — the active-filter bar.
 *
 * Each chip shows `[Filter Label: Value] [✕ remove]`. Clear All is shown
 * ONLY when count >= 2 (C10). Chip removal is instant and re-runs the query.
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

  const chips: { label: string; remove: () => void }[] = []

  if (spec.search) {
    chips.push({
      label: `${t('media.search')}: ${spec.search}`,
      remove: () => onUpdate({ search: null }),
    })
  }
  if (spec.device) {
    chips.push({
      label: `${t('media.device')}: ${spec.device}`,
      remove: () => onUpdate({ device: null }),
    })
  }
  if (spec.month) {
    chips.push({
      label: `${t('media.month')}: ${spec.month}`,
      remove: () => onUpdate({ month: null }),
    })
  }
  if (spec.fileType) {
    chips.push({
      label: `${t('media.type')}: ${spec.fileType}`,
      remove: () => onUpdate({ fileType: null }),
    })
  }
  if (spec.noMetadata) {
    chips.push({
      label: t('media.noMetadata'),
      remove: () => onUpdate({ noMetadata: false }),
    })
  }

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
