import {
  useState,
  useCallback,
  useEffect,
  useMemo,
  type MouseEvent as ReactMouseEvent,
} from 'react'
import { Button, PageHeader } from '../../components/ui'
import { StateRegion } from '../../components/layout/StateRegion'
import { useI18n } from '../../hooks/useContexts'
import { useAsync } from '../../hooks/useAsync'
import { useSetFavorite } from '../../hooks/useFavorites'
import { useMediaQuery } from '../../hooks/useMediaQuery'
import { useSelection } from '../../hooks/useSelection'
import { useShortcuts, type Shortcut } from '../../hooks/useShortcuts'
import { formatCount } from '../../lib/format'
import { resolveAppRoot } from '../../lib/ipc'
import type { MediaDto, RootInfoDto } from '../../types/api'
import { MediaGrid } from './MediaGrid'
import { MediaContextMenu, type ContextTarget } from './MediaContextMenu'
import { MediaLightbox } from './MediaLightbox'
import { FilterDrawer } from './FilterDrawer'
import { FilterChips } from './FilterChips'
import { MediaFilterRow } from './MediaFilterRow'
import type { OpenCollection } from '../collections'

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
 *
 * c22: when the shell mounts it with a `collection` (§11.10 C-8c), the query
 * runs `collection_open` instead of `media_query`, the header carries the
 * collection's name + a back control, and `[Z3d]` adds the *selection* (or the
 * whole page, with no selection) to a manual list — the §12.4 rewrite of the
 * retired snapshot flow.
 */
export function MediaScreen({
  selection,
  vault,
  collection = null,
  onExitCollection,
  onRequestAddToList,
  onFavoriteInfo,
}: {
  selection: ReturnType<typeof useSelection>
  /** §11.6 hide bridge from the shell: `[C-4j]` and the folder-hide action. */
  vault: {
    configured: boolean
    onHide: (id: string) => void
    onHideFolder: (folder: string) => void
  }
  /** c22/§11.10: the open collection, or null for the plain gallery. */
  collection?: OpenCollection | null
  onExitCollection?: () => void
  /** c22/§12.4: hands the shell the ids to add (selection, or the whole page). */
  onRequestAddToList?: (ids: string[]) => void
  /**
   * c22/§24.2: reports the selection's favorite aggregate to the shell, which
   * feeds `[Z4h]`/`[Z5f]`. `null` = no selection (or a selection whose ids are
   * not all on the current page — the flag cannot be known without N reads).
   */
  onFavoriteInfo?: (info: 'on' | 'off' | 'mixed' | null) => void
}) {
  const { t } = useI18n()
  const query = useMediaQuery(undefined, collection?.key ?? null)
  const setFavorite = useSetFavorite()
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
  // c19: the tile context menu (§10.6.2, mockup `#ctxmenu`). Closed by Esc /
  // scroll / a click outside, and before any of its actions run.
  const [ctx, setCtx] = useState<ContextTarget | null>(null)

  // `?? []` is a fresh array every render; memoized so the hooks below
  // (`selectionFavorite`, `shortcuts`, `openLightbox`) do not recompute on
  // renders that only flipped a boolean.
  const items = useMemo(() => query.data?.items ?? [], [query.data])
  const total = query.data?.total ?? 0

  // c22/§24.2: the selection's favorite aggregate. Only derivable when every
  // selected id is on the loaded page (a selection can outlive a page turn);
  // otherwise the shell gets `null` and hides the flag controls.
  const selectionFavorite = useMemo<'on' | 'off' | 'mixed' | null>(() => {
    if (selection.count === 0) return null
    const picked = items.filter((item) => selection.selected.has(item.id))
    if (picked.length !== selection.count) return null
    if (picked.every((item) => item.isFavorite)) return 'on'
    if (picked.every((item) => !item.isFavorite)) return 'off'
    return 'mixed'
  }, [items, selection.selected, selection.count])
  useEffect(() => {
    onFavoriteInfo?.(selectionFavorite)
  }, [selectionFavorite, onFavoriteInfo])

  const toggleFavorite = useCallback(
    (item: MediaDto) => {
      void setFavorite([item.id], !item.isFavorite)
    },
    [setFavorite],
  )

  // c22/§12.4: `[Z3d]` hands the shell what to add — the selection when there
  // is one, else the whole loaded page (the §12.4 flow the human confirmed:
  // "add what I am looking at").
  const requestAddToList = useCallback(() => {
    const ids = selection.count > 0 ? [...selection.selected] : items.map((item) => item.id)
    if (ids.length === 0) return
    onRequestAddToList?.(ids)
  }, [selection, items, onRequestAddToList])

  const lightboxId = lightboxIndex !== null ? (items[lightboxIndex]?.id ?? null) : null

  const openLightbox = useCallback(
    (id: string) => {
      const index = items.findIndex((item) => item.id === id)
      if (index >= 0) setLightboxIndex(index)
    },
    [items],
  )
  const closeLightbox = useCallback(() => setLightboxIndex(null), [])

  const openContext = useCallback((item: MediaDto, event: ReactMouseEvent) => {
    setCtx({ id: item.id, x: event.clientX, y: event.clientY })
  }, [])
  const closeContext = useCallback(() => setCtx(null), [])

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
      {/* c22/§11.10 [C-8d]: inside a collection the back control returns to the
          card grid; the route is the shell's to move. Its own left-aligned row
          above the title (Google Photos puts back top-left, never top-right),
          and the arrow glyph is CSS (`collection-back::before`) so the label
          stays the exact "Voltar" the test pins. */}
      {collection && onExitCollection ? (
        <div className="collection-back-row">
          <Button variant="text" data-testid="collection-back" onClick={onExitCollection}>
            {t('common.back')}
          </Button>
        </div>
      ) : null}
      <PageHeader
        title={collection ? t(collection.labelKey) : t('nav.media')}
        subtitle={collection ? t('media.collectionSubtitle') : t('media.subtitle')}
        actions={
          <>
            <span className="md-badge" data-testid="media-total">
              {formatCount(total)}
            </span>
            {/* c22/§12.4 [Z3d]: add the selection (or the page) to a manual
                list — the add-items rewrite of the retired snapshot flow. */}
            <Button variant="tonal" data-testid="z3d-add-list" onClick={requestAddToList}>
              {selection.count > 0
                ? t('lists.addSelection', { count: selection.count })
                : t('lists.addView', { count: total })}
            </Button>
            <Button variant="tonal" onClick={() => setSelectionMode((v) => !v)}>
              {t('media.selectionMode')}
            </Button>
          </>
        }
      />

      {/* [Z3g-i] L1 filter row — a Mídia concern, never a Zone 1 one. */}
      <MediaFilterRow
        spec={query.spec}
        onUpdate={query.updateSpec}
        onOpenDrawer={() => setDrawerOpen(true)}
      />

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
            onContext={openContext}
            selectionMode={selectionMode}
            selection={selection}
            navEnabled={lightboxId === null}
            onToggleFavorite={toggleFavorite}
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
          vault={vault}
        />
      ) : null}

      {/* L2 filter drawer (§10.6.1). */}
      <FilterDrawer
        open={drawerOpen}
        onClose={() => setDrawerOpen(false)}
        spec={query.spec}
        onApply={query.updateSpec}
      />

      {/* §10.6.2 / mockup `#ctxmenu` — the tile right-click menu. Rendered above
          the grid; `onHide` routes through the shell's hide bridge, so a locked
          vault sends the human to the gate rather than silently failing. */}
      {ctx ? (
        <MediaContextMenu
          target={ctx}
          vaultConfigured={vault.configured}
          isFavorite={items.find((item) => item.id === ctx.id)?.isFavorite ?? false}
          onOpen={openLightbox}
          onHide={vault.onHide}
          onToggleFavorite={(id) => {
            const item = items.find((entry) => entry.id === id)
            if (item) toggleFavorite(item)
          }}
          onClose={closeContext}
        />
      ) : null}
    </div>
  )
}
