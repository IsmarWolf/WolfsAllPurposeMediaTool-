import { useState } from 'react'
import { PageHeader } from '../../components/ui'
import { useI18n } from '../../hooks/useContexts'

type Source = 'usb' | 'wifi'

/**
 * §11.4/§12.1-12.2 Backup — **one** page with USB | Wi-Fi as source tabs
 * (C-15). The tabs are the transport, not separate screens; the ingest machinery
 * is c11/c12, so c4 proves the tab contract only.
 */
export function BackupScreen() {
  const { t } = useI18n()
  const [source, setSource] = useState<Source>('usb')

  return (
    <div className="screen">
      <PageHeader title={t('nav.backup')} subtitle={t('backup.subtitle')} />

      <div className="source-tabs" role="tablist" aria-label={t('backup.source')}>
        {(['usb', 'wifi'] as const).map((entry) => (
          <button
            key={entry}
            type="button"
            role="tab"
            aria-selected={source === entry}
            className={`source-tabs__tab${source === entry ? ' source-tabs__tab--active' : ''}`}
            onClick={() => setSource(entry)}
          >
            {t(entry === 'usb' ? 'backup.usb' : 'backup.wifi')}
          </button>
        ))}
      </div>

      <p className="screen__todo">
        {t(source === 'usb' ? 'backup.usbComing' : 'backup.wifiComing')}
      </p>
    </div>
  )
}
