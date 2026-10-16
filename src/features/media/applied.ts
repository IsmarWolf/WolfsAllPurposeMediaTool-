import type { FilterSpec } from '../../types/api'

/**
 * §10.6.1 / §15 — the active filters of a spec, locale-free.
 *
 * A direct mirror of `core/filters.rs::applied` (same fields, same order:
 * search → device → cities → month → fileType → noMetadata). The gallery chips
 * and the smart-list criteria summary both render from this list, so the two
 * surfaces **cannot** disagree about what a filter means — the exact parity the
 * c16 gate (§15) requires. Keeping the shape in one place is what makes that a
 * structural fact instead of a coincidence.
 */
export type AppliedField = 'search' | 'device' | 'city' | 'month' | 'fileType' | 'noMetadata'

export interface AppliedFilter {
  field: AppliedField
  /** `null` for boolean filters (no-metadata), which have no value to show. */
  value: string | null
}

export function appliedFilters(spec: FilterSpec): AppliedFilter[] {
  const out: AppliedFilter[] = []
  if (spec.search) {
    out.push({ field: 'search', value: spec.search })
  }
  if (spec.device) {
    out.push({ field: 'device', value: spec.device })
  }
  for (const city of spec.cities) {
    out.push({ field: 'city', value: city })
  }
  if (spec.month) {
    out.push({ field: 'month', value: spec.month })
  }
  if (spec.fileType) {
    out.push({ field: 'fileType', value: spec.fileType })
  }
  if (spec.noMetadata) {
    out.push({ field: 'noMetadata', value: null })
  }
  return out
}

type Translate = (key: string, params?: Record<string, string | number>) => string

/** The `[Filter Label: Value]` text of one chip (§10.6.1). */
export function chipLabel(filter: AppliedFilter, t: Translate): string {
  switch (filter.field) {
    case 'search':
      return `${t('media.search')}: ${filter.value}`
    case 'device':
      return `${t('media.device')}: ${filter.value}`
    case 'city':
      return `${t('media.location')}: ${filter.value}`
    case 'month':
      return `${t('media.month')}: ${filter.value}`
    case 'fileType':
      return `${t('media.type')}: ${filter.value}`
    case 'noMetadata':
      return t('media.noMetadata')
  }
}
