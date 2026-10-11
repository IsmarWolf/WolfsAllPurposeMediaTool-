import { Button } from '../../components/ui'
import { useI18n } from '../../hooks/useContexts'
import type { useUsbIngest } from '../../hooks/useUsbIngest'
import { formatCount, ratioOf } from '../../lib/format'

export type UsbIngest = ReturnType<typeof useUsbIngest>

/**
 * §11.3 Backup → USB, §12.1's `[C-3e Conectar]` → `[C-3f]` → `[C-3j Iniciar
 * importação]` / `[C-3k Parar]`.
 *
 * Presentational on purpose: `useUsbIngest` is mounted by the Backup screen and
 * passed in, because the job lives in the Rust `JobRegistry` (§9.3) and its events
 * must keep arriving while another tab is shown.
 *
 * Copy (pt-BR) is the UI's language; the code, keys and comments are English.
 */
export function UsbPane({ ingest }: { ingest: UsbIngest }) {
  const { t } = useI18n()
  const {
    devices,
    device,
    loadingDevices,
    error,
    stale,
    reloading,
    reload,
    select,
    mode,
    setMode,
    onlyNew,
    setOnlyNew,
    starting,
    running,
    pull,
    progress,
    summary,
    failure,
    start,
    stop,
  } = ingest

  return (
    <section className="md-card backup-usb" data-testid="c-3e-usb">
      {/* The control the human asked for, and it lives in the card header so it is
          there in every state — "no phone" is precisely the state where it is
          wanted. Disabled only while a pass runs: enumerating MTP mid-transfer
          spends USB round-trips on a phone already streaming, and the selection
          must not move under a live job. */}
      <div className="usb-head">
        <h2>{t('backup.usb')}</h2>
        <Button
          variant="text"
          onClick={() => void reload()}
          disabled={running || reloading}
          data-testid="c-3e-refresh"
        >
          {reloading ? t('backup.usbRescanBusy') : t('backup.usbRescan')}
        </Button>
      </div>

      {error ? (
        // No list at all and the last read failed. Saying "no device connected"
        // here would be a lie about a phone that is plugged in and unreadable — so
        // this replaces that message rather than stacking on top of it.
        //
        // Both halves, and the second one is not decoration: `error.E_DEVICE`'s own
        // sentence is the same generic line for every driver problem, while
        // `detail` is what the backend reported, HRESULT included. Rendering
        // `t(error.code)` here instead printed the bare string `E_DEVICE`, which
        // told the human nothing at all about a failure they were looking at.
        <p className="md-card__hint md-card__hint--error" data-testid="c-3e-list-error">
          {t('error.E_DEVICE')}
          <span className="usb-error__detail">{error.detail}</span>
        </p>
      ) : loadingDevices ? (
        <p className="md-card__hint">{t('backup.usbScanning')}</p>
      ) : devices.length === 0 ? (
        // §8.2: never claim to know more than the backend proved. MTP cannot tell
        // "nothing plugged in" apart from "plugged in, locked, or waiting for the
        // iPhone trust prompt" — Windows publishes no content device until the
        // phone answers — so the message states what was established (no device
        // identified) and then the things that actually unblock it, instead of
        // asserting that the cable is empty.
        <p className="md-card__hint" data-testid="c-3e-none">
          {t('backup.usbNone')}
        </p>
      ) : (
        <>
          {/* Affirmative, and counted: a list on screen is the only evidence there
              is that Windows is publishing the phone at all. */}
          <p className="md-card__hint" data-testid="c-3e-found">
            {t('backup.usbFound', { count: devices.length })}
            {devices.length > 1 ? ` ${t('backup.usbChoose')}` : ''}
          </p>
          <div className="usb-devices" role="radiogroup" aria-label={t('backup.usbDevice')}>
            {devices.map((entry) => (
              <label
                key={entry.id}
                className={`usb-device${entry.friendlyName === device?.friendlyName ? ' usb-device--active' : ''}`}
              >
                <input
                  type="radio"
                  name="usb-device"
                  value={entry.friendlyName}
                  checked={entry.friendlyName === device?.friendlyName}
                  disabled={running}
                  onChange={() => select(entry.friendlyName)}
                />
                <span className="usb-device__name">{entry.friendlyName}</span>
                {/* §5.4: what will actually be created, not the raw device name —
                    two phones can report the same friendly name and land in one
                    folder otherwise. */}
                <span className="usb-device__label">
                  {t('backup.usbWillLand', { label: entry.name })}
                </span>
              </label>
            ))}
          </div>
        </>
      )}

      {stale ? (
        // The list survived a failed refresh, so say that it may be out of date
        // rather than pretending the read succeeded (§10.6.4).
        <p className="md-card__hint md-card__hint--error" data-testid="c-3e-stale">
          {t('backup.usbStale')}
        </p>
      ) : null}

      {device ? (
        <>
          {/* §11.3's destination line, in the same spirit as C-3n2 on the
              Disco-local pane: a preview of `Media/<label>/`, which §5.4
              sanitizes again on the Rust side. */}
          <p className="md-card__hint">{t('backup.destination', { label: device.name })}</p>

          {/* §12.1's mode. `move` is the destructive one and the copy states the
              order it obeys, so the choice is never a surprise. */}
          <div className="usb-options">
            <label className="md-checkbox">
              <input
                type="radio"
                name="usb-mode"
                checked={mode === 'copy'}
                disabled={running}
                onChange={() => setMode('copy')}
                data-testid="c-3f-copy"
              />
              {t('backup.usbModeCopy')}
            </label>
            <label className="md-checkbox">
              <input
                type="radio"
                name="usb-mode"
                checked={mode === 'move'}
                disabled={running}
                onChange={() => setMode('move')}
                data-testid="c-3f-move"
              />
              {t('backup.usbModeMove')}
            </label>
          </div>

          {/* `only_new` is a real saving on a re-import (it skips the transfer,
              not just the hash) but it is a *heuristic* — name+size — so the
              label says which. */}
          <label className="md-checkbox">
            <input
              type="checkbox"
              checked={onlyNew}
              disabled={running}
              onChange={(event) => setOnlyNew(event.target.checked)}
              data-testid="c-3f-only-new"
            />
            {t('backup.usbOnlyNew')}
          </label>
        </>
      ) : null}

      {running ? (
        <>
          {/* Two phases with separate bars: a 4 GB video spends minutes being
              pulled and is one row when scanned. Conflating them would show a bar
              that appears frozen for the whole transfer. */}
          {pull ? (
            <div className="usb-phase" data-testid="c-3e-pull">
              <p className="md-card__hint">
                {/* `total: 0` is §9.2's "the walk cannot know its size yet", and on
                    USB it arrives *first* — before the first byte, because `list_media`
                    walks the whole device. Saying "scanning the folder" here would
                    name a folder that does not exist and hide a wait that measured
                    ~4 minutes on a real phone. */}
                {pull.total ? (
                  <>
                    {t('backup.usbPullPhase')}{' '}
                    {`${formatCount(pull.current)} / ${formatCount(pull.total)}`}
                  </>
                ) : (
                  t('backup.usbListing')
                )}
              </p>
              {pull.total > 0 ? (
                <div className="md-meter" aria-label={t('backup.usbPullProgressLabel')}>
                  <div
                    className="md-meter__fill md-meter__fill--images"
                    style={{ width: `${ratioOf(pull.current, pull.total) * 100}%` }}
                  />
                </div>
              ) : null}
              <p className="md-card__hint usb-phase__file" title={pull.lastName}>
                {pull.lastName}
              </p>
            </div>
          ) : null}

          {progress ? (
            <div className="usb-phase" data-testid="c-3e-progress">
              <p className="md-card__hint">
                {t('backup.usbScanPhase')}{' '}
                {progress.total
                  ? `${formatCount(progress.current)} / ${formatCount(progress.total)}`
                  : t('dash.scanCollecting')}
              </p>
              {progress.total > 0 ? (
                <div className="md-meter" aria-label={t('dash.scanProgressLabel')}>
                  <div
                    className="md-meter__fill md-meter__fill--videos"
                    style={{ width: `${ratioOf(progress.current, progress.total) * 100}%` }}
                  />
                </div>
              ) : null}
            </div>
          ) : null}
        </>
      ) : null}

      {summary ? (
        summary.cancelled ? (
          // A cancelled run still sends a summary, with `cancelled: true` — the
          // same contract §7.3.1 gave the folder scanner. Printing the counters
          // instead would read as "imported 0 files, all good", which is the one
          // reading that is both wrong and the reason c12 needs this branch.
          <p className="md-card__hint" data-testid="c-3e-cancelled">
            {t('dash.scanCancelled')}
          </p>
        ) : (
          <p className="md-card__hint" data-testid="c-3e-summary">
            {t('backup.usbResult', {
              inserted: formatCount(summary.inserted),
              duplicates: formatCount(summary.duplicates),
              moved: mode === 'move' ? t('backup.usbMoved') : '',
            })}
          </p>
        )
      ) : null}

      {failure ? (
        <p className="md-card__hint md-card__hint--error" data-testid="c-3e-failure">
          {t('backup.usbFailed', { device: failure.device, message: failure.message })}
        </p>
      ) : null}

      <div className="md-card__row">
        {running ? (
          <Button variant="tonal" onClick={() => void stop()} data-testid="c-3k-stop">
            {t('dash.scanCancel')}
          </Button>
        ) : (
          <Button
            variant="filled"
            onClick={() => void start()}
            disabled={starting || device === null}
            data-testid="c-3j-start"
          >
            {starting ? t('backup.usbStarting') : t('backup.importStart')}
          </Button>
        )}
        {running ? (
          // The reminder that matters most while it runs: `move` is deleting from
          // the phone, and the human is the only one who can stop it.
          <span className="md-card__hint">
            {mode === 'move' ? t('backup.usbMoveReminder') : t('backup.usbCopyReminder')}
          </span>
        ) : null}
      </div>
    </section>
  )
}
