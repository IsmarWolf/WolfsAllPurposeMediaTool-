import { useI18n } from '../../hooks/useI18n'

export function Dashboard() {
  const { t } = useI18n()

  return (
    <section className="flex flex-col gap-card">
      <h1 className="text-headline-m text-fg">{t('nav.dashboard')}</h1>
      <div className="rounded-lg bg-surface-ctr p-8 shadow-rest">
        <p className="text-body-m text-on-surface-variant">{t('shell.scaffoldNotice')}</p>
      </div>
    </section>
  )
}
