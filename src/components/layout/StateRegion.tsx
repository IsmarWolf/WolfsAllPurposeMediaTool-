import type { ReactNode } from 'react'
import { useI18n } from '../../hooks/useContexts'
import { useStateMachine, type StageInput } from '../../hooks/useStateMachine'
import { ErrorCard, SkeletonBlock } from '../ui'

/**
 * §10.6.4 — the one state region. All five branches are rendered from the state
 * derived by `useStateMachine`, so a screen physically cannot forget a branch.
 */
export function StateRegion<T>({
  stage,
  ideal,
  skeletonCount = 12,
  empty,
}: {
  stage: StageInput<T>
  ideal: ReactNode
  skeletonCount?: number
  /**
   * The empty branch's copy. Defaults to the media copy; the §11.6 Cofre passes
   * its own, because "Esta pasta está vazia / Importe mídia para começar" is a
   * lie for a vault — you do not import *into* it, you hide things out of the
   * gallery (§7.10 c19 decision 13).
   */
  empty?: { title: string; cta: string }
}) {
  const { t } = useI18n()
  const state = useStateMachine(stage)

  if (state === 'loading') {
    // Skeleton tiles mirroring the real grid geometry — no global spinner.
    return (
      <div className="state-region" data-state="loading" aria-busy="true">
        {Array.from({ length: skeletonCount }, (_, index) => (
          <SkeletonBlock key={index} height={140} />
        ))}
      </div>
    )
  }

  if (state === 'error') {
    return (
      <div className="state-region" data-state="error">
        <ErrorCard
          messageKey="error.unavailable"
          detail={stage.error ? String(stage.error) : undefined}
        />
      </div>
    )
  }

  if (state === 'empty') {
    return (
      <div className="state-region" data-state="empty">
        <p className="md-empty">{empty ? empty.title : t('state.empty')}</p>
        <p className="md-empty__cta">{empty ? empty.cta : t('state.emptyCta')}</p>
      </div>
    )
  }

  if (state === 'noresults') {
    return (
      <div className="state-region" data-state="noresults">
        <p className="md-empty">{t('state.noResults')}</p>
        <p className="md-empty__cta">{t('state.clearFilters')}</p>
      </div>
    )
  }

  return (
    <div className="state-region" data-state="ideal">
      {ideal}
    </div>
  )
}
