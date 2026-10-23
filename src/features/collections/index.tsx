import { useMemo, useState } from 'react'
import { ErrorCard, PageHeader, SkeletonBlock } from '../../components/ui'
import { useI18n } from '../../hooks/useContexts'
import { useAsync } from '../../hooks/useAsync'
import { useDbChanged } from '../../hooks/useTauriEvent'
import { formatCount } from '../../lib/format'
import { collectionsList, mediaDetail, resolveAppRoot } from '../../lib/ipc'
import type { CollectionDto, MediaDto, RootInfoDto } from '../../types/api'
import { thumbUrl } from '../media/assetUrl'

/**
 * c22/§11.10: the card grid opens one collection into the scoped gallery by
 * handing the shell the key, which then mounts `MediaScreen` with `collection`
 * set — the cards and the gallery never talk to each other directly.
 */
export type OpenCollection = { key: string; labelKey: string }

/**
 * c22/§11.10 `CollectionsScreen` — the five smart collections as cards
 * (`[C-8a]`..`[C-8g]`, §24.1).
 *
 * Google Photos shape (human 2026-10-19): a responsive grid of **large rounded
 * cards**, each one a full-bleed cover thumbnail with the title in strong type
 * and the exact item count **below the card** — not a badge crammed onto the
 * image. Cards read `collections_list` (c21): name, count, up to four cover
 * thumbnails. Single click selects (the Explorer model the human confirmed for
 * tiles, §12.6, reused here); double-click or Enter opens one into the scoped
 * gallery (`collection_open`, C-8c) by handing the shell the key. Documentos
 * and Pessoas & Animais have no predicate yet (Phase 8): they render greyed
 * with "Disponível em breve" — the card exists because the wireframe freezes
 * it, but it is never a dead click.
 */
export function CollectionsScreen({ onOpen }: { onOpen: (target: OpenCollection) => void }) {
  const { t } = useI18n()
  const { data, error, loading, rerun } = useAsync<CollectionDto[]>(
    () => collectionsList('standard'),
    [],
  )
  useDbChanged(rerun)
  const rootInfo = useAsync<RootInfoDto>(() => resolveAppRoot(), [])
  const root = rootInfo.data?.root ?? null

  const [selected, setSelected] = useState<string | null>(null)
  // Memoized: a new `[]` identity every render would re-key the cover reader
  // below into an infinite refetch (the `useAsync` deps are identities).
  const cards = useMemo(() => data ?? [], [data])

  // One batched read for every cover the cards reference: five cards × up to
  // four ids, resolved together so the grid paints once instead of per card.
  const coverIds = useMemo(() => {
    const ids = new Set<string>()
    for (const card of cards) {
      for (const id of card.coverMediaIds) ids.add(id)
    }
    return [...ids]
  }, [cards])
  const coverKey = coverIds.join('|')
  const covers = useAsync(async () => {
    const map = new Map<string, MediaDto>()
    await Promise.all(
      coverIds.map(async (id) => {
        const media = await mediaDetail(id)
        if (media) map.set(id, media)
      }),
    )
    return map
  }, [coverKey])

  return (
    <div className="screen">
      <PageHeader title={t('nav.collections')} subtitle={t('collections.subtitle')} />

      {error ? (
        <ErrorCard messageKey="error.unavailable" detail={error.detail} onRetry={rerun} />
      ) : loading ? (
        <div className="collections__grid" data-testid="collections-grid">
          {Array.from({ length: 5 }, (_, index) => (
            <SkeletonBlock key={index} height={200} />
          ))}
        </div>
      ) : (
        <div className="collections__grid" data-testid="collections-grid">
          {cards.map((card) => (
            <CollectionCard
              key={card.key}
              card={card}
              covers={covers.data}
              root={root}
              selected={selected === card.key}
              onSelect={setSelected}
              onOpen={onOpen}
            />
          ))}
        </div>
      )}
    </div>
  )
}

function CollectionCard({
  card,
  covers,
  root,
  selected,
  onSelect,
  onOpen,
}: {
  card: CollectionDto
  covers: Map<string, MediaDto> | null
  root: string | null
  selected: boolean
  onSelect: (key: string) => void
  onOpen: (target: OpenCollection) => void
}) {
  const { t } = useI18n()
  // §24.1: the two Phase-8 predicates are known to the backend as count-0
  // collections. The DTO carries no availability flag, so the FE names them
  // by key — the same list §11.10's wireframe freezes.
  const pending = card.key === 'documents' || card.key === 'people'
  const name = t(card.labelKey)

  const open = () => {
    if (pending) return
    onOpen({ key: card.key, labelKey: card.labelKey })
  }

  // Google Photos cover: one lead thumbnail fills the card, up to three more sit
  // beside it as a strip. A short set leaves the lead square and the strip
  // empty — no fake slots pretending there are more photos than there are.
  const [lead, ...rest] = card.coverMediaIds
  const leadMedia = lead ? covers?.get(lead) : undefined
  const strip = rest.slice(0, 3)

  return (
    <article className="collections__card-wrap">
      <button
        type="button"
        className={`collections__card${selected ? ' collections__card--selected' : ''}${pending ? ' collections__card--pending' : ''}`}
        data-testid="collection-card"
        data-key={card.key}
        aria-disabled={pending || undefined}
        aria-label={
          pending ? `${name} — ${t('collections.soon')}` : `${name} — ${formatCount(card.count)}`
        }
        onClick={() => {
          onSelect(card.key)
          // Explorer model (§12.6): a plain click only moves the highlight; the
          // open is the double-click (or Enter, below) — so a re-click on the
          // open collection never yanks the human back into the gallery.
          if (selected) open()
        }}
        onDoubleClick={open}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault()
            open()
          }
        }}
      >
        <span className="collections__cover" aria-hidden="true">
          {leadMedia ? (
            <img
              className="collections__cover-img"
              src={thumbUrl(root, leadMedia.thumb400 ?? leadMedia.thumb200)}
              alt=""
              loading="lazy"
            />
          ) : (
            <span className="collections__cover-empty" />
          )}
          {strip.length > 0 ? (
            <span className="collections__strip">
              {strip.map((id) => {
                const media = covers?.get(id)
                return media ? (
                  <img
                    key={id}
                    className="collections__strip-img"
                    src={thumbUrl(root, media.thumb200)}
                    alt=""
                    loading="lazy"
                  />
                ) : (
                  <span key={id} className="collections__strip-slot" />
                )
              })}
            </span>
          ) : null}
          {pending ? <span className="collections__pending-veil" aria-hidden="true" /> : null}
        </span>
        {/* Google Photos prints the label and count *under* the cover — inside
            the card button, so the whole card is one target and one testable
            text node (`collection-card`'s textContent carries name + count). */}
        <span className="collections__meta">
          <span className="collections__name">{name}</span>
          <span className="collections__count">
            {pending
              ? t('collections.soon')
              : t('collections.items', { count: formatCount(card.count) })}
          </span>
        </span>
      </button>
    </article>
  )
}
