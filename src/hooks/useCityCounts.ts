import { useCallback } from 'react'
import { cityCounts } from '../lib/ipc'
import type { CityCountDto, MediaScopeDto } from '../types/api'
import { useAsync } from './useAsync'
import { useDbChanged } from './useTauriEvent'

/**
 * §10.3 `useCityCounts(scope)` — the L2 `Localização` tag-cloud nodes (§10.6.1).
 *
 * The counts come from `city_counts`, which is scoped (§6.6): Standard counts
 * only visible media, so a hidden city can never leak into the drawer. Any write
 * elsewhere in the app (`wolfs://db-changed`) re-reads, so the tag-cloud follows
 * an ingest or a scan without a reopen.
 */
export function useCityCounts(scope: MediaScopeDto) {
  const load = useCallback(() => cityCounts(scope), [scope])
  const { data, error, loading, rerun } = useAsync<CityCountDto[]>(load, [scope])
  useDbChanged(rerun)

  return { cities: data ?? [], error, loading }
}
