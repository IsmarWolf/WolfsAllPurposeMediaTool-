import { useState } from 'react'
import { Button } from '../../components/ui'
import { StateRegion } from '../../components/layout/StateRegion'
import { FilterChips } from '../media/FilterChips'
import { FilterDrawer } from '../media/FilterDrawer'
import { MediaFilterRow } from '../media/MediaFilterRow'
import { MediaGrid } from '../media/MediaGrid'
import { useI18n, useToast } from '../../hooks/useContexts'
import { useAsync } from '../../hooks/useAsync'
import { useListActions } from '../../hooks/useLists'
import { useMediaQuery } from '../../hooks/useMediaQuery'
import { useSelection } from '../../hooks/useSelection'
import { resolveAppRoot } from '../../lib/ipc'
import type { RootInfoDto } from '../../types/api'

/**
 * §11.5 **Pick-media modal** — "reuses the gallery with multi-select".
 *
 * It is the real gallery: `useMediaQuery` (same L1/L2 filters and paging),
 * `MediaGrid` (same tiles, keyboard model and hover upgrade) and its **own**
 * `useSelection`, so picking never disturbs the gallery's selection behind the
 * modal. `[C-5j Adicionar selecionados]` calls `lists_add_items` (manual lists
 * only — a smart list refuses, §7.9).
 */
export function PickMediaModal({ listId, onClose }: { listId: string; onClose: () => void }) {
  const { t } = useI18n()
  const { push } = useToast()
  const query = useMediaQuery()
  const selection = useSelection()
  const { addItems } = useListActions()
  const [drawerOpen, setDrawerOpen] = useState(false)
  const [busy, setBusy] = useState(false)

  const rootInfo = useAsync<RootInfoDto>(() => resolveAppRoot(), [])
  const root = rootInfo.data?.root ?? null

  const items = query.data?.items ?? []
  const count = selection.count

  const add = async () => {
    if (count === 0 || busy) {
      return
    }
    setBusy(true)
    const result = await addItems(listId, [...selection.selected])
    setBusy(false)
    if (result !== null) {
      push('success', t('lists.added', { count }))
      onClose()
    }
  }

  return (
    <div
      className="md-modal md-modal--wide"
      role="dialog"
      aria-modal="true"
      aria-label={t('lists.pickMedia')}
    >
      <div className="md-modal__backdrop" onClick={onClose} />
      <div className="md-modal__panel">
        <div className="md-drawer__header">
          <h2>{t('lists.pickMedia')}</h2>
          <button type="button" className="md-btn md-btn--text" onClick={onClose}>
            {t('common.close')}
          </button>
        </div>

        <MediaFilterRow
          spec={query.spec}
          onUpdate={query.updateSpec}
          onOpenDrawer={() => setDrawerOpen(true)}
        />
        <FilterChips spec={query.spec} onClear={query.clearFilters} onUpdate={query.updateSpec} />

        <div className="lists__pick-media-grid" data-testid="lists-pick-media-grid">
          <StateRegion
            stage={{
              loading: query.loading,
              error: query.error,
              data: query.data,
              count: items.length,
              filtered: query.filtered,
            }}
            ideal={
              <MediaGrid
                items={items}
                root={root}
                onOpen={() => {}}
                selectionMode
                selection={selection}
                navEnabled
              />
            }
          />
        </div>

        {query.data?.hasMore ? (
          <Button
            variant="text"
            onClick={query.loadMore}
            disabled={query.loading || query.loadingMore}
          >
            {t('media.loadMore')}
          </Button>
        ) : null}

        <div className="md-modal__footer">
          <span className="lists__pick-count">{t('lists.selectedCount', { count })}</span>
          <Button variant="filled" onClick={add} disabled={busy || count === 0}>
            {t('lists.addSelected')}
          </Button>
        </div>

        <FilterDrawer
          open={drawerOpen}
          onClose={() => setDrawerOpen(false)}
          spec={query.spec}
          onApply={query.updateSpec}
        />
      </div>
    </div>
  )
}
