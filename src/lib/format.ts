/**
 * pt-BR display formatting. The wire format is always raw numbers/bytes; the
 * only place that turns them into "12.340" and "128 GB" is here.
 */

const numberFormat = new Intl.NumberFormat('pt-BR')

export function formatCount(value: number): string {
  return numberFormat.format(value)
}

const BYTE_UNITS = ['B', 'KB', 'MB', 'GB', 'TB'] as const

/** Binary units, one decimal, dropping `.0` on bytes: "942 B", "1,4 MB", "128 GB". */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return '0 B'
  }

  let value = bytes
  let unit = 0
  while (value >= 1024 && unit < BYTE_UNITS.length - 1) {
    value /= 1024
    unit += 1
  }

  const rounded = unit === 0 ? String(Math.round(value)) : value.toFixed(1).replace('.', ',')
  return `${rounded} ${BYTE_UNITS[unit]}`
}

/** Share of a total, clamped to 0..1 — the meter widths of C-1k. */
export function ratioOf(part: number, total: number): number {
  if (total <= 0) {
    return 0
  }
  return Math.min(1, Math.max(0, part / total))
}
