import { PageHeader, SkeletonBlock } from '../../components/ui'
import { useI18n } from '../../hooks/useContexts'

/**
 * §11.2 Mídia — the gallery is c8. What c4 must prove is the *shell* contract:
 * the page owns its own filter surface (L1 row + L2 drawer button + chips live
 * here, §11.2/C16) and the grid is the Zone-3 content, not the header.
 */
export function MediaScreen() {
  const { t } = useI18n()

  return (
    <div className="screen">
      <PageHeader title={t('nav.media')} subtitle={t('media.subtitle')} />

      {/* [Z3g-i] L1 filter row — a Mídia concern, never a Zone 1 one. */}
      <div className="l1-row" role="group" aria-label={t('media.filtersL1')}>
        <span className="l1-row__label">{t('media.filtersL1')}</span>
        <button type="button" className="md-btn md-btn--text" disabled>
          {t('media.facetLocation')}
        </button>
        <button type="button" className="md-btn md-btn--text" disabled>
          {t('media.facetMonth')}
        </button>
        <button type="button" className="md-btn md-btn--text" disabled>
          {t('media.facetNoMetadata')}
        </button>
        {/* [Z3j] L2 drawer button. */}
        <button type="button" className="md-btn md-btn--tonal" disabled>
          {t('media.filtersL2')}
        </button>
      </div>

      <div className="media-grid">
        {Array.from({ length: 8 }, (_, index) => (
          <SkeletonBlock key={index} height={160} />
        ))}
      </div>
      <p className="screen__todo">{t('media.comingInC8')}</p>
    </div>
  )
}
