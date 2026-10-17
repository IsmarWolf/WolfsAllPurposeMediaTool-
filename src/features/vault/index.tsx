import { useCallback, useMemo, useState } from 'react'
import { Button, PageHeader, SkeletonBlock } from '../../components/ui'
import { StateRegion } from '../../components/layout/StateRegion'
import { useI18n, useToast } from '../../hooks/useContexts'
import { useSelection } from '../../hooks/useSelection'
import { useMediaQuery } from '../../hooks/useMediaQuery'
import { useVaultActions, useVaultStatus } from '../../hooks/useVault'
import { mediaRemove } from '../../lib/ipc'
import { toAppError, type AppError } from '../../lib/errors'
import { MediaGrid } from '../media/MediaGrid'
import { vaultThumbUrl } from '../media/assetUrl'

/**
 * §11.6 Cofre — a **full gate**, never a toast (C15).
 *
 * The nav item never blocks: with no config the screen *is* the first-run gate
 * (`[C-6d]` password ×2 + strength, then `[C-6e]` the one-time recovery key);
 * configured but locked it is the password gate (`[C-6a..c]`); unlocked it is the
 * Vault Mode grid (`[C-6f..j]`). The tiles are served by the session-gated
 * `wolfsvault` scheme (§11.6/C6), so locking mid-session blanks them.
 *
 * Vault Mode is a **separate surface**: its grid passes `scope: Vault` and owns
 * its own `useSelection`, exactly like `PickMediaModal` (§12.6) — the Mídia
 * selection can never leak across the gate and vice versa.
 */
export function VaultScreen() {
  const { t } = useI18n()
  const { errorText } = useToast()
  const { status, error, loading, reload } = useVaultStatus()

  return (
    <div className="screen">
      <PageHeader title={t('vault.title')} subtitle={t('vault.subtitle')} />

      {error ? (
        <section className="md-card md-card--wide">
          <p className="md-error__message" role="alert">
            {errorText(error)}
          </p>
        </section>
      ) : loading || !status ? (
        <section className="md-card md-card--wide" data-testid="vault-loading">
          <SkeletonBlock height={20} width="40%" />
          <SkeletonBlock height={52} width="70%" />
        </section>
      ) : !status.configured ? (
        <VaultSetupGate onDone={reload} />
      ) : !status.unlocked ? (
        <VaultLockedGate onDone={reload} />
      ) : (
        <VaultMode onLocked={reload} />
      )}
    </div>
  )
}

/* --------------------------------------------------------- first run ([C-6d/6e]) */

/** Password score, 0–4: length lanes plus the three character classes (§12.5.1). */
function passwordStrength(password: string): number {
  if (!password) return 0
  let score = 0
  if (password.length >= 8) score += 1
  if (password.length >= 12) score += 1
  if (/[a-z]/.test(password) && /[A-Z]/.test(password)) score += 1
  if (/\d/.test(password)) score += 1
  if (/[^A-Za-z0-9]/.test(password)) score += 1
  return Math.min(score, 4)
}

const STRENGTH_KEYS = [
  'vault.strengthWeak',
  'vault.strengthWeak',
  'vault.strengthFair',
  'vault.strengthGood',
  'vault.strengthStrong',
] as const

function VaultSetupGate({ onDone }: { onDone: () => void }) {
  const { t } = useI18n()
  const { push } = useToast()
  const { setup } = useVaultActions()
  const [password, setPassword] = useState('')
  const [confirm, setConfirm] = useState('')
  const [message, setMessage] = useState<string | null>(null)
  const [recoveryKey, setRecoveryKey] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const strength = passwordStrength(password)

  const submit = async () => {
    if (password.length < 8) {
      setMessage(t('vault.passwordTooShort'))
      return
    }
    if (password !== confirm) {
      setMessage(t('vault.passwordMismatch'))
      return
    }
    setBusy(true)
    const key = await setup(password)
    setBusy(false)
    if (key === null) {
      // `setup` already pushed the §8.2 toast for the failure — never a second
      // message for the same fact.
      return
    }
    setMessage(null)
    setRecoveryKey(key)
  }

  // §11.6: after setup the user must re-enter the password — `vault_setup` does
  // not open the session, so finishing returns to the (now configured) lock gate.
  if (recoveryKey) {
    return (
      <RecoveryKeyCard
        recoveryKey={recoveryKey}
        onContinue={() => {
          push('success', t('vault.setupDone'))
          onDone()
        }}
      />
    )
  }

  return (
    <section className="md-card md-card--wide vault-gate" data-testid="vault-setup">
      <h2>{t('vault.setupTitle')}</h2>
      <p className="md-hint">{t('vault.setupIntro')}</p>

      <label className="vault-field">
        <span>{t('vault.password')}</span>
        <input
          className="md-input"
          type="password"
          value={password}
          autoComplete="new-password"
          onChange={(e) => setPassword(e.target.value)}
        />
      </label>

      <div className="vault-strength" aria-label={t('vault.strengthLabel')}>
        {[0, 1, 2, 3].map((segment) => (
          <span
            key={segment}
            className={`vault-strength__seg${segment < strength ? ' vault-strength__seg--on' : ''}`}
          />
        ))}
        <span className="vault-strength__label">{t(STRENGTH_KEYS[strength])}</span>
      </div>

      <label className="vault-field">
        <span>{t('vault.passwordConfirm')}</span>
        <input
          className="md-input"
          type="password"
          value={confirm}
          autoComplete="new-password"
          onChange={(e) => setConfirm(e.target.value)}
        />
      </label>

      {message ? (
        <p className="md-modal__hint md-modal__hint--error" role="alert">
          {message}
        </p>
      ) : null}

      <div className="vault-actions">
        <Button variant="filled" onClick={submit} disabled={busy}>
          {t('vault.setupContinue')}
        </Button>
      </div>
    </section>
  )
}

/* ------------------------------------------------------- recovery key ([C-6e]) */

function RecoveryKeyCard({
  recoveryKey,
  onContinue,
}: {
  recoveryKey: string
  onContinue: () => void
}) {
  const { t } = useI18n()
  const { push } = useToast()

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(recoveryKey)
      push('success', t('vault.recoveryCopied'))
    } catch {
      push('warn', t('vault.recoveryCopyFailed'))
    }
  }

  const download = () => {
    try {
      const blob = new Blob([`${recoveryKey}\n`], { type: 'text/plain' })
      const url = URL.createObjectURL(blob)
      const anchor = document.createElement('a')
      anchor.href = url
      anchor.download = 'chave-de-recuperacao-cofre.txt'
      anchor.click()
      URL.revokeObjectURL(url)
      push('success', t('vault.recoveryDownloaded'))
    } catch {
      push('warn', t('vault.recoveryCopyFailed'))
    }
  }

  return (
    <section className="md-card md-card--wide vault-gate" data-testid="vault-recovery-key">
      <h2>{t('vault.recoveryTitle')}</h2>
      <p className="md-hint">{t('vault.recoveryIntro')}</p>
      <code className="vault-recovery__key">{recoveryKey}</code>
      <div className="vault-actions">
        <Button variant="outlined" onClick={copy}>
          {t('vault.recoveryCopy')}
        </Button>
        <Button variant="outlined" onClick={download}>
          {t('vault.recoveryDownload')}
        </Button>
      </div>
      <div className="vault-actions">
        <Button variant="filled" onClick={onContinue}>
          {t('vault.recoveryDone')}
        </Button>
      </div>
    </section>
  )
}

/* ---------------------------------------------------------- locked ([C-6a..c]) */

function VaultLockedGate({ onDone }: { onDone: () => void }) {
  const { t } = useI18n()
  const { push, errorText } = useToast()
  const { unlock, recover } = useVaultActions()
  const [password, setPassword] = useState('')
  const [error, setError] = useState<AppError | null>(null)
  const [shake, setShake] = useState(false)
  const [busy, setBusy] = useState(false)
  const [recoverOpen, setRecoverOpen] = useState(false)
  const [recoveryKey, setRecoveryKey] = useState('')
  const [recoverError, setRecoverError] = useState<AppError | null>(null)

  const submit = async () => {
    if (busy || password.length === 0) return
    setBusy(true)
    const failure = await unlock(password)
    setBusy(false)
    if (failure) {
      // §11.6: wrong password is inline `[E_AUTH]` + a shake — never a toast.
      setError(failure)
      setShake(true)
      window.setTimeout(() => setShake(false), 400)
      return
    }
    setError(null)
    push('success', t('vault.unlockedToast'))
    onDone()
  }

  const submitRecovery = async () => {
    if (recoveryKey.trim().length === 0) return
    const failure = await recover(recoveryKey.trim())
    if (failure) {
      setRecoverError(failure)
      return
    }
    setRecoverError(null)
    push('success', t('vault.unlockedToast'))
    onDone()
  }

  return (
    <section className="md-card md-card--wide vault-gate" data-testid="vault-locked">
      <h2>{t('vault.lockedTitle')}</h2>
      <p className="md-hint">{t('vault.lockedIntro')}</p>

      <label className="vault-field">
        <span>{t('vault.password')}</span>
        <input
          className={`md-input${shake ? ' vault-shake' : ''}`}
          type="password"
          value={password}
          autoComplete="current-password"
          aria-invalid={error ? true : undefined}
          onChange={(e) => {
            setPassword(e.target.value)
            setError(null)
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void submit()
          }}
        />
      </label>

      {error ? (
        <p className="md-modal__hint md-modal__hint--error" role="alert">
          {errorText(error)}
        </p>
      ) : null}

      <div className="vault-actions">
        <Button variant="filled" onClick={submit} disabled={busy || password.length === 0}>
          {t('vault.unlock')}
        </Button>
        <Button
          variant="text"
          onClick={() => {
            setRecoverOpen(true)
            setRecoverError(null)
          }}
        >
          {t('vault.forgot')}
        </Button>
      </div>

      {recoverOpen ? (
        <div className="vault-recover">
          <p className="md-hint">{t('vault.recoverIntro')}</p>
          <input
            className="md-input"
            type="text"
            value={recoveryKey}
            placeholder={t('vault.recoveryPlaceholder')}
            autoComplete="off"
            onChange={(e) => {
              setRecoveryKey(e.target.value)
              setRecoverError(null)
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void submitRecovery()
            }}
          />
          {recoverError ? (
            <p className="md-modal__hint md-modal__hint--error" role="alert">
              {errorText(recoverError)}
            </p>
          ) : null}
          <div className="vault-actions">
            <Button variant="tonal" onClick={submitRecovery} disabled={recoveryKey.trim() === ''}>
              {t('vault.recover')}
            </Button>
            <Button variant="text" onClick={() => setRecoverOpen(false)}>
              {t('common.cancel')}
            </Button>
          </div>
        </div>
      ) : null}
    </section>
  )
}

/* ------------------------------------------------------- Vault Mode ([C-6f..j]) */

/**
 * The unlocked surface. It is the only place a hidden row is read, and it reads
 * them through the same `useMediaQuery` the gallery uses, just seeded on
 * `scope: Vault` (§6.6 — the scope is explicit, never inferred).
 */
function VaultMode({ onLocked }: { onLocked: () => void }) {
  const { t } = useI18n()
  const { push, pushError } = useToast()
  const selection = useSelection()
  const query = useMediaQuery({ scope: 'vault' })
  const { lock, unhide } = useVaultActions()
  // The ids the confirm modal is about, captured when it opens, so a selection
  // change while it is up cannot retarget the delete.
  const [pendingDelete, setPendingDelete] = useState<string[] | null>(null)
  const [busy, setBusy] = useState(false)

  const items = useMemo(() => query.data?.items ?? [], [query.data])
  const total = query.data?.total ?? 0
  const count = selection.count

  const lockNow = useCallback(() => {
    void lock().then(() => onLocked())
  }, [lock, onLocked])

  const restore = async () => {
    if (count === 0 || busy) return
    setBusy(true)
    const restored = await unhide([...selection.selected])
    setBusy(false)
    if (restored === null) return
    push('success', t('vault.restored', { count: restored }))
    selection.clear()
    query.retry()
  }

  const confirmDelete = async () => {
    if (!pendingDelete) return
    setBusy(true)
    try {
      const removed = await mediaRemove(pendingDelete)
      push('success', t('vault.deleted', { count: removed }))
      selection.clear()
      query.retry()
    } catch (raw) {
      pushError(toAppError(raw))
    } finally {
      setBusy(false)
      setPendingDelete(null)
    }
  }

  return (
    <div className="vault-mode" data-testid="vault-mode">
      <div className="vault-banner" data-testid="c-6f-banner">
        <strong>{t('vault.modeActive')}</strong>
        <span className="vault-banner__count">{t('vault.hiddenCount', { count: total })}</span>
        <span className="vault-banner__spacer" />
        <Button variant="outlined" onClick={lockNow} data-testid="c-6j-lock">
          {t('vault.lock')}
        </Button>
      </div>

      <div className="vault-toolbar" role="toolbar" aria-label={t('vault.selectionLabel')}>
        <span className="vault-toolbar__count">{t('bulk.count', { count: String(count) })}</span>
        <Button
          variant="tonal"
          onClick={restore}
          disabled={count === 0 || busy}
          data-testid="c-6h-restore"
        >
          {t('vault.restore')}
        </Button>
        <span className="vault-toolbar__spacer" />
        <button
          type="button"
          className="md-btn md-btn--text md-btn--danger"
          onClick={() => setPendingDelete([...selection.selected])}
          disabled={count === 0 || busy}
          data-testid="c-6i-delete"
        >
          {t('vault.delete')}
        </button>
      </div>

      <StateRegion
        stage={{
          loading: query.loading,
          error: query.error,
          data: query.data,
          count: items.length,
          filtered: false,
        }}
        empty={{ title: t('vault.empty'), cta: t('vault.emptyCta') }}
        ideal={
          <MediaGrid
            items={items}
            root={null}
            onOpen={() => {}}
            selectionMode
            selection={selection}
            navEnabled
            thumbSrc={(item, hi) => vaultThumbUrl(hi ? item.thumb400 : item.thumb200)}
          />
        }
      />

      {query.data?.hasMore ? (
        <Button
          variant="text"
          onClick={query.loadMore}
          disabled={query.loading || query.loadingMore}
        >
          {t('media.loadMore')}
        </Button>
      ) : null}

      {pendingDelete ? (
        <div
          className="md-modal"
          role="dialog"
          aria-modal="true"
          aria-label={t('vault.deleteTitle')}
        >
          <div className="md-modal__backdrop" onClick={() => setPendingDelete(null)} />
          <div className="md-modal__panel">
            <h2>{t('vault.deleteTitle')}</h2>
            <p className="md-hint">{t('vault.deleteConfirm', { count: pendingDelete.length })}</p>
            <div className="md-modal__footer">
              <Button variant="text" onClick={() => setPendingDelete(null)}>
                {t('common.cancel')}
              </Button>
              <button
                type="button"
                className="md-btn md-btn--filled md-btn--danger"
                onClick={confirmDelete}
                disabled={busy}
              >
                {t('vault.delete')}
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  )
}
