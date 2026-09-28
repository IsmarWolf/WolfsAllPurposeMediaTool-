import { useI18n } from '../../hooks/useContexts'
import type { Route } from '../../routes'

type NavItem = {
  route: Route
  labelKey: string
  /** The Mídia badge is the one live datum Zone 2 is allowed to show (C16). */
  badge?: number
}

type NavGroup = { headingKey: string; items: NavItem[] }

/**
 * §11.0 Zone 2 — grouped nav and nothing else (C16): Explorar / Transferir /
 * Sistema, then a divider, then the **detached** Segurança → Cofre, bottom
 * anchored, dashed danger keyline, `?` + lock-state dot. It is never disabled:
 * it routes to the §11.6 gate, it never blocks.
 */
export function Sidebar({
  route,
  onNavigate,
  mediaCount,
  vaultLocked,
}: {
  route: Route
  onNavigate: (route: Route) => void
  mediaCount: number | null
  vaultLocked: boolean
}) {
  const { t } = useI18n()

  const groups: NavGroup[] = [
    {
      headingKey: 'nav.explore',
      items: [
        { route: 'dashboard', labelKey: 'nav.home' },
        { route: 'media', labelKey: 'nav.media', badge: mediaCount ?? undefined },
        { route: 'lists', labelKey: 'nav.lists' },
      ],
    },
    {
      headingKey: 'nav.transfer',
      items: [{ route: 'backup', labelKey: 'nav.backup' }],
    },
    {
      headingKey: 'nav.system',
      items: [{ route: 'settings', labelKey: 'nav.settings' }],
    },
  ]

  return (
    <nav className="zone2" aria-label={t('nav.primary')}>
      {groups.map((group) => (
        <div key={group.headingKey} className="zone2__group">
          <h2 className="zone2__heading">{t(group.headingKey)}</h2>
          {group.items.map((item) => (
            <button
              key={item.route}
              type="button"
              data-testid="nav-item"
              className={`zone2__item${route === item.route ? ' zone2__item--active' : ''}`}
              aria-current={route === item.route ? 'page' : undefined}
              onClick={() => onNavigate(item.route)}
            >
              <span>{t(item.labelKey)}</span>
              {item.badge !== undefined ? <span className="zone2__badge">{item.badge}</span> : null}
            </button>
          ))}
        </div>
      ))}

      <div className="zone2__divider" />

      {/* SEGURANÇA — always detached, always last, never blocked. */}
      {/* SEGURANÇA - always detached, always last, never blocked. The `›` is the
          disclosure marker of §11.0: the PLAN wireframe draws it as a mojibake
          `?`, but it is a glyph, never a question mark in the UI. */}
      <div className="zone2__group zone2__group--security">
        <h2 className="zone2__heading">{t('nav.security')}</h2>
        <button
          type="button"
          data-testid="nav-item"
          className={`zone2__item zone2__item--vault${route === 'vault' ? ' zone2__item--active' : ''}`}
          aria-current={route === 'vault' ? 'page' : undefined}
          onClick={() => onNavigate('vault')}
        >
          <span className="zone2__marker" aria-hidden="true">
            ›
          </span>
          <span>{t('nav.vault')}</span>
          <span
            className={`zone2__lock-dot${vaultLocked ? ' zone2__lock-dot--locked' : ''}`}
            aria-label={vaultLocked ? t('vault.locked') : t('vault.unlocked')}
          />
        </button>
      </div>
    </nav>
  )
}
