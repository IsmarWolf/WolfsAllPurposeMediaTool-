import { useState } from 'react'
import { PageHeader } from '../../components/ui'
import { useAsync, useTauriCommand } from '../../hooks/useAsync'
import { useI18n, useToast } from '../../hooks/useContexts'
import {
  appVersions,
  resolveAppRoot,
  settingsRecalcRoot,
  settingsRevealRoot,
  settingsVacuum,
} from '../../lib/ipc'
import type { AppVersionsDto, RecalcRootDto, RootInfoDto } from '../../types/api'

/**
 * §11.7 Sistema — the only screen c4 implements. It is also the telemetry home
 * (C16): root path, root source, first run, ffmpeg, DB compact, app version.
 */
export function SettingsScreen() {
  const { t } = useI18n()
  const { push, errorText } = useToast()

  const root = useAsync<RootInfoDto>(() => resolveAppRoot(), [])
  const versions = useAsync<AppVersionsDto>(() => appVersions(), [])
  // A recalculation can move the root, so its result wins over the boot value
  // until the next reload.
  const [recalculated, setRecalculated] = useState<RecalcRootDto | null>(null)

  const info = recalculated ?? root.data
  const reveal = useTauriCommand(async () => settingsRevealRoot())
  const vacuum = useTauriCommand(async () => settingsVacuum())
  const recalc = useTauriCommand(async () => {
    const result = await settingsRecalcRoot()
    setRecalculated(result)
    push('success', result.changed ? t('settings.recalcChanged') : t('settings.recalcSame'))
  })

  return (
    <div className="screen">
      <PageHeader title={t('nav.settings')} subtitle={t('settings.subtitle')} />

      <section className="md-card md-card--wide" data-testid="settings-origin">
        <h2>{t('settings.originTitle')}</h2>

        {root.error ? (
          <p className="md-error__message" role="alert">
            {/* `errorText`, not `i18nKey ?? detail`: the key still has to reach
                `t`, and the detail is technical text (§8.2). */}
            {errorText(root.error)}
          </p>
        ) : null}

        <dl className="kv">
          <dt>{t('settings.rootPath')}</dt>
          <dd>
            <code data-testid="settings-root-path">{info?.root ?? '—'}</code>
          </dd>

          <dt>{t('settings.rootSource')}</dt>
          <dd data-testid="settings-root-source">{info ? t(`root.${info.source}`) : '—'}</dd>

          <dt>{t('settings.firstRun')}</dt>
          <dd>{info ? (info.firstRun ? t('common.yes') : t('common.no')) : '—'}</dd>

          <dt>{t('settings.ffmpeg')}</dt>
          <dd>
            <span className={`md-badge${info?.ffmpegOk ? ' md-badge--ok' : ' md-badge--warn'}`}>
              {info?.ffmpegOk ? t('dash.ffmpegOk') : t('dash.ffmpegMissing')}
            </span>
          </dd>

          <dt>{t('settings.appVersion')}</dt>
          <dd>
            {versions.data?.appVersion ?? '—'} · {versions.data?.os ?? '?'} ·{' '}
            {versions.data?.arch ?? '?'}
          </dd>
        </dl>

        {/* §5.6 — the root actions, in the order the wireframe lists them. */}
        <div className="md-card__row">
          <button
            type="button"
            className="md-btn md-btn--tonal"
            disabled={reveal.busy}
            onClick={() => void reveal.run()}
          >
            {t('settings.reveal')}
          </button>
          <button
            type="button"
            className="md-btn md-btn--tonal"
            disabled={recalc.busy}
            onClick={() => void recalc.run()}
          >
            {recalc.busy ? t('common.working') : t('settings.recalc')}
          </button>
          <button
            type="button"
            className="md-btn md-btn--tonal"
            disabled={vacuum.busy}
            onClick={() =>
              void vacuum.run().then((ok) => {
                // The command's failure is already a toast; claiming "done" on
                // top of it would be a second, contradictory message.
                if (ok) {
                  push('success', t('settings.vacuumDone'))
                }
              })
            }
          >
            {vacuum.busy ? t('common.working') : t('settings.vacuum')}
          </button>
        </div>
      </section>
    </div>
  )
}
