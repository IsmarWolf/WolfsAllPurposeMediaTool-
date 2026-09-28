import { useEffect, useState } from 'react'
import { PageHeader } from '../../components/ui'
import { useI18n } from '../../hooks/useContexts'
import { vaultStatus } from '../../lib/ipc'
import type { VaultStatusDto } from '../../types/api'

/**
 * §11.6 Cofre — a **full gate**, never a toast (C15). c4 renders the gate's
 * locked state with the real `vault_config` read; setup/unlock is c18.
 */
export function VaultScreen() {
  const { t } = useI18n()
  const [status, setStatus] = useState<VaultStatusDto | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let alive = true
    void vaultStatus()
      .then((next) => {
        if (alive) {
          setStatus(next)
        }
      })
      .catch((raw: unknown) => {
        if (alive) {
          setError(raw instanceof Error ? raw.message : String(raw))
        }
      })
    return () => {
      alive = false
    }
  }, [])

  return (
    <div className="screen screen--gate">
      <PageHeader title={t('vault.title')} subtitle={t('vault.subtitle')} />

      <section className="md-card md-card--wide" data-testid="vault-gate">
        {error ? (
          <p className="md-error__message" role="alert">
            {error}
          </p>
        ) : null}

        <p className="vault-gate__status">
          {status?.configured ? t('vault.configured') : t('vault.notConfigured')}
        </p>
        <p className="md-hint">{t('vault.comingInC18')}</p>
      </section>
    </div>
  )
}
