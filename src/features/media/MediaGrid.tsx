import { useRef, useCallback, useEffect, useState } from 'react'
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
 * §10.6.5 `MediaGrid` — virtualized grid with LQIP tiles.
 *
 * Above 1,000 items only the viewport is rendered (`@tanstack/react-virtual`
 * windows the rows); below that the grid renders every tile. Each tile shows
 * the 200px WebP immediately and upgrades to 400px on hover (§10.6.5 LQIP).
 * The overlay has a checkbox (top-left) and a fav star (top-right); the body
 * click opens the lightbox.
 */
export function MediaGrid({
  items,
  root,
  onOpen,
  selectionMode,
  selection,
}: {
  items: MediaDto[]
  root: string | null
  onOpen: (id: string) => void
  selectionMode: boolean
  selection: ReturnType<typeof useSelection>
}) {
  const { selected, toggle } = selection

  const handleBodyClick = useCallback(
    (id: string) => {
      if (selectionMode) {
        toggle(id)
      } else {
        onOpen(id)
      }
    },
    [selectionMode, toggle, onOpen],
  )

  if (items.length <= VIRTUALIZE_ABOVE) {
    return (
      <div className="media-grid" data-testid="media-grid">
        {items.map((item) => (
          <MediaTile
            key={item.id}
            item={item}
            root={root}
            selected={selected.has(item.id)}
            onClick={() => handleBodyClick(item.id)}
            onToggle={() => toggle(item.id)}
          />
        ))}
      </div>
    )
  }

  return (
    <VirtualGrid
      items={items}
      root={root}
      onOpen={onOpen}
      selectionMode={selectionMode}
      selection={selection}
    />
  )
}

function VirtualGrid({
  items,
  root,
  onOpen,
  selectionMode,
  selection,
}: {
  items: MediaDto[]
  root: string | null
  onOpen: (id: string) => void
  selectionMode: boolean
  selection: ReturnType<typeof useSelection>
}) {
  const { selected, toggle } = selection
  const parentRef = useRef<HTMLDivElement>(null)
  const [width, setWidth] = useState(0)

  // The row height follows the column width: the tile is a square image plus a
  // fixed info strip, so a hardcoded 220px would clip on a wide window.
  useEffect(() => {
    const element = parentRef.current
    if (!element) return
    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) setWidth(entry.contentRect.width)
    })
    observer.observe(element)
    setWidth(element.clientWidth)
    return () => observer.disconnect()
  }, [])

  const tileWidth = (width - GAP * (VIRTUAL_COLUMNS - 1)) / VIRTUAL_COLUMNS
  const rowHeight = (tileWidth > 0 ? tileWidth : ROW_HEIGHT - GAP) + INFO_HEIGHT

  const rowCount = Math.ceil(items.length / VIRTUAL_COLUMNS)

  const virtualizer = useVirtualizer({
    count: rowCount,
    getScrollElement: () => parentRef.current,
    estimateSize: () => rowHeight + GAP,
    overscan: 3,
  })

  const handleBodyClick = useCallback(
    (id: string) => {
      if (selectionMode) {
        toggle(id)
      } else {
        onOpen(id)
      }
    },
    [selectionMode, toggle, onOpen],
  )

  return (
    <div ref={parentRef} className="media-grid__virtual" data-testid="media-grid">
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
              {rowItems.map((item) => (
                <MediaTile
                  key={item.id}
                  item={item}
                  root={root}
                  selected={selected.has(item.id)}
                  onClick={() => handleBodyClick(item.id)}
                  onToggle={() => toggle(item.id)}
                />
              ))}
            </div>
          )
        })}
      </div>
    </div>
  )
}

function MediaTile({
  item,
  root,
  selected,
  onClick,
  onToggle,
}: {
  item: MediaDto
  root: string | null
  selected: boolean
  onClick: () => void
  onToggle: () => void
}) {
  const { t } = useI18n()
  // §10.6.5 LQIP: the 200px WebP shows first, hover swaps in the 400px one
  // (§6.7 — both slots are already on disk, so this is a second load, never a
  // rescale).
  const [hovered, setHovered] = useState(false)
  const src = thumbUrl(root, hovered ? item.thumb400 : item.thumb200)

  return (
    <article
      className={`media-tile${selected ? ' media-tile--selected' : ''}`}
      data-testid="media-tile"
      onClick={onClick}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          onClick()
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
            backend-side to `Thumbnails/` only. An empty src or a missing file
            hides the img and the tile still shows the name + meta, never a
            broken-image icon. */}
        {src ? (
          <img
            src={src}
            alt=""
            loading="lazy"
            className="media-tile__img"
            onError={(e) => {
              ;(e.target as HTMLImageElement).style.display = 'none'
            }}
          />
        ) : null}
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
        <button
          type="button"
          className="media-tile__fav"
          aria-label={t('media.favorite')}
          onClick={(e) => e.stopPropagation()}
        >
          ♡
        </button>
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
