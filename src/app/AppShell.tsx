import {
  Gauge,
  HardDriveDownload,
  Images,
  ListChecks,
  Lock,
  Settings,
  Upload,
  type LucideIcon,
} from 'lucide-react'
import { useState } from 'react'
import { Dashboard } from '../features/dashboard'
import { useI18n } from '../hooks/useI18n'
import { useSkin } from '../hooks/useSkin'
import { BlankScreen } from './BlankScreen'

export type ViewId = 'dashboard' | 'media' | 'lists' | 'backup' | 'settings' | 'vault'

type NavItem = {
  id: ViewId
  labelKey: string
  icon: LucideIcon
}

type NavGroup = {
  labelKey: string
  items: NavItem[]
}

const NAV_GROUPS: NavGroup[] = [
  {
    labelKey: 'nav.group.explorar',
    items: [
      { id: 'dashboard', labelKey: 'nav.dashboard', icon: Gauge },
      { id: 'media', labelKey: 'nav.media', icon: Images },
      { id: 'lists', labelKey: 'nav.lists', icon: ListChecks },
    ],
  },
  {
    labelKey: 'nav.group.transferir',
    items: [{ id: 'backup', labelKey: 'nav.backup', icon: HardDriveDownload }],
  },
  {
    labelKey: 'nav.group.sistema',
    items: [{ id: 'settings', labelKey: 'nav.settings', icon: Settings }],
  },
]

const VAULT_ITEM: NavItem = { id: 'vault', labelKey: 'nav.vault', icon: Lock }

export function AppShell() {
  const { t } = useI18n()
  const { skin } = useSkin()
  const [view, setView] = useState<ViewId>('dashboard')

  return (
    <div data-skin={skin} className="flex h-screen flex-col overflow-hidden bg-bg">
      <header className="flex h-hit shrink-0 items-center justify-between bg-surface-ctr px-6">
        <button
          type="button"
          onClick={() => setView('dashboard')}
          className="flex h-hit items-center gap-3 rounded-pill px-4 text-title-l text-primary transition-colors duration-micro hover:bg-primary/5 active:scale-95"
        >
          <span className="h-6 w-6 rounded-sm bg-primary" aria-hidden="true" />
          {t('app.name')}
        </button>

        <div className="flex items-center gap-3">
          <button
            type="button"
            onClick={() => setView('backup')}
            className="flex h-icon-btn items-center gap-2 rounded-pill bg-primary px-6 text-label-m text-on-primary shadow-rest transition-all duration-standard ease-emphasized hover:shadow-hover active:scale-95"
          >
            <Upload size={18} aria-hidden="true" />
            {t('shell.import')}
          </button>
          <button
            type="button"
            onClick={() => setView('settings')}
            aria-label={t('nav.settings')}
            className="flex h-icon-btn w-icon-btn items-center justify-center rounded-pill text-primary transition-colors duration-micro hover:bg-primary/5 active:scale-95"
          >
            <Settings size={20} aria-hidden="true" />
          </button>
        </div>
      </header>

      <div className="flex min-h-0 flex-1">
        <nav
          aria-label={t('shell.zone.sidebar')}
          className="flex w-64 shrink-0 flex-col gap-6 bg-surface-ctr px-4 py-6"
        >
          <div className="flex flex-col gap-6">
            {NAV_GROUPS.map((group) => (
              <div key={group.labelKey} className="flex flex-col gap-1">
                <span className="px-4 pb-1 text-label-s text-on-surface-variant">
                  {t(group.labelKey)}
                </span>
                {group.items.map((item) => (
                  <NavButton
                    key={item.id}
                    item={item}
                    active={view === item.id}
                    onSelect={setView}
                  />
                ))}
              </div>
            ))}
          </div>

          <div className="mt-auto flex flex-col gap-1 border-t border-dashed border-outline-ctr pt-4">
            <span className="px-4 pb-1 text-label-s text-on-surface-variant">
              {t('nav.group.seguranca')}
            </span>
            <NavButton item={VAULT_ITEM} active={view === VAULT_ITEM.id} onSelect={setView} />
          </div>
        </nav>

        <main
          aria-label={t('shell.zone.canvas')}
          className="min-w-0 flex-1 overflow-y-auto bg-bg p-8"
        >
          {view === 'dashboard' ? <Dashboard /> : <BlankScreen view={view} />}
        </main>
      </div>
    </div>
  )
}

type NavButtonProps = {
  item: NavItem
  active: boolean
  onSelect: (view: ViewId) => void
}

function NavButton({ item, active, onSelect }: NavButtonProps) {
  const { t } = useI18n()
  const Icon = item.icon
  const isVault = item.id === 'vault'

  const base = 'flex h-hit w-full items-center gap-3 rounded-pill px-4 text-label-m transition-all duration-standard ease-emphasized active:scale-95'
  const tone = isVault
    ? active
      ? 'bg-error-ctr text-on-error-ctr'
      : 'border border-dashed border-error/70 text-error hover:bg-error/5'
    : active
      ? 'bg-secondary-ctr text-on-secondary-ctr'
      : 'text-on-surface-variant hover:bg-primary/5'

  return (
    <button type="button" onClick={() => onSelect(item.id)} className={`${base} ${tone}`}>
      <Icon size={20} aria-hidden="true" />
      {t(item.labelKey)}
    </button>
  )
}
