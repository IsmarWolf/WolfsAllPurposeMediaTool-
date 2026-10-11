import { useI18n, useTransfer } from '../../hooks/useContexts'
import { formatCount, ratioOf } from '../../lib/format'

/**
 * The bottom transfer bar — "where did my photos go", for as long as they are
 * going.
 *
 * It is in the shell and not in the Backup page because that is the whole point:
 * the pass is started there and then the human walks off to the Dashboard or the
 * gallery to do something else, and a bar that vanishes with the page answers
 * "did it stop?" with the one answer that is never true. §9.2's channels carry
 * no job id and §9.3's job outlives the screen, so the state comes from
 * `TransferProvider` — the same instance the pane reads, never a second
 * subscription that could disagree with it.
 *
 * It shows one job at a time and nothing at all when none is running, rather
 * than a permanent strip: a bar that is always there is a bar nobody reads, and
 * an idle one next to Zone 5 would be two floating things competing.
 */
export function TransferBar() {
  const { t } = useI18n()
  const { job } = useTransfer()

  if (!job) {
    return null
  }

  // §9.2's `total: 0` is "the walk cannot know its size yet", which is a
  // different statement from "nothing has happened". Collapsing the two would
  // paint a stalled pass as an empty one.
  const determinate = job.total > 0
  const percent = determinate ? Math.round(ratioOf(job.current, job.total) * 100) : 0

  return (
    <div
      className="transfer"
      data-testid="transfer-bar"
      data-kind={job.kind}
      data-phase={job.phase}
    >
      <div className="transfer__row">
        {/* The source is named because the channel is shared: §9.2 has one
            `wolfs://progress/ingest` for USB and Wi-Fi and no job id, so "a
            transfer is running" without saying which would be a guess. */}
        <span className="transfer__source">{t(`transfer.source.${job.kind}`)}</span>
        <span className="transfer__label" title={job.label}>
          {job.label}
        </span>
        <span className="transfer__phase">{t(`transfer.phase.${job.phase}`)}</span>
        {/* The file being pulled. It is the only proof this is *this* pass and not
            a leftover, and on a slow phone the name is what tells a human whether
            the album they wanted is the one coming. */}
        <span className="transfer__file" title={job.lastName} data-testid="transfer-bar-file">
          {job.lastName}
        </span>
        <span className="transfer__spacer" />
        {/* The counts, not just the bar: a percentage alone cannot say whether the
            pass is moving 3 files or 3 000. */}
        <span className="transfer__count" data-testid="transfer-bar-count">
          {determinate
            ? `${formatCount(job.current)} / ${formatCount(job.total)}`
            : t('transfer.unknownTotal')}
        </span>
        {job.cancel ? (
          <button
            type="button"
            className="md-btn md-btn--text transfer__cancel"
            onClick={job.cancel}
            data-testid="transfer-bar-cancel"
          >
            {t('dash.scanCancel')}
          </button>
        ) : null}
      </div>

      <div
        className="transfer__track"
        role="progressbar"
        aria-label={t('transfer.barLabel')}
        aria-valuemin={0}
        // Indeterminate means *no* `aria-valuenow`: that is how ARIA says "I
        // cannot know yet". Reporting 0 would be a claim — and the human would
        // read a listing pass as one that has stalled.
        {...(determinate ? { 'aria-valuemax': job.total, 'aria-valuenow': job.current } : {})}
      >
        <div
          className={`transfer__fill${determinate ? '' : ' transfer__fill--indeterminate'}`}
          style={determinate ? { width: `${percent}%` } : undefined}
          data-testid="transfer-bar-fill"
        />
      </div>
    </div>
  )
}
