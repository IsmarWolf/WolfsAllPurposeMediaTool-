import { useState } from 'react'
import { Button } from '../../components/ui'
import { useI18n, useToast } from '../../hooks/useContexts'
import { useListActions, useLists } from '../../hooks/useLists'
import { formatCount } from '../../lib/format'

/**
 * §12.4 (c22 rewrite) `[Adicionar à lista]` — **manual membership only**.
 *
 * Picks an existing list or creates one, then `lists_add_items` appends the
 * given ids. The smart-snapshot flow it replaces (`ApplyToList`, §24.1) is
 * retired from the UI: the backend commands still exist and the frontend
 * bindings stay callable (deprecated), but nothing in the app offers them —
 * a list's membership is now only ever hand-picked, with the one undo the
 * §12.4 front-end toast carries.
 *
 * Rendered lazily by the shell (both the `[Z3d]` header button and `[Z5b]`
 * route through it), so the `lists_list` read only happens when the modal is
 * actually open.
 */
export function AddToListModal({ ids, onClose }: { ids: string[]; onClose: () => void }) {
  const { t } = useI18n()
  const { push } = useToast()
  const { lists, loading, error } = useLists()
  const { create, addItems, removeItems } = useListActions()
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)

  const finish = (listId: string, listName: string) => {
    // §12.4: the undo is front-end only — it removes the same ids it just
    // added. `lists_remove_items` is the inverse command, so no backend
    // "undo token" exists or is needed.
    push('success', t('lists.addedTo', { count: ids.length, name: listName }), {
      label: t('common.undo'),
      run: () => {
        void removeItems(listId, ids)
      },
    })
    onClose()
  }

  const add = async (listId: string, listName: string) => {
    if (busy) return
    setBusy(true)
    const result = await addItems(listId, ids)
    setBusy(false)
    if (result !== null) {
      finish(listId, listName)
    }
  }

  const createAndAdd = async () => {
    const trimmed = name.trim()
    if (!trimmed || busy) {
      return
    }
    setBusy(true)
    const created = await create(trimmed)
    if (created) {
      const result = await addItems(created.id, ids)
      setBusy(false)
      if (result !== null) {
        finish(created.id, created.name)
        return
      }
      return
    }
    setBusy(false)
  }

  return (
    <div className="md-modal" role="dialog" aria-modal="true" aria-label={t('lists.addTo')}>
      <div className="md-modal__backdrop" onClick={onClose} />
      <div className="md-modal__panel">
        <div className="md-drawer__header">
          <h2>{t('lists.addTo')}</h2>
          <button type="button" className="md-btn md-btn--text" onClick={onClose}>
            {t('common.close')}
          </button>
        </div>

        <p className="md-modal__hint">{t('lists.addHint', { count: ids.length })}</p>

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
                  void createAndAdd()
                }
              }}
            />
            <Button
              variant="filled"
              onClick={createAndAdd}
              disabled={busy || name.trim().length === 0}
            >
              {t('lists.createAndAdd')}
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
                    onClick={() => void add(list.id, list.name)}
                  >
                    <span>{list.name}</span>
                    <span className="lists__pick-meta">{formatCount(list.memberCount)}</span>
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
