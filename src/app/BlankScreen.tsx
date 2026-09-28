import { useI18n } from '../hooks/useI18n'
import type { ViewId } from './AppShell'

const TITLES: Record<ViewId, string> = {
  dashboard: 'nav.dashboard',
  media: 'nav.media',
  lists: 'nav.lists',
  backup: 'nav.backup',
  settings: 'nav.settings',
  vault: 'nav.vault',
}

export function BlankScreen({ view }: { view: ViewId }) {
  const { t } = useI18n()

  return (
    <section className="flex flex-col gap-card">
      <h1 className="text-headline-m text-fg">{t(TITLES[view])}</h1>
      <div className="rounded-lg bg-surface-ctr p-8 shadow-rest">
        <p className="text-body-m text-on-surface-variant">{t('shell.scaffoldNotice')}</p>
      </div>
    </section>
  )
}
