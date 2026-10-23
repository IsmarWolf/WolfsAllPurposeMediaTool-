import { useI18n } from '../../hooks/useContexts'

/**
 * §11.9 `BulkToolbar` — the Zone 5 floating bar, visible only when
 * `selection >= 1`. Primary actions on the left, the isolated destructive red on
 * the right (§10.6.2).
 *
 * c22: `[Z5b Adicionar à lista]` (§12.4, manual membership) and `[Z5f
 * Favoritar/Desfavoritar]` (§24.2) join the bar. Both are opt-in props — the
 * dashboard and lists surfaces pass neither, so the bar never grows a control
 * that cannot act there (the dead-control rule, as ever).
 *
 * c22 (second pass): `[Z5e Apagar]` — the wireframe's isolated destructive red
 * on the right (§11.9) — was rendered with **no handler** since c10, a dead
 * control that read as a working delete. It is an opt-in prop like the others,
 * and the shell behind it always confirms first (C7): the button only *opens*
 * the confirm, `media_remove` does the deleting (now including the file, §14).
 */
export function BulkToolbar({
  count,
  onClear,
  vaultConfigured = false,
  onHideSelected,
  onAddToList,
  onToggleFavorite,
  favoriteAllOn = false,
  onDelete,
}: {
  count: number
  onClear: () => void
  /** §11.9: `[Z5d Ocultar no cofre*]` is rendered only if the vault is configured. */
  vaultConfigured?: boolean
  onHideSelected?: () => void
  /** c22/§12.4 [Z5b]: opens the add-to-list modal for the current selection. */
  onAddToList?: () => void
  /**
   * c22/§24.2 [Z5f]: sets the flag on the whole selection. The label flips to
   * "Desfavoritar" only when *every* selected item is already a favorite;
   * a mixed selection flips everything on (the least surprising default —
   * a second click can always un-favorite).
   */
  onToggleFavorite?: () => void
  favoriteAllOn?: boolean
  /** c22/§11.9 [Z5e]: opens the destructive confirm for the selection. */
  onDelete?: () => void
}) {
  const { t } = useI18n()

  if (count < 1) {
    return null
  }

  return (
    <div className="zone5" role="toolbar" aria-label={t('bulk.label')}>
      <span className="zone5__count">{t('bulk.count', { count: String(count) })}</span>
      <button type="button" className="md-btn md-btn--text" onClick={onClear}>
        {t('bulk.clear')}
      </button>
      {onAddToList ? (
        <button
          type="button"
          className="md-btn md-btn--text"
          data-testid="z5b-add-list"
          onClick={onAddToList}
        >
          {t('lists.addTo')}
        </button>
      ) : null}
      {onToggleFavorite ? (
        <button
          type="button"
          className="md-btn md-btn--text"
          data-testid="z5f-fav"
          onClick={onToggleFavorite}
        >
          {favoriteAllOn ? t('media.unfavorite') : t('media.favorite')}
        </button>
      ) : null}
      {vaultConfigured ? (
        <button
          type="button"
          className="md-btn md-btn--text"
          data-testid="z5d-hide"
          onClick={onHideSelected}
        >
          {t('media.hideToVault')}
        </button>
      ) : null}
      <span className="zone5__spacer" />
      {onDelete ? (
        <button
          type="button"
          className="md-btn md-btn--text md-btn--danger"
          data-testid="z5e-delete"
          onClick={onDelete}
        >
          {t('bulk.delete')}
        </button>
      ) : null}
    </div>
  )
}
