import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { toAppError } from '../lib/errors'
import { mediaQuery } from '../lib/ipc'
import type { FilterSpec, MediaQueryDto } from '../types/api'

const DEFAULT_SPEC: FilterSpec = {
  scope: 'standard',
  device: null,
  cities: [],
  month: null,
  noMetadata: false,
  search: null,
  fileType: null,
  sort: 'capturedDesc',
  limit: 240,
  offset: 0,
}

/**
 * §10.3 `useMediaQuery()` — the gallery's data hook.
 *
 * The spec is the single source of truth: every filter mutation produces a new
 * object and re-runs the query. Pagination is `offset`-based; `hasMore` tells
 * the grid whether to offer "load more". A stale response never overwrites a
 * newer one (request id guard).
 */
export function useMediaQuery() {
  const [spec, setSpec] = useState<FilterSpec>(DEFAULT_SPEC)
  const [data, setData] = useState<MediaQueryDto | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<ReturnType<typeof toAppError> | null>(null)
  const requestId = useRef(0)

  const run = useCallback(async (nextSpec: FilterSpec) => {
    const id = ++requestId.current
    setLoading(true)
    setError(null)
    try {
      const result = await mediaQuery(nextSpec)
      if (id === requestId.current) {
        setData(result)
        setError(null)
      }
    } catch (raw) {
      if (id === requestId.current) {
        setError(toAppError(raw))
        setData(null)
      }
    } finally {
      if (id === requestId.current) {
        setLoading(false)
      }
    }
  }, [])

  // First read on mount — same "one read per mount" contract as useAsync.
  useEffect(() => {
    // oxlint-disable-next-line react/set-state-in-effect
    void run(DEFAULT_SPEC)
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
    if (!data?.hasMore || loading) return
    setSpec((current) => {
      const next = { ...current, offset: current.offset + current.limit }
      void run(next)
      return next
    })
  }, [data, loading, run])

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
      spec.fileType !== null,
    [spec],
  )

  const retry = useCallback(() => {
    void run(spec)
  }, [run, spec])

  return {
    spec,
    data,
    loading,
    error,
    filtered,
    updateSpec,
    loadMore,
    clearFilters,
    retry,
  }
}
