import {
  useRef,
  useCallback,
  useEffect,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
} from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { useI18n } from '../../hooks/useContexts'
import { useSelection } from '../../hooks/useSelection'
import { formatBytes } from '../../lib/format'
import type { MediaDto } from '../../types/api'
import { thumbUrl } from './assetUrl'

const ROW_HEIGHT = 220
const GAP = 12
/** The info strip under the square image (§10.6.5's tile), fixed in CSS. */
const INFO_HEIGHT = 48
/** §10.6.5: windowing applies above 1,000 items; below that the grid is plain. */
const VIRTUALIZE_ABOVE = 1000
/** The virtual grid is 4 columns wide (one row per virtualizer index). */
const VIRTUAL_COLUMNS = 4
/**
 * c13 fix (2026-10-05): a thumbnail is produced in the background (the §7.6
 * boot/ingest/scan mop-up), so a tile that 404s at first paint may exist a
 * second later. Instead of hiding it forever, the image retries with a
 * cache-buster until it lands — bounded, so a permanently missing tile (e.g. an
 * ffmpeg-only video with no host) settles into its name+meta state.
 *
 * 2026-10-11: the window now covers a whole regeneration sweep, not just a
 * scan's tail. Regenerating a real library (hundreds of tiles, one ffmpeg pass
 * each) runs for minutes; a 30 s ladder gave up long before the files landed, so
 * a tile stayed grey until a hover swapped in the 400 (§10.6.5). 300 × 1.5 s is
 * a 7.5 min ceiling, and `loading="lazy"` keeps the retries to tiles near the
 * viewport, so the window stays cheap.
 */
const THUMB_RETRY_LIMIT = 300
const THUMB_RETRY_MS = 1500

/**
 * §10.6.5 `MediaGrid` — virtualized grid with LQIP tiles.
 *
 * Above 1,000 items only the viewport is rendered (`@tanstack/react-virtual`
 * windows the rows); below that the grid renders every tile. Each tile shows
 * the 200px WebP immediately and upgrades to 400px on hover (§10.6.5 LQIP).
 * The overlay has a checkbox (top-left) and a fav star (top-right).
 *
 * Windows-Explorer interaction model (human-confirmed 2026-10-15, §12.6):
 * single click selects, double click opens the lightbox, Ctrl/Cmd+click toggles
 * membership without opening, Shift+click ranges from the anchor, arrow keys
 * walk a keyboard cursor over the tiles (Shift+arrows extend), Space toggles
 * the focused tile and Enter opens it.
 */
export function MediaGrid({
  items,
  root,
  onOpen,
  onContext,
  selectionMode,
  selection,
  navEnabled,
  thumbSrc,
  onToggleFavorite,
}: {
  items: MediaDto[]
  root: string | null
  onOpen: (id: string) => void
  /**
   * c19 (2026-10-17): the tile context menu (right-click), mirrored from the
   * frozen mockup's `#ctxmenu`. Absent in the §11.6 vault grid, where the
   * mockup also suppresses it (`S.view === "vault"`).
   */
  onContext?: (item: MediaDto, event: ReactMouseEvent) => void
  selectionMode: boolean
  selection: ReturnType<typeof useSelection>
  /** False while the lightbox is up: `←/→` then belong to the overlay, not the grid. */
  navEnabled: boolean
  /**
   * c19: the tile source resolver. Defaults to the `Thumbnails/`-scoped asset
   * protocol (§10.6.5); the §11.6 vault grid passes `vaultThumbUrl`, because a
   * vault row's thumbnail lives under `.vault/thumbs/` and must go through the
   * session-gated `wolfsvault` scheme instead. `hi` is the hover (400px) slot.
   */
  thumbSrc?: (item: MediaDto, hi: boolean) => string
  /**
   * c22/§24.2: the ♡ toggle. Absent in the vault grid, the pick-media modal and
   * anywhere else the mockup's overlay star is not a live control — a heart
   * that does nothing is exactly the dead control the review rule forbids.
   */
  onToggleFavorite?: (item: MediaDto) => void
}) {
  const { selected } = selection
  const handleClick = useTileClick({ items, selectionMode, selection })

  if (items.length <= VIRTUALIZE_ABOVE) {
    return (
      <PlainGrid
        items={items}
        root={root}
        onOpen={onOpen}
        onContext={onContext}
        selected={selected}
        handleClick={handleClick}
        onToggle={selection.toggle}
        selection={selection}
        navEnabled={navEnabled}
        thumbSrc={thumbSrc}
        onToggleFavorite={onToggleFavorite}
      />
    )
  }

  return (
    <VirtualGrid
      items={items}
      root={root}
      onOpen={onOpen}
      onContext={onContext}
      selected={selected}
      handleClick={handleClick}
      onToggle={selection.toggle}
      selection={selection}
      navEnabled={navEnabled}
      thumbSrc={thumbSrc}
      onToggleFavorite={onToggleFavorite}
    />
  )
}

/**
 * The default tile source: the `Thumbnails/`-scoped asset protocol. Extracted so
 * both grid variants and the vault override agree on the hover (400px) slot.
 */
function defaultThumbSrc(root: string | null, item: MediaDto, hi: boolean): string {
  return thumbUrl(root, hi ? item.thumb400 : item.thumb200)
}

/** The Explorer click map, shared by both grid variants. */
function useTileClick({
  items,
  selectionMode,
  selection,
}: {
  items: MediaDto[]
  selectionMode: boolean
  selection: ReturnType<typeof useSelection>
}) {
  const { toggle, selectOne, selectRange } = selection
  return useCallback(
    (index: number, event: ReactMouseEvent) => {
      const id = items[index]?.id
      if (!id) return
      if (event.ctrlKey || event.metaKey) {
        toggle(id)
        return
      }
      if (event.shiftKey) {
        const anchorIndex = selection.anchor
          ? items.findIndex((item) => item.id === selection.anchor)
          : -1
        if (anchorIndex < 0) {
          selectOne(id)
          return
        }
        const [from, to] = anchorIndex <= index ? [anchorIndex, index] : [index, anchorIndex]
        selectRange(items.slice(from, to + 1).map((item) => item.id))
        return
      }
      if (selectionMode) {
        toggle(id)
      } else {
        selectOne(id)
      }
    },
    [items, selectionMode, selection.anchor, toggle, selectOne, selectRange],
  )
}

/**
 * Where an arrow lands, Windows-Explorer row semantics: `←/→` step one tile,
 * `↑/↓` step one row of `columns`. Clamped at the edges (never wraps, never
 * dead), `null` for a non-arrow key or an empty page.
 */
function arrowTarget(current: number, key: string, columns: number, length: number): number | null {
  if (length === 0) {
    return null
  }
  const cols = Math.max(1, columns)
  let next: number
  switch (key) {
    case 'ArrowLeft':
      next = current - 1
      break
    case 'ArrowRight':
      next = current + 1
      break
    case 'ArrowUp':
      next = current - cols
      break
    case 'ArrowDown':
      next = current + cols
      break
    default:
      return null
  }
  return Math.min(Math.max(next, 0), length - 1)
}

/**
 * The keyboard cursor. Reads the focused tile when it is inside `containerRef`,
 * else falls back to the anchor (so `←/→` work right after a click, before any
 * tile has real DOM focus). Moves the selection the Explorer way — plain arrow
 * replaces it, Shift+arrow extends from the anchor — and asks the grid to put
 * DOM focus on the landed tile on the next frame (the virtual grid may still
 * have to render that row).
 */
function useKeyboardNav({
  items,
  columns,
  selection,
  navEnabled,
}: {
  items: MediaDto[]
  /** Column count resolution, given the grid container (measured when non-virtual). */
  columns: (container: HTMLElement | null) => number
  selection: ReturnType<typeof useSelection>
  navEnabled: boolean
}) {
  const containerRef = useRef<HTMLDivElement>(null)
  const [pendingFocus, setPendingFocus] = useState<number | null>(null)
  const { selectOne, selectRange, setAnchor } = selection

  useEffect(() => {
    if (pendingFocus === null) {
      return
    }
    let frame = 0
    let raf: number | null = null
    const attempt = () => {
      const tile = containerRef.current?.querySelector<HTMLElement>(
        `[data-index="${pendingFocus}"]`,
      )
      if (tile) {
        tile.focus({ preventScroll: true })
        // jsdom has no `scrollIntoView`; the optional call keeps the tests honest.
        tile.scrollIntoView?.({ block: 'nearest' })
        setPendingFocus(null)
        return
      }
      // The virtual grid scrolled; give React a few frames to render the row.
      if (frame++ < 5 && typeof requestAnimationFrame === 'function') {
        raf = requestAnimationFrame(attempt)
      } else {
        setPendingFocus(null)
      }
    }
    // A plain grid (and every test) has the tile already; only the virtual one waits.
    attempt()
    return () => {
      if (raf !== null) {
        cancelAnimationFrame(raf)
      }
    }
  }, [pendingFocus])

  const onKeyDown = useCallback(
    (event: ReactKeyboardEvent) => {
      if (!navEnabled || !event.key.startsWith('Arrow') || items.length === 0) {
        return
      }
      const active = document.activeElement as HTMLElement | null
      const focused = active?.closest<HTMLElement>('[data-index]')
      const anchorIndex = selection.anchor
        ? items.findIndex((item) => item.id === selection.anchor)
        : -1
      const from = focused ? Number(focused.dataset.index) : anchorIndex
      const start = from >= 0 ? from : 0
      const to = arrowTarget(start, event.key, columns(containerRef.current), items.length)
      if (to === null) {
        return
      }
      event.preventDefault()
      if (event.shiftKey) {
        const origin = anchorIndex >= 0 ? anchorIndex : start
        const [a, b] = origin <= to ? [origin, to] : [to, origin]
        selectRange(items.slice(a, b + 1).map((item) => item.id))
        if (anchorIndex < 0) {
          setAnchor(items[origin].id)
        }
      } else {
        selectOne(items[to].id)
      }
      setPendingFocus(to)
    },
    [navEnabled, items, columns, selection.anchor, selectOne, selectRange, setAnchor],
  )

  return { containerRef, onKeyDown, pendingFocus }
}

/** Below the virtualize threshold: every tile is rendered, so columns are measurable. */
function PlainGrid({
  items,
  root,
  onOpen,
  onContext,
  selected,
  handleClick,
  onToggle,
  selection,
  navEnabled,
  thumbSrc,
  onToggleFavorite,
}: {
  items: MediaDto[]
  root: string | null
  onOpen: (id: string) => void
  onContext?: (item: MediaDto, event: ReactMouseEvent) => void
  selected: ReadonlySet<string>
  handleClick: (index: number, event: ReactMouseEvent) => void
  onToggle: (id: string) => void
  selection: ReturnType<typeof useSelection>
  navEnabled: boolean
  thumbSrc?: (item: MediaDto, hi: boolean) => string
  onToggleFavorite?: (item: MediaDto) => void
}) {
  const { containerRef, onKeyDown } = useKeyboardNav({
    items,
    columns: measureColumns,
    selection,
    navEnabled,
  })
  const resolve = thumbSrc ?? ((item: MediaDto, hi: boolean) => defaultThumbSrc(root, item, hi))

  return (
    <div ref={containerRef} className="media-grid" data-testid="media-grid" onKeyDown={onKeyDown}>
      {items.map((item, index) => (
        <MediaTile
          key={item.id}
          item={item}
          index={index}
          thumbSrc={resolve}
          selected={selected.has(item.id)}
          onClick={(event) => handleClick(index, event)}
          onOpen={() => onOpen(item.id)}
          onToggle={() => onToggle(item.id)}
          onContext={onContext}
          onToggleFavorite={onToggleFavorite}
        />
      ))}
    </div>
  )
}

/**
 * `grid-template-columns` is the authoritative column count (an auto-fill grid
 * can change it with the window); the tile-position fallback covers engines
 * that report no computed track list. `1` is the safe floor.
 */
function measureColumns(container: HTMLElement | null): number {
  if (!container) {
    return 1
  }
  const tracks = getComputedStyle(container).gridTemplateColumns.split(' ').filter(Boolean)
  if (tracks.length > 0) {
    return tracks.length
  }
  const tiles = Array.from(container.querySelectorAll<HTMLElement>('[data-index]'))
  if (tiles.length === 0) {
    return 1
  }
  const firstTop = tiles[0].offsetTop
  return Math.max(1, tiles.filter((tile) => tile.offsetTop === firstTop).length)
}

function VirtualGrid({
  items,
  root,
  onOpen,
  onContext,
  selected,
  handleClick,
  onToggle,
  selection,
  navEnabled,
  thumbSrc,
  onToggleFavorite,
}: {
  items: MediaDto[]
  root: string | null
  onOpen: (id: string) => void
  onContext?: (item: MediaDto, event: ReactMouseEvent) => void
  selected: ReadonlySet<string>
  handleClick: (index: number, event: ReactMouseEvent) => void
  onToggle: (id: string) => void
  selection: ReturnType<typeof useSelection>
  navEnabled: boolean
  thumbSrc?: (item: MediaDto, hi: boolean) => string
  onToggleFavorite?: (item: MediaDto) => void
}) {
  const { containerRef, onKeyDown, pendingFocus } = useKeyboardNav({
    items,
    columns: () => VIRTUAL_COLUMNS,
    selection,
    navEnabled,
  })
  const resolve = thumbSrc ?? ((item: MediaDto, hi: boolean) => defaultThumbSrc(root, item, hi))
  const [width, setWidth] = useState(0)

  // The row height follows the column width: the tile is a square image plus a
  // fixed info strip, so a hardcoded 220px would clip on a wide window.
  useEffect(() => {
    const element = containerRef.current
    if (!element) return
    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) setWidth(entry.contentRect.width)
    })
    observer.observe(element)
    setWidth(element.clientWidth)
    return () => observer.disconnect()
  }, [containerRef])

  const tileWidth = (width - GAP * (VIRTUAL_COLUMNS - 1)) / VIRTUAL_COLUMNS
  const rowHeight = (tileWidth > 0 ? tileWidth : ROW_HEIGHT - GAP) + INFO_HEIGHT

  const rowCount = Math.ceil(items.length / VIRTUAL_COLUMNS)

  const virtualizer = useVirtualizer({
    count: rowCount,
    getScrollElement: () => containerRef.current,
    estimateSize: () => rowHeight + GAP,
    overscan: 3,
  })

  // An arrow can land on a row that is not rendered; scroll it in first so the
  // hook's focus attempt (next frame) finds the tile.
  useEffect(() => {
    if (pendingFocus === null) {
      return
    }
    virtualizer.scrollToIndex(Math.floor(pendingFocus / VIRTUAL_COLUMNS), { align: 'auto' })
  }, [pendingFocus, virtualizer])

  return (
    <div
      ref={containerRef}
      className="media-grid__virtual"
      data-testid="media-grid"
      onKeyDown={onKeyDown}
    >
      <div
        style={{
          height: `${virtualizer.getTotalSize()}px`,
          width: '100%',
          position: 'relative',
        }}
      >
        {virtualizer.getVirtualItems().map((virtualRow) => {
          const start = virtualRow.index * VIRTUAL_COLUMNS
          const rowItems = items.slice(start, start + VIRTUAL_COLUMNS)
          return (
            <div
              key={virtualRow.key}
              style={{
                position: 'absolute',
                top: 0,
                left: 0,
                width: '100%',
                height: `${rowHeight}px`,
                transform: `translateY(${virtualRow.start}px)`,
                display: 'grid',
                gridTemplateColumns: `repeat(${VIRTUAL_COLUMNS}, 1fr)`,
                gap: `${GAP}px`,
              }}
            >
              {rowItems.map((item, offset) => {
                const index = start + offset
                return (
                  <MediaTile
                    key={item.id}
                    item={item}
                    index={index}
                    thumbSrc={resolve}
                    selected={selected.has(item.id)}
                    onClick={(event) => handleClick(index, event)}
                    onOpen={() => onOpen(item.id)}
                    onToggle={() => onToggle(item.id)}
                    onContext={onContext}
                    onToggleFavorite={onToggleFavorite}
                  />
                )
              })}
            </div>
          )
        })}
      </div>
    </div>
  )
}

function MediaTile({
  item,
  index,
  thumbSrc,
  selected,
  onClick,
  onOpen,
  onToggle,
  onContext,
  onToggleFavorite,
}: {
  item: MediaDto
  index: number
  /** c19: resolves the tile source; the vault grid swaps in `vaultThumbUrl`. */
  thumbSrc: (item: MediaDto, hi: boolean) => string
  selected: boolean
  onClick: (event: ReactMouseEvent) => void
  onOpen: () => void
  onToggle: () => void
  onContext?: (item: MediaDto, event: ReactMouseEvent) => void
  onToggleFavorite?: (item: MediaDto) => void
}) {
  const { t } = useI18n()
  // §10.6.5 LQIP: the 200px WebP shows first, hover swaps in the 400px one
  // (§6.7 — both slots are already on disk, so this is a second load, never a
  // rescale).
  const [hovered, setHovered] = useState(false)
  const src = thumbSrc(item, hovered)

  return (
    <article
      className={`media-tile${selected ? ' media-tile--selected' : ''}`}
      data-testid="media-tile"
      data-index={index}
      onClick={onClick}
      onDoubleClick={onOpen}
      onContextMenu={(e) => {
        // c19: only the Mídia grid opens the context menu (the mockup suppresses
        // it in the vault view); a consumer that passes no `onContext` keeps the
        // browser's own menu.
        if (!onContext) return
        e.preventDefault()
        onContext(item, e)
      }}
      onKeyDown={(e) => {
        // Explorer keyboard on a tile: Enter opens, Space toggles it (which the
        // window-level Space binding then leaves alone through its I-beam guard).
        if (e.key === 'Enter') {
          e.preventDefault()
          onOpen()
        } else if (e.key === ' ') {
          e.preventDefault()
          onToggle()
        }
      }}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      tabIndex={0}
      role="button"
      aria-pressed={selected}
      aria-label={item.relativePath}
    >
      <div className="media-tile__image">
        {/* c10: sources go through the asset protocol (`assetUrl`), scoped
            backend-side to `Thumbnails/` only. `ThumbImage` retries while the
            background mop-up writes the tile; a source that never arrives leaves
            the name + meta, never a broken-image icon. */}
        <ThumbImage key={hovered ? 'hi' : 'lo'} src={src} />
        {item.fileType === 'video' ? <span className="media-tile__badge">▶</span> : null}
      </div>
      <div className="media-tile__overlay">
        <button
          type="button"
          className="media-tile__check"
          aria-label={t('media.select')}
          aria-pressed={selected}
          onClick={(e) => {
            e.stopPropagation()
            onToggle()
          }}
        >
          {selected ? '☑' : '☐'}
        </button>
        {/* c22/§24.2 — the ♡ toggle. Rendered only where a consumer can act on
            it (no handler → no control, dead-control rule); `aria-pressed`
            carries the state, the glyph only paints it. */}
        {onToggleFavorite ? (
          <button
            type="button"
            className={`media-tile__fav${item.isFavorite ? ' media-tile__fav--on' : ''}`}
            data-testid="tile-fav"
            aria-label={item.isFavorite ? t('media.unfavorite') : t('media.favorite')}
            aria-pressed={item.isFavorite}
            onClick={(e) => {
              e.stopPropagation()
              onToggleFavorite(item)
            }}
          >
            {item.isFavorite ? '♥' : '♡'}
          </button>
        ) : null}
      </div>
      <div className="media-tile__info">
        <span className="media-tile__name">{item.relativePath.split('/').pop()}</span>
        <span className="media-tile__meta">
          {item.capturedAt ? item.capturedAt.slice(0, 10) : '—'} · {formatBytes(item.fileSize)}
        </span>
      </div>
    </article>
  )
}

/**
 * A tile's image with a bounded reload ladder. The `key` at the call site
 * remounts it on a hover swap, so the retry count never leaks across sources.
 */
function ThumbImage({ src }: { src: string }) {
  const [retry, setRetry] = useState(0)
  const [hidden, setHidden] = useState(false)
  const timer = useRef<number | null>(null)

  useEffect(() => {
    return () => {
      if (timer.current !== null) window.clearTimeout(timer.current)
    }
  }, [])

  if (!src || hidden) return null
  const url = retry > 0 ? `${src}?r=${retry}` : src

  return (
    <img
      src={url}
      alt=""
      loading="lazy"
      className="media-tile__img"
      onError={() => {
        if (retry >= THUMB_RETRY_LIMIT) {
          setHidden(true)
          return
        }
        timer.current = window.setTimeout(() => setRetry((n) => n + 1), THUMB_RETRY_MS)
      }}
    />
  )
}
