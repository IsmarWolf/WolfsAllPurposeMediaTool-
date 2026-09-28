import { useI18n } from '../../hooks/useContexts'
import type { Route } from '../../routes'

/**
 * §11.0 Zone 1 — shell-only header: `[Z1a]` brand, `[Z1b]` Importar,
 * `[Z1c]` Ajustes, `[Z1d]` DB-health dot.
 *
 * **No filters here** (C16): the L1 row, the L2 drawer button and the chips
 * belong to the Mídia screen because they act only on the gallery.
 */
export function Header({
  route,
  onNavigate,
  dbHealthy,
  dbUnknown,
}: {
  route: Route
  onNavigate: (route: Route) => void
  dbHealthy: boolean
  dbUnknown: boolean
}) {
  const { t } = useI18n()

  return (
    <header className="zone1">
      <button
        type="button"
        className="zone1__brand"
        aria-label={t('app.name')}
        onClick={() => onNavigate('dashboard')}
      >
        {t('app.name')}
      </button>

      <div className="zone1__actions">
        <button
          type="button"
          className="md-btn md-btn--filled"
          onClick={() => onNavigate('backup')}
          aria-current={route === 'backup' ? 'page' : undefined}
        >
          {t('action.import')}
        </button>
        <button
          type="button"
          className="md-btn md-btn--text"
          onClick={() => onNavigate('settings')}
          aria-current={route === 'settings' ? 'page' : undefined}
        >
          {t('nav.settings')}
        </button>

        {/* [Z1d] — grey = unknown, green = answering, red = E_DB. */}
        <span
          data-testid="z1d-db-health"
          className={`zone1__db-dot${dbUnknown ? '' : dbHealthy ? ' zone1__db-dot--ok' : ' zone1__db-dot--bad'}`}
          role="status"
          title={
            dbUnknown ? t('appBar.dbUnknown') : dbHealthy ? t('appBar.dbOk') : t('appBar.dbFail')
          }
          aria-label={
            dbUnknown ? t('appBar.dbUnknown') : dbHealthy ? t('appBar.dbOk') : t('appBar.dbFail')
          }
        />
      </div>
    </header>
  )
}
