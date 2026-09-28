import type { ButtonHTMLAttributes, ReactNode } from 'react'
import { useI18n } from '../../hooks/useContexts'

/** §19.1: a filled button for the single primary action, a tonal one for the rest. */
export function Button({
  children,
  variant = 'tonal',
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: 'filled' | 'tonal' | 'text' | 'outlined'
}) {
  return (
    <button type="button" className={`md-btn md-btn--${variant}`} {...rest}>
      {children}
    </button>
  )
}

/** §19.1 signature element: the pill. Every tag and every active chip is one. */
export function Chip({
  active = false,
  tone = 'neutral',
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  active?: boolean
  tone?: 'neutral' | 'accent' | 'warn' | 'danger'
}) {
  return (
    <button
      type="button"
      className={`md-chip md-chip--${tone}${active ? ' md-chip--active' : ''}`}
      aria-pressed={active}
      {...rest}
    >
      {children}
    </button>
  )
}

/** §19.1: a CSS-grid track of 1fr auto, with a hairline separator. */
export function PageHeader({
  title,
  subtitle,
  actions,
}: {
  title: string
  subtitle?: string
  actions?: ReactNode
}) {
  return (
    <header className="md-header">
      <div className="md-header__text">
        <h1>{title}</h1>
        {subtitle ? <p>{subtitle}</p> : null}
      </div>
      {actions ? <div className="md-header__actions">{actions}</div> : null}
    </header>
  )
}

/** §10.6.4 — the Error card of the five-state model, with its Retry button. */
export function ErrorCard({
  messageKey,
  detail,
  onRetry,
}: {
  messageKey: string
  detail?: string
  onRetry?: () => void
}) {
  const { t } = useI18n()
  return (
    <section className="md-error" role="alert">
      <p className="md-error__message">{t(messageKey)}</p>
      {detail ? <p className="md-error__detail">{detail}</p> : null}
      {onRetry ? (
        <Button variant="tonal" onClick={onRetry}>
          {t('common.retry')}
        </Button>
      ) : null}
    </section>
  )
}

/** §10.6.4 skeleton — never a spinner in the middle of the layout. */
export function SkeletonBlock({
  height = 16,
  width = '100%',
}: {
  height?: number
  width?: string
}) {
  return <div className="md-skeleton" style={{ height, width }} aria-hidden="true" />
}

export function EmptyState({ messageKey }: { messageKey: string }) {
  const { t } = useI18n()
  return <p className="md-empty">{t(messageKey)}</p>
}
