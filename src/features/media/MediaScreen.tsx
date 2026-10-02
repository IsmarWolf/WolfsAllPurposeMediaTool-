import { useState, useCallback } from 'react'
import { Button, PageHeader } from '../../components/ui'
import { StateRegion } from '../../components/layout/StateRegion'
import { useI18n } from '../../hooks/useContexts'
import { useAsync } from '../../hooks/useAsync'
import { useMediaQuery } from '../../hooks/useMediaQuery'
import { useSelection } from '../../hooks/useSelection'
import { formatCount } from '../../lib/format'
import { resolveAppRoot } from '../../lib/ipc'
import type { RootInfoDto } from '../../types/api'
import { MediaGrid } from './MediaGrid'
import { MediaLightbox } from './MediaLightbox'
import { FilterDrawer } from './FilterDrawer'
import { FilterChips } from './FilterChips'

/**
 * §11.2 Mídia — the gallery screen.
 *
 * The page owns its own filter surface (L1 row + L2 drawer + chips, §10.6.1/C16):
 * the shell header carries none. The grid is virtualized with LQIP tiles
 * (§10.6.5); the five-state machine (§10.6.4) drives the render. Selection is
 * the shell's (§12.6) so the Inspector + bulk bar follow the gallery.
 */
export function MediaScreen({ selection }: { selection: ReturnType<typeof useSelection> }) {
  const { t } = useI18n()
  const query = useMediaQuery()
  // c10: the asset scope is backend-side, but the URL needs the absolute root —
  // `RootInfoDto.root` is the one absolute path the frontend may hold (§22).
  const rootInfo = useAsync<RootInfoDto>(() => resolveAppRoot(), [])
  const root = rootInfo.data?.root ?? null
  const [lightboxId, setLightboxId] = useState<string | null>(null)
  const [drawerOpen, setDrawerOpen] = useState(false)
  const [selectionMode, setSelectionMode] = useState(false)

  const items = query.data?.items ?? []
  const total = query.data?.total ?? 0

  const openLightbox = useCallback((id: string) => setLightboxId(id), [])
  const closeLightbox = useCallback(() => setLightboxId(null), [])

  return (
    <div className="screen">
      <PageHeader
        title={t('nav.media')}
        subtitle={t('media.subtitle')}
        actions={
          <>
            <span className="md-badge" data-testid="media-total">
              {formatCount(total)}
            </span>
            <Button variant="tonal" onClick={() => setSelectionMode((v) => !v)}>
              {t('media.selectionMode')}
            </Button>
          </>
        }
      />

      {/* [Z3g-i] L1 filter row — a Mídia concern, never a Zone 1 one. */}
      <div className="l1-row" role="group" aria-label={t('media.filtersL1')}>
        <span className="l1-row__label">{t('media.filtersL1')}</span>
        <input
          type="search"
          className="md-input"
          placeholder={t('media.searchPlaceholder')}
          value={query.spec.search ?? ''}
          onChange={(e) => query.updateSpec({ search: e.target.value || null })}
          aria-label={t('media.search')}
        />
        <select
          className="md-select"
          value={query.spec.fileType ?? ''}
          onChange={(e) => query.updateSpec({ fileType: e.target.value || null })}
          aria-label={t('media.type')}
        >
          <option value="">{t('media.typeAll')}</option>
          <option value="image">{t('media.typeImage')}</option>
          <option value="video">{t('media.typeVideo')}</option>
        </select>
        <select
          className="md-select"
          value={query.spec.sort}
          onChange={(e) => query.updateSpec({ sort: e.target.value })}
          aria-label={t('media.sort')}
        >
          <option value="capturedDesc">{t('media.sortDateDesc')}</option>
          <option value="capturedAsc">{t('media.sortDateAsc')}</option>
          <option value="name">{t('media.sortName')}</option>
          <option value="size">{t('media.sortSize')}</option>
        </select>
        {/* [Z3j] L2 drawer button. */}
        <Button variant="tonal" onClick={() => setDrawerOpen(true)}>
          {t('media.filtersL2')}
        </Button>
      </div>

      {/* [Z3k/l] State chips — hidden when count = 0. */}
      <FilterChips spec={query.spec} onClear={query.clearFilters} onUpdate={query.updateSpec} />

      {/* State region — the five-state machine (§10.6.4). */}
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
            onOpen={openLightbox}
            selectionMode={selectionMode}
            selection={selection}
          />
        }
      />

      {/* [Z3f] Load more — auto page-in at 80% of the virtual window. */}
      {query.data?.hasMore ? (
        <Button variant="text" onClick={query.loadMore} disabled={query.loading}>
          {t('media.loadMore')}
        </Button>
      ) : null}

      {/* Lightbox (§11.4). */}
      {lightboxId ? <MediaLightbox id={lightboxId} root={root} onClose={closeLightbox} /> : null}

      {/* L2 filter drawer (§10.6.1). */}
      <FilterDrawer
        open={drawerOpen}
        onClose={() => setDrawerOpen(false)}
        spec={query.spec}
        onApply={query.updateSpec}
      />
    </div>
  )
}
