import { useState } from 'react'
import { Button } from '../../components/ui'
import { useI18n, useToast } from '../../hooks/useContexts'
import { useListActions, useLists } from '../../hooks/useLists'
import { formatCount } from '../../lib/format'
import type { FilterSpec } from '../../types/api'

/**
 * §12.4 `[Aplicar à lista ▾]` — the gallery's smart-snapshot flow.
 *
 * Picks an existing list or creates one, then `lists_snapshot_apply` reads the
 * **active** spec, runs it now, and replaces the list's membership. A manual
 * list that receives a snapshot becomes smart (criteria stored, §12.4); a smart
 * list is overwritten (c15 decision 4). Rendered lazily by `MediaScreen`, so its
 * `lists_list` read only happens when the human opens it.
 */
export function ApplyToList({ spec, onClose }: { spec: FilterSpec; onClose: () => void }) {
  const { t } = useI18n()
  const { push } = useToast()
  const { lists, loading, error } = useLists()
  const { create, applySnapshot } = useListActions()
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)

  const apply = async (listId: string) => {
    setBusy(true)
    const result = await applySnapshot(listId, spec)
    setBusy(false)
    if (result !== null) {
      push('success', t('lists.applied'))
      onClose()
    }
  }

  const createAndApply = async () => {
    const trimmed = name.trim()
    if (!trimmed || busy) {
      return
    }
    setBusy(true)
    const created = await create(trimmed)
    if (created) {
      const result = await applySnapshot(created.id, spec)
      if (result !== null) {
        push('success', t('lists.applied'))
        onClose()
        return
      }
    }
    setBusy(false)
  }

  return (
    <div className="md-modal" role="dialog" aria-modal="true" aria-label={t('lists.applyToList')}>
      <div className="md-modal__backdrop" onClick={onClose} />
      <div className="md-modal__panel">
        <div className="md-drawer__header">
          <h2>{t('lists.applyToList')}</h2>
          <button type="button" className="md-btn md-btn--text" onClick={onClose}>
            {t('common.close')}
          </button>
        </div>

        <p className="md-modal__hint">{t('lists.applyHint')}</p>

        <div className="md-drawer__section">
          <h3>{t('lists.newList')}</h3>
          <div className="lists__newrow">
            <input
              type="text"
              className="md-input"
              placeholder={t('lists.namePlaceholder')}
              value={name}
              aria-label={t('lists.newList')}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  void createAndApply()
                }
              }}
            />
            <Button
              variant="filled"
              onClick={createAndApply}
              disabled={busy || name.trim().length === 0}
            >
              {t('lists.createAndApply')}
            </Button>
          </div>
        </div>

        <div className="md-drawer__section">
          <h3>{t('lists.existing')}</h3>
          {error ? (
            <p className="md-modal__hint md-modal__hint--error">{t('lists.loadError')}</p>
          ) : loading ? (
            <p className="md-modal__hint">{t('lists.loading')}</p>
          ) : lists.length === 0 ? (
            <p className="md-modal__hint">{t('lists.none')}</p>
          ) : (
            <ul className="lists__pick">
              {lists.map((list) => (
                <li key={list.id}>
                  <button
                    type="button"
                    className="lists__pick-item"
                    disabled={busy}
                    onClick={() => void apply(list.id)}
                  >
                    <span>{list.name}</span>
                    <span className="lists__pick-meta">
                      {list.filterCriteria ? t('lists.kindSmart') : t('lists.kindManual')}
                      {' · '}
                      {formatCount(list.memberCount)}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </div>
  )
}
