import { Button } from '../../components/ui'
import { useI18n } from '../../hooks/useContexts'
import type { FilterSpec } from '../../types/api'

/**
 * §11.2 `[Z3g-i]` L1 filter row — search, type, sort, and the L2 drawer trigger.
 *
 * Extracted from `MediaScreen` in c16 so the **pick-media modal** (§11.5,
 * "reuses the gallery") drives exactly this row over its own spec. One filter
 * surface definition means a change to the L1 row is a change to both, which is
 * the C16 one-control-one-zone rule applied to a *shared* control rather than a
 * duplicated one.
 *
 * `drawerLabel` lets the caller name what the L2 button opens (the gallery says
 * "Filtros", a snapshot builder says the same but its footer differs).
 */
export function MediaFilterRow({
  spec,
  onUpdate,
  onOpenDrawer,
  drawerDisabled = false,
}: {
  spec: FilterSpec
  onUpdate: (patch: Partial<FilterSpec>) => void
  onOpenDrawer?: () => void
  drawerDisabled?: boolean
}) {
  const { t } = useI18n()

  return (
    <div className="l1-row" role="group" aria-label={t('media.filtersL1')}>
      <span className="l1-row__label">{t('media.filtersL1')}</span>
      <input
        type="search"
        className="md-input"
        placeholder={t('media.searchPlaceholder')}
        value={spec.search ?? ''}
        onChange={(e) => onUpdate({ search: e.target.value || null })}
        aria-label={t('media.search')}
      />
      <select
        className="md-select"
        value={spec.fileType ?? ''}
        onChange={(e) => onUpdate({ fileType: e.target.value || null })}
        aria-label={t('media.type')}
      >
        <option value="">{t('media.typeAll')}</option>
        <option value="image">{t('media.typeImage')}</option>
        <option value="video">{t('media.typeVideo')}</option>
      </select>
      <select
        className="md-select"
        value={spec.sort}
        onChange={(e) => onUpdate({ sort: e.target.value })}
        aria-label={t('media.sort')}
      >
        <option value="capturedDesc">{t('media.sortDateDesc')}</option>
        <option value="addedDesc">{t('media.sortAddedDesc')}</option>
        <option value="capturedAsc">{t('media.sortDateAsc')}</option>
        <option value="name">{t('media.sortName')}</option>
        <option value="size">{t('media.sortSize')}</option>
      </select>
      {onOpenDrawer ? (
        <Button variant="tonal" onClick={onOpenDrawer} disabled={drawerDisabled}>
          {t('media.filtersL2')}
        </Button>
      ) : null}
    </div>
  )
}
