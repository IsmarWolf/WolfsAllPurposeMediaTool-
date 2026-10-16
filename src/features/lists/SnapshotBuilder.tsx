import { useState } from 'react'
import { Button } from '../../components/ui'
import { FilterDrawer } from '../media/FilterDrawer'
import { useI18n, useToast } from '../../hooks/useContexts'
import { useListActions } from '../../hooks/useLists'
import { DEFAULT_FILTER_SPEC } from '../../hooks/useMediaQuery'
import type { FilterSpec } from '../../types/api'

/**
 * §11.5 **Snapshot builder** — "the exact filter drawer from §11.2, but its
 * footer has `[C-5i Aplicar critérios (snapshot)]`".
 *
 * Reuses `FilterDrawer` verbatim (same groups, same live-patch semantics) over a
 * **local** spec seeded from the list's stored criteria, so editing criteria
 * never touches the gallery. Applying calls `lists_snapshot_apply`, which runs
 * the spec now and replaces membership (c15 decision 4); the backend forces
 * `scope = standard`, so a snapshot can never capture vault media (C6).
 */
export function SnapshotBuilder({
  listId,
  initial,
  onClose,
}: {
  listId: string
  initial: FilterSpec | null
  onClose: () => void
}) {
  const { t } = useI18n()
  const { push } = useToast()
  const { applySnapshot } = useListActions()
  const [spec, setSpec] = useState<FilterSpec>({ ...DEFAULT_FILTER_SPEC, ...(initial ?? {}) })
  const [busy, setBusy] = useState(false)

  const apply = async () => {
    if (busy) {
      return
    }
    setBusy(true)
    const result = await applySnapshot(listId, spec)
    setBusy(false)
    if (result !== null) {
      push('success', t('lists.snapshotApplied'))
      onClose()
    }
  }

  return (
    <FilterDrawer
      open
      onClose={onClose}
      spec={spec}
      onApply={(patch) => setSpec((current) => ({ ...current, ...patch, offset: 0 }))}
      footer={
        <>
          <Button variant="text" onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="filled" onClick={apply} disabled={busy}>
            {t('lists.applyCriteria')}
          </Button>
        </>
      }
    />
  )
}
