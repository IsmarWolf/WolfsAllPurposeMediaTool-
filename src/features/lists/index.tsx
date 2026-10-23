import { useMemo, useState } from 'react'
import { Button, EmptyState, ErrorCard, PageHeader, SkeletonBlock } from '../../components/ui'
import { useI18n } from '../../hooks/useContexts'
import { useAsync } from '../../hooks/useAsync'
import { useListActions, useListDetail, useLists } from '../../hooks/useLists'
import { formatBytes, formatCount } from '../../lib/format'
import { mediaDetail, resolveAppRoot } from '../../lib/ipc'
import type { ListDto, MediaDto, RootInfoDto } from '../../types/api'
import { thumbUrl } from '../media/assetUrl'
import { PickMediaModal } from './PickMediaModal'

/**
 * §11.5 Listas — Google Photos card grid of lists; selecting one opens the
 * detail view with a back control (c22 redesign).
 */
export function ListsScreen() {
  const { t } = useI18n()
  const { lists, error: listsError, loading: listsLoading, reload } = useLists()
  const listsRerun = reload
  const actions = useListActions()

  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [creating, setCreating] = useState(false)
  const [newName, setNewName] = useState('')

  const rootInfo = useAsync<RootInfoDto>(() => resolveAppRoot(), [])
  const root = rootInfo.data?.root ?? null

  const coverIds = useMemo(() => {
    const ids = new Set<string>()
    for (const list of lists) {
      for (const id of list.coverMediaIds ?? []) ids.add(id)
    }
    return [...ids]
  }, [lists])
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

  const create = async () => {
    const trimmed = newName.trim()
    if (!trimmed) {
      return
    }
    const created = await actions.create(trimmed)
    if (created) {
      setSelectedId(created.id)
      setNewName('')
      setCreating(false)
    }
  }

  if (selectedId) {
    return <ListDetail id={selectedId} root={root} onBack={() => setSelectedId(null)} />
  }

  return (
    <div className="screen">
      <PageHeader
        title={t('nav.lists')}
        subtitle={t('lists.subtitle')}
        actions={
          <Button variant="filled" onClick={() => setCreating(true)} data-testid="lists-new">
            {t('lists.newList')}
          </Button>
        }
      />

      {creating ? (
        <div className="lists__newrow" data-testid="lists-new-form">
          <input
            type="text"
            className="md-input"
            placeholder={t('lists.namePlaceholder')}
            value={newName}
            autoFocus
            aria-label={t('lists.newList')}
            onChange={(e) => setNewName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                void create()
              } else if (e.key === 'Escape') {
                setCreating(false)
              }
            }}
          />
          <Button variant="filled" onClick={create} disabled={newName.trim().length === 0}>
            {t('lists.create')}
          </Button>
          <Button variant="text" onClick={() => setCreating(false)}>
            {t('common.cancel')}
          </Button>
        </div>
      ) : null}

      {listsError ? (
        <ErrorCard messageKey="lists.loadError" detail={listsError.detail} onRetry={listsRerun} />
      ) : listsLoading ? (
        <div className="lists__grid" data-testid="lists-index">
          {Array.from({ length: 4 }, (_, index) => (
            <SkeletonBlock key={index} height={200} />
          ))}
        </div>
      ) : lists.length === 0 ? (
        <EmptyState
          titleKey="lists.emptyTitle"
          ctaKey="lists.emptyCta"
          onCta={() => setCreating(true)}
        />
      ) : (
        <div className="lists__grid" data-testid="lists-index">
          {lists.map((list) => (
            <ListCard
              key={list.id}
              list={list}
              root={root}
              covers={covers.data}
              onOpen={() => setSelectedId(list.id)}
            />
          ))}
        </div>
      )}
    </div>
  )
}

function ListCard({
  list,
  root,
  covers,
  onOpen,
}: {
  list: ListDto
  root: string | null
  covers: Map<string, MediaDto> | null
  onOpen: () => void
}) {
  const { t } = useI18n()
  const coverIds = list.coverMediaIds ?? []
  const [lead, ...rest] = coverIds
  const leadMedia = lead ? covers?.get(lead) : undefined
  const strip = rest.slice(0, 3)

  return (
    <article className="lists__card-wrap">
      <button type="button" className="lists__card" data-testid="lists-index-item" onClick={onOpen}>
        <span className="lists__cover" aria-hidden="true">
          {leadMedia ? (
            <img
              className="lists__cover-img"
              src={thumbUrl(root, leadMedia.thumb400 ?? leadMedia.thumb200)}
              alt=""
              loading="lazy"
            />
          ) : (
            <span className="lists__cover-empty" />
          )}
          {strip.length > 0 ? (
            <span className="lists__strip">
              {strip.map((id) => {
                const media = covers?.get(id)
                return media ? (
                  <img
                    key={id}
                    className="lists__strip-img"
                    src={thumbUrl(root, media.thumb200)}
                    alt=""
                    loading="lazy"
                  />
                ) : (
                  <span key={id} className="lists__strip-slot" />
                )
              })}
            </span>
          ) : null}
        </span>
        <span className="lists__meta">
          <span className="lists__name">{list.name}</span>
          <span className="lists__count">
            {t('lists.members', { count: formatCount(list.memberCount) })}
          </span>
        </span>
      </button>
    </article>
  )
}

function ListDetail({ id, root, onBack }: { id: string; root: string | null; onBack: () => void }) {
  const { t } = useI18n()
  const actions = useListActions()
  const { detail: rawDetail } = useListDetail(id)
  const detail = rawDetail && rawDetail.id === id ? rawDetail : null

  const [renaming, setRenaming] = useState(false)
  const [draftName, setDraftName] = useState('')
  const [confirmDelete, setConfirmDelete] = useState(false)
  const [showPick, setShowPick] = useState(false)

  const rename = async () => {
    const trimmed = draftName.trim()
    if (!trimmed) return
    const result = await actions.rename(id, trimmed)
    if (result !== null) {
      setRenaming(false)
    }
  }

  const remove = async () => {
    const result = await actions.remove(id)
    if (result !== null) {
      onBack()
    }
  }

  if (!detail) {
    return (
      <div className="screen">
        <div className="collection-back-row">
          <button
            type="button"
            className="collection-back"
            onClick={onBack}
            data-testid="lists-back"
          >
            {t('common.back')}
          </button>
        </div>
        <SkeletonBlock height={160} />
      </div>
    )
  }

  return (
    <div className="screen">
      <div className="collection-back-row">
        <button type="button" className="collection-back" onClick={onBack} data-testid="lists-back">
          {t('common.back')}
        </button>
      </div>

      <header className="lists__detail-head">
        {renaming ? (
          <div className="lists__newrow">
            <input
              type="text"
              className="md-input"
              value={draftName}
              autoFocus
              aria-label={t('lists.rename')}
              onChange={(e) => setDraftName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  void rename()
                } else if (e.key === 'Escape') {
                  setRenaming(false)
                }
              }}
            />
            <Button variant="filled" onClick={rename} data-testid="lists-rename-save">
              {t('common.save')}
            </Button>
            <Button variant="text" onClick={() => setRenaming(false)}>
              {t('common.cancel')}
            </Button>
          </div>
        ) : (
          <>
            <h2 data-testid="lists-detail-name">{detail.name}</h2>
            <Button
              variant="text"
              data-testid="lists-rename"
              onClick={() => {
                setDraftName(detail.name)
                setRenaming(true)
              }}
            >
              {t('lists.rename')}
            </Button>
            {confirmDelete ? (
              <span className="lists__confirm">
                <button
                  type="button"
                  className="md-btn md-btn--text md-btn--danger"
                  data-testid="lists-delete-confirm"
                  onClick={remove}
                >
                  {t('lists.deleteConfirm')}
                </button>
                <Button variant="text" onClick={() => setConfirmDelete(false)}>
                  {t('common.cancel')}
                </Button>
              </span>
            ) : (
              <button
                type="button"
                className="md-btn md-btn--text md-btn--danger"
                data-testid="lists-delete"
                onClick={() => setConfirmDelete(true)}
              >
                {t('lists.delete')}
              </button>
            )}
          </>
        )}
      </header>

      <p className="lists__info" data-testid="lists-info">
        {t('lists.members', { count: detail.memberCount })}
      </p>

      <div className="md-card__row">
        <Button variant="filled" data-testid="lists-add-media" onClick={() => setShowPick(true)}>
          {t('lists.addMedia')}
        </Button>
      </div>

      {detail.members.length === 0 ? (
        <p className="md-card__hint" data-testid="lists-members-empty">
          {t('lists.noMembers')}
        </p>
      ) : (
        <div className="lists__members" data-testid="lists-members">
          {detail.members.map((member) => (
            <MemberTile
              key={member.id}
              member={member}
              root={root}
              onRemove={() => void actions.removeItems(detail.id, [member.id])}
            />
          ))}
        </div>
      )}

      {showPick ? <PickMediaModal listId={id} onClose={() => setShowPick(false)} /> : null}
    </div>
  )
}

function MemberTile({
  member,
  root,
  onRemove,
}: {
  member: MediaDto
  root: string | null
  onRemove: () => void
}) {
  const { t } = useI18n()
  return (
    <article className="media-tile" data-testid="lists-member">
      <div className="media-tile__image">
        <img
          src={thumbUrl(root, member.thumb200)}
          alt=""
          loading="lazy"
          className="media-tile__img"
        />
        {member.fileType === 'video' ? <span className="media-tile__badge">▶</span> : null}
      </div>
      <div className="media-tile__overlay">
        <button
          type="button"
          className="media-tile__check"
          data-testid="lists-member-remove"
          aria-label={`${t('lists.remove')}: ${member.relativePath}`}
          onClick={onRemove}
        >
          ✕
        </button>
      </div>
      <div className="media-tile__info">
        <span className="media-tile__name">{member.relativePath.split('/').pop()}</span>
        <span className="media-tile__meta">
          {member.capturedAt ? member.capturedAt.slice(0, 10) : '—'} ·{' '}
          {formatBytes(member.fileSize)}
        </span>
      </div>
    </article>
  )
}
