import { useState } from 'react'
import { StateRegion } from '../../components/layout/StateRegion'
import { Button, ErrorCard, PageHeader, SkeletonBlock } from '../../components/ui'
import { useI18n } from '../../hooks/useContexts'
import { useRootInfo } from '../../hooks/useTauriEvent'
import { formatBytes, formatCount, ratioOf } from '../../lib/format'
import type { Route } from '../../routes'
import type { RootInfoDto, StatsDto } from '../../types/api'

type Props = {
  stats: StatsDto | null
  loading: boolean
  error: unknown
  onRetry: () => void
  resolve: () => Promise<RootInfoDto>
  onNavigate: (route: Route) => void
}

/**
 * §11.1 Dashboard — the telemetry home (C16: no meter, no filter presets, no
 * live data in the sidebar; all of it lives here).
 *
 * C-1a..1d stat cards, C-1f vault mini-card, C-1h primary CTA, C-1i quick
 * actions, C-1j the QA-only state-machine triggers, C-1k the full-width
 * storage & system card.
 */
export function DashboardScreen({ stats, loading, error, onRetry, resolve, onNavigate }: Props) {
  const { t } = useI18n()
  const root = useRootInfo(resolve)
  const [demo, setDemo] = useState<'ideal' | 'empty' | 'noresults' | 'loading' | 'error' | null>(
    null,
  )

  return (
    <div className="screen">
      <PageHeader
        title={t('dash.title')}
        actions={
          <Button variant="filled" onClick={() => onNavigate('backup')}>
            {t('action.ingest')}
          </Button>
        }
      />

      {demo ? (
        <section className="dash-demo" data-testid="c-1j-demo">
          <h2>{t('dash.demoTitle')}</h2>
          <StateRegion
            stage={{
              loading: demo === 'loading',
              error: demo === 'error' ? 'E_DB' : null,
              data: demo === 'empty' || demo === 'noresults' ? [] : ['demo'],
              count: demo === 'empty' || demo === 'noresults' ? 0 : 1,
              filtered: demo === 'noresults',
            }}
            ideal={
              <div className="dash-demo__grid">
                {Array.from({ length: 6 }, (_, index) => (
                  <SkeletonBlock key={index} height={140} />
                ))}
              </div>
            }
          />
          <Button variant="text" onClick={() => setDemo(null)}>
            {t('dash.demoClose')}
          </Button>
        </section>
      ) : null}

      <div className="dash-cols">
        <div className="dash-cols__main">
          {error ? (
            <ErrorCard messageKey="error.E_DB" onRetry={onRetry} />
          ) : (
            <div className="dash-stats" data-testid="c-1a-stats">
              <StatCard
                label={t('dash.totalItems')}
                value={stats ? formatCount(stats.totalItems) : null}
                hint={stats ? t('dash.totalItemsHint') : null}
                busy={loading}
              />
              <StatCard
                label={t('dash.images')}
                value={stats ? formatCount(stats.images) : null}
                hint={stats ? formatBytes(stats.imagesBytes) : null}
                busy={loading}
              />
              <StatCard
                label={t('dash.videos')}
                value={stats ? formatCount(stats.videos) : null}
                hint={stats ? formatBytes(stats.videosBytes) : null}
                busy={loading}
              />
              {/* C-7: "sem metadados" = no capture date AND no GPS. */}
              <StatCard
                label={t('dash.noMetadata')}
                value={stats ? formatCount(stats.noMetadata) : null}
                hint={t('dash.noMetadataHint')}
                busy={loading}
                onClick={() => onNavigate('media')}
              />
            </div>
          )}
        </div>

        <div className="dash-cols__side">
          <section className="md-card" data-testid="c-1f-vault">
            <h2>{t('dash.vaultTitle')}</h2>
            <p>{t('dash.vaultLocked')}</p>
            <Button variant="tonal" onClick={() => onNavigate('vault')}>
              {t('dash.vaultOpen')}
            </Button>
          </section>

          <section className="md-card" data-testid="c-1i-quick">
            <h2>{t('dash.quickTitle')}</h2>
            <div className="md-card__row">
              <Button variant="tonal" onClick={() => onNavigate('backup')}>
                {t('dash.quickScanPc')}
              </Button>
              <Button variant="tonal" onClick={() => onNavigate('backup')}>
                {t('dash.quickIphone')}
              </Button>
              <Button variant="tonal" onClick={() => onNavigate('media')}>
                {t('dash.quickThumbs')}
              </Button>
            </div>
          </section>
        </div>
      </div>

      {/* C-1k — full-width, below dash-cols: the meter, the ffmpeg badge, the
          resolved root path. This is the only place telemetry lives. */}
      <section className="md-card md-card--wide" data-testid="c-1k-system">
        <h2>{t('dash.systemTitle')}</h2>

        <div className="dash-meter" aria-label={t('dash.meterLabel')}>
          <MeterRow
            label={t('dash.images')}
            value={stats ? stats.imagesBytes : null}
            total={stats?.totalBytes ?? 0}
            tone="images"
          />
          <MeterRow
            label={t('dash.videos')}
            value={stats ? stats.videosBytes : null}
            total={stats?.totalBytes ?? 0}
            tone="videos"
          />
          <MeterRow
            label={t('dash.hidden')}
            value={stats ? stats.hidden : null}
            total={stats?.totalItems ?? 0}
            tone="hidden"
            count
          />
        </div>

        <div className="dash-system__row">
          <span className={`md-badge${root?.ffmpegOk ? ' md-badge--ok' : ' md-badge--warn'}`}>
            {root?.ffmpegOk ? t('dash.ffmpegOk') : t('dash.ffmpegMissing')}
          </span>
          <code className="dash-system__root" title={root?.root ?? ''}>
            {root?.root ?? t('appBar.rootUnknown')}
          </code>
        </div>
      </section>

      {/* C-1j — QA-only affordance to smoke each branch of §10.6.4. */}
      <section className="dash-dev" data-testid="c-1j-states">
        <h2>{t('dash.demoTitle')}</h2>
        <div className="md-card__row">
          {(['ideal', 'empty', 'noresults', 'loading', 'error'] as const).map((state) => (
            <Button key={state} variant="text" onClick={() => setDemo(state)}>
              {state}
            </Button>
          ))}
        </div>
      </section>
    </div>
  )
}

function StatCard({
  label,
  value,
  hint,
  busy = false,
  onClick,
}: {
  label: string
  value: string | null
  hint?: string | null
  busy?: boolean
  onClick?: () => void
}) {
  const { t } = useI18n()

  return (
    <article className={`md-card md-stat${onClick ? ' md-stat--action' : ''}`}>
      <h3 className="md-stat__label">{label}</h3>
      {/* While the first read is in flight the number is a skeleton, not a 0:
          a 0 here would be a lie and it would blink. */}
      {value ? (
        <p className="md-stat__value">{value}</p>
      ) : (
        <SkeletonBlock height={32} width="60%" />
      )}
      {hint ? <p className="md-stat__hint">{hint}</p> : null}
      {busy ? <span className="visually-hidden">{t('dash.loading')}</span> : null}
      {onClick ? (
        <Button variant="text" onClick={onClick}>
          {label}
        </Button>
      ) : null}
    </article>
  )
}

function MeterRow({
  label,
  value,
  total,
  tone,
  count = false,
}: {
  label: string
  /** `null` while the first read is in flight: never render a fake 0. */
  value: number | null
  total: number
  tone: 'images' | 'videos' | 'hidden'
  count?: boolean
}) {
  const width = value === null ? '0%' : `${(ratioOf(value, total) * 100).toFixed(1)}%`

  return (
    <div className="dash-meter__row">
      <span className="dash-meter__label">{label}</span>
      <div className="md-meter">
        <div className={`md-meter__fill md-meter__fill--${tone}`} style={{ width }} />
      </div>
      <span className="dash-meter__value">
        {value === null ? '—' : count ? formatCount(value) : formatBytes(value)}
      </span>
    </div>
  )
}
