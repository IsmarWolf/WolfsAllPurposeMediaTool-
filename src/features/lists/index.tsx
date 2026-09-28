import { PageHeader } from '../../components/ui'
import { useI18n } from '../../hooks/useContexts'

/** §11.3 Listas — smart lists are c15. Persisted filters belong here only (§10.6.6). */
export function ListsScreen() {
  const { t } = useI18n()

  return (
    <div className="screen">
      <PageHeader title={t('nav.lists')} subtitle={t('lists.subtitle')} />
      <p className="screen__todo">{t('lists.comingInC15')}</p>
    </div>
  )
}
