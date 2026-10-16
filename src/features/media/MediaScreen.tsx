import { useState, useCallback, useMemo } from 'react'
import { Button, PageHeader } from '../../components/ui'
import { StateRegion } from '../../components/layout/StateRegion'
import { useI18n } from '../../hooks/useContexts'
import { useAsync } from '../../hooks/useAsync'
import { useMediaQuery } from '../../hooks/useMediaQuery'
import { useSelection } from '../../hooks/useSelection'
import { useShortcuts, type Shortcut } from '../../hooks/useShortcuts'
import { formatCount } from '../../lib/format'
import { resolveAppRoot } from '../../lib/ipc'
import type { RootInfoDto } from '../../types/api'
import { MediaGrid } from './MediaGrid'
import { MediaLightbox } from './MediaLightbox'
import { FilterDrawer } from './FilterDrawer'
import { FilterChips } from './FilterChips'

/**
 * §12.6's guard, reused by the two bindings whose owner may also handle the key
 * itself (a focused tile's Enter/Space, a focused button's Space).
 */
function focusedInteractive(): boolean {
  const active = document.activeElement as HTMLElement | null
  return Boolean(active?.closest('button, a, input, select, textarea, [role="button"]'))
}

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
  // The lightbox tracks a *position* in the page, not just an id, so `←/→`
  // (§11.4) have somewhere to step. A stale index (the item vanished under a
  // db-changed re-query) resolves to `null` and closes the overlay by itself.
  const [lightboxIndex, setLightboxIndex] = useState<number | null>(null)
  const [drawerOpen, setDrawerOpen] = useState(false)
  const [selectionMode, setSelectionMode] = useState(false)

  const items = query.data?.items ?? []
  const total = query.data?.total ?? 0

  const lightboxId = lightboxIndex !== null ? (items[lightboxIndex]?.id ?? null) : null

  const openLightbox = useCallback(
    (id: string) => {
      const index = items.findIndex((item) => item.id === id)
      if (index >= 0) setLightboxIndex(index)
    },
    [items],
  )
  const closeLightbox = useCallback(() => setLightboxIndex(null), [])

  // [C-4e]/[C-4g] — absent at the ends of the page, so neither arrow is ever a
  // control that does nothing.
  const prevItem =
    lightboxIndex !== null && lightboxIndex > 0
      ? () => setLightboxIndex((i) => (i === null || i <= 0 ? i : i - 1))
      : undefined
  const nextItem =
    lightboxIndex !== null && lightboxIndex < items.length - 1
      ? () => setLightboxIndex((i) => (i === null ? i : Math.min(i + 1, items.length - 1)))
      : undefined

  // The view-scoped half of §12.6's map — the half that needs this page's items.
  // The shell keeps only `Esc` (inspector drawer / selection); everything below
  // is about the gallery's own contents. The click map, the tile-level
  // Enter/Space and the arrow cursor live in `MediaGrid`.
  const shortcuts = useMemo<Shortcut[]>(
    () => [
      // "Select all in the current spec" — and Ctrl/Cmd+A again clears it
      // (Windows Explorer toggle, human 2026-10-15). The cap lives in
      // `useSelection`.
      { key: 'a', ctrl: true, run: () => selection.selectAll(items.map((item) => item.id)) },
      {
        // With a tile focused this is the tile's own Space (toggle it); the
        // `role="button"` guard below leaves it alone there. Without one, the
        // anchor is the cursor, so Space still toggles something.
        key: ' ',
        run: () => {
          if (focusedInteractive()) return
          if (selection.anchor) selection.toggle(selection.anchor)
        },
      },
      {
        // Explorer's open key. A focused tile handles Enter itself (same guard).
        key: 'enter',
        run: () => {
          if (focusedInteractive()) return
          const target = selection.anchor ?? items[0]?.id
          if (target) openLightbox(target)
        },
      },
      // §11.4: `←/→` step along the open lightbox. Guards make both ends a stop.
      // (With no lightbox up these no-op; the grid's own cursor handles them.)
      {
        key: 'arrowleft',
        run: () => {
          if (lightboxIndex === null) return
          setLightboxIndex((i) => (i === null || i <= 0 ? i : i - 1))
        },
      },
      {
        key: 'arrowright',
        run: () => {
          if (lightboxIndex === null) return
          setLightboxIndex((i) => (i === null ? i : Math.min(i + 1, items.length - 1)))
        },
      },
      // The two overlays this screen owns; the shell's `Esc` clears selection or
      // closes the inspector drawer alongside it (§10.6.5).
      {
        key: 'escape',
        run: () => {
          if (lightboxId !== null) {
            closeLightbox()
          } else if (drawerOpen) {
            setDrawerOpen(false)
          }
        },
      },
    ],
    [items, selection, openLightbox, lightboxIndex, lightboxId, drawerOpen, closeLightbox],
  )
  useShortcuts(shortcuts)

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
          <option value="addedDesc">{t('media.sortAddedDesc')}</option>
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
            navEnabled={lightboxId === null}
          />
        }
      />

      {/* [Z3f] Load more — auto page-in at 80% of the virtual window. */}
      {query.data?.hasMore ? (
        <Button
          variant="text"
          onClick={query.loadMore}
          disabled={query.loading || query.loadingMore}
        >
          {t('media.loadMore')}
        </Button>
      ) : null}

      {/* Lightbox (§11.4). Keyed by id so a `←/→` step remounts it — the video
          scope grant is per file and must not be inherited by the next one. */}
      {lightboxId ? (
        <MediaLightbox
          key={lightboxId}
          id={lightboxId}
          root={root}
          onClose={closeLightbox}
          onPrev={prevItem}
          onNext={nextItem}
        />
      ) : null}

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
