import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { toAppError } from '../lib/errors'
import { mediaQuery } from '../lib/ipc'
import type { FilterSpec, MediaQueryDto } from '../types/api'
import { useDbChanged } from './useTauriEvent'

const DEFAULT_SPEC: FilterSpec = {
  scope: 'standard',
  device: null,
  cities: [],
  month: null,
  noMetadata: false,
  search: null,
  fileType: null,
  favorite: false,
  screenshot: false,
  sort: 'capturedDesc',
  limit: 240,
  offset: 0,
}

/**
 * The spec a fresh filter surface starts from. Exported for the c16 snapshot
 * builder (§11.5), which seeds its local spec from a list's stored criteria —
 * or this when the list is new/manual — and must agree with the gallery's own
 * defaults or "Editar critérios" would open on a different starting point than
 * the gallery shows.
 */
export const DEFAULT_FILTER_SPEC: FilterSpec = DEFAULT_SPEC

/**
 * §10.3 `useMediaQuery()` — the gallery's data hook.
 *
 * The spec is the single source of truth: every filter mutation produces a new
 * object and re-runs the query. Pagination is `offset`-based; `hasMore` tells
 * the grid whether to offer "load more". A stale response never overwrites a
 * newer one (request id guard).
 *
 * Two fixes from the c13 smoke (2026-10-05): "load more" *appends* the next
 * page on top of the current items and keeps the grid mounted (only the
 * initial/no-data read shows the skeleton), so the scroll position survives;
 * and any `wolfs://db-changed` — an ingest or a scan elsewhere in the app —
 * re-runs the current spec, so the badge and the grid catch up on their own.
 */
export function useMediaQuery(initial?: Partial<FilterSpec>) {
  // The §11.6 vault grid is the only caller that passes `initial` (`scope: Vault`):
  // it is the same query, filters and paging as the gallery, seeded on the hidden
  // scope so the two pages share one code path (C6: the scope is explicit here,
  // never inferred). Captured once — a fresh inline object each render must not
  // reset the spec.
  // The ref is read only from the mount effect below, never during render; the
  // state computes the same merged spec once, so both agree without touching a
  // ref in render (react-compiler rule).
  const initialSpec = useRef<FilterSpec>({ ...DEFAULT_SPEC, ...initial })
  const [spec, setSpec] = useState<FilterSpec>(() => ({ ...DEFAULT_SPEC, ...initial }))
  const [data, setData] = useState<MediaQueryDto | null>(null)
  const [loading, setLoading] = useState(true)
  const [loadingMore, setLoadingMore] = useState(false)
  const [error, setError] = useState<ReturnType<typeof toAppError> | null>(null)
  const requestId = useRef(0)

  const run = useCallback(async (nextSpec: FilterSpec, append = false) => {
    const id = ++requestId.current
    if (append) {
      setLoadingMore(true)
    } else {
      setLoading(true)
    }
    setError(null)
    try {
      const result = await mediaQuery(nextSpec)
      if (id === requestId.current) {
        setData((prev) =>
          append && prev ? { ...result, items: [...prev.items, ...result.items] } : result,
        )
        setError(null)
      }
    } catch (raw) {
      if (id === requestId.current) {
        setError(toAppError(raw))
        if (!append) {
          setData(null)
        }
      }
    } finally {
      if (id === requestId.current) {
        setLoading(false)
        setLoadingMore(false)
      }
    }
  }, [])

  // First read on mount — same "one read per mount" contract as useAsync.
  useEffect(() => {
    // oxlint-disable-next-line react/set-state-in-effect
    void run(initialSpec.current)
  }, [run])

  const updateSpec = useCallback(
    (patch: Partial<FilterSpec>) => {
      setSpec((current) => {
        const next = { ...current, ...patch, offset: 0 }
        void run(next)
        return next
      })
    },
    [run],
  )

  const loadMore = useCallback(() => {
    if (!data?.hasMore || loading || loadingMore) return
    setSpec((current) => {
      const next = { ...current, offset: current.offset + current.limit }
      void run(next, true)
      return next
    })
  }, [data, loading, loadingMore, run])

  const clearFilters = useCallback(() => {
    setSpec((current) => {
      const next = { ...DEFAULT_SPEC, scope: current.scope }
      void run(next)
      return next
    })
  }, [run])

  const filtered = useMemo(
    () =>
      spec.search !== null ||
      spec.device !== null ||
      spec.cities.length > 0 ||
      spec.month !== null ||
      spec.noMetadata ||
      spec.fileType !== null ||
      spec.favorite ||
      spec.screenshot,
    [spec],
  )

  const retry = useCallback(() => {
    void run(spec)
  }, [run, spec])

  // c13 fix: any write elsewhere in the app (scan, USB/Wi-Fi ingest, remove)
  // makes the badge and the grid stale. Re-run the *current* spec, so a load in
  // progress is not lost to an unused `offset`.
  useDbChanged(retry)

  return {
    spec,
    data,
    loading,
    loadingMore,
    error,
    filtered,
    updateSpec,
    loadMore,
    clearFilters,
    retry,
  }
}
