import { useState } from 'react'
import { Button, EmptyState, PageHeader, SkeletonBlock } from '../../components/ui'
import { useI18n, useToast } from '../../hooks/useContexts'
import { useAsync } from '../../hooks/useAsync'
import { useListActions, useListDetail, useLists } from '../../hooks/useLists'
import { formatBytes, formatCount } from '../../lib/format'
import { resolveAppRoot } from '../../lib/ipc'
import type { MediaDto, RootInfoDto } from '../../types/api'
import { thumbUrl } from '../media/assetUrl'
import { appliedFilters, chipLabel } from '../media/applied'
import { PickMediaModal } from './PickMediaModal'
import { SnapshotBuilder } from './SnapshotBuilder'

/**
 * §11.5 Listas — the two-pane screen.
 *
 * Left: the list index (`[C-5b]`, A→Z, manual/smart badge). Right: the selected
 * list — inline rename `[C-5c]`, delete `[C-5d]`, info `[C-5e]`, and either the
 * smart bar `[C-5f]` / `[Sincronizar]` with a criteria summary `[C-5h]`, or the
 * manual `[C-5g Adicionar mídia]`. The snapshot builder and pick-media modal are
 * the §11.2 drawer and the gallery reused (c16), never a second implementation.
 *
 * The active list is **derived** (`selectedId` falls back to the first row)
 * rather than mirrored in an effect: deleting the active list cannot leave a
 * dangling id on screen, and a fresh load shows a list without an extra render.
 */
export function ListsScreen() {
  const { t } = useI18n()
  const { push } = useToast()
  const { lists, error, loading } = useLists()
  const actions = useListActions()

  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [creating, setCreating] = useState(false)
  const [newName, setNewName] = useState('')
  const [renaming, setRenaming] = useState(false)
  const [draftName, setDraftName] = useState('')
  const [confirmDelete, setConfirmDelete] = useState(false)
  const [showSnapshot, setShowSnapshot] = useState(false)
  const [showPick, setShowPick] = useState(false)

  const activeId =
    selectedId && lists.some((list) => list.id === selectedId) ? selectedId : (lists[0]?.id ?? null)
  const { detail: rawDetail } = useListDetail(activeId)
  // `useAsync` does not blank `data` when the dependency changes, so switching
  // lists would flash the previous list's members. Tie the shown detail to the
  // active id: a mismatched (or null) detail renders the skeleton, never stale.
  const detail = rawDetail && rawDetail.id === activeId ? rawDetail : null

  const rootInfo = useAsync<RootInfoDto>(() => resolveAppRoot(), [])
  const root = rootInfo.data?.root ?? null

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

  const rename = async () => {
    const trimmed = draftName.trim()
    if (!activeId || !trimmed) {
      return
    }
    const result = await actions.rename(activeId, trimmed)
    if (result !== null) {
      setRenaming(false)
    }
  }

  const remove = async () => {
    if (!activeId) {
      return
    }
    const result = await actions.remove(activeId)
    if (result !== null) {
      setConfirmDelete(false)
      setSelectedId(null)
    }
  }

  const sync = async () => {
    if (!activeId) {
      return
    }
    const result = await actions.sync(activeId)
    if (result !== null) {
      push('success', t('lists.synced'))
    }
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

      <div className="lists">
        <nav className="lists__index" aria-label={t('lists.indexLabel')}>
          {error ? (
            <p className="md-card__hint md-card__hint--error">{t('lists.loadError')}</p>
          ) : loading ? (
            <SkeletonBlock height={120} />
          ) : lists.length === 0 ? (
            <p className="md-card__hint">{t('lists.none')}</p>
          ) : (
            lists.map((list) => (
              <button
                key={list.id}
                type="button"
                data-testid="lists-index-item"
                className={`lists__item${list.id === activeId ? ' lists__item--active' : ''}`}
                aria-current={list.id === activeId ? 'true' : undefined}
                onClick={() => setSelectedId(list.id)}
              >
                <span className="lists__item-name">{list.name}</span>
                <span
                  className={`lists__badge lists__badge--${list.filterCriteria ? 'smart' : 'manual'}`}
                >
                  {list.filterCriteria ? t('lists.kindSmart') : t('lists.kindManual')}
                </span>
                <span className="lists__item-count">{formatCount(list.memberCount)}</span>
              </button>
            ))
          )}
        </nav>

        <section className="lists__detail" data-testid="lists-detail">
          {!activeId ? (
            <EmptyState messageKey="lists.selectOne" />
          ) : !detail ? (
            <SkeletonBlock height={160} />
          ) : (
            <>
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

              {/* [C-5e] tipo · membros n */}
              <p className="lists__info" data-testid="lists-info">
                {detail.filterCriteria ? t('lists.kindSmart') : t('lists.kindManual')}
                {' · '}
                {t('lists.members', { count: detail.memberCount })}
              </p>

              {detail.filterCriteria ? (
                <>
                  {/* [C-5h] criteria summary — chips derived from the stored JSON,
                      built by the same helper the gallery chips use (§15 parity). */}
                  <div className="media-chips" data-testid="lists-criteria">
                    {appliedFilters(detail.filterCriteria).map((filter, index) => (
                      <span key={`${filter.field}-${index}`} className="md-chip">
                        {chipLabel(filter, t)}
                      </span>
                    ))}
                    {appliedFilters(detail.filterCriteria).length === 0 ? (
                      <span className="md-chip">{t('lists.criteriaAll')}</span>
                    ) : null}
                  </div>
                  <div className="lists__smartbar" data-testid="lists-smartbar">
                    <Button
                      variant="tonal"
                      data-testid="lists-edit-criteria"
                      onClick={() => setShowSnapshot(true)}
                    >
                      {t('lists.editCriteria')}
                    </Button>
                    <Button variant="tonal" data-testid="lists-sync" onClick={sync}>
                      {t('lists.sync')}
                    </Button>
                  </div>
                </>
              ) : (
                <div className="md-card__row">
                  <Button
                    variant="tonal"
                    data-testid="lists-add-media"
                    onClick={() => setShowPick(true)}
                  >
                    {t('lists.addMedia')}
                  </Button>
                </div>
              )}

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
            </>
          )}
        </section>
      </div>

      {showSnapshot && activeId ? (
        <SnapshotBuilder
          listId={activeId}
          initial={detail?.filterCriteria ?? null}
          onClose={() => setShowSnapshot(false)}
        />
      ) : null}

      {showPick && activeId ? (
        <PickMediaModal listId={activeId} onClose={() => setShowPick(false)} />
      ) : null}
    </div>
  )
}

/**
 * A member tile in the §11.5 grid: the §10.6.5 tile's visual language (square
 * LQIP image, name + meta strip) with a per-tile `[✕ remove]` instead of the
 * gallery's check/fav overlay. Manual lists only — a smart list never renders
 * this grid, because its membership is a snapshot (§7.9).
 */
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
