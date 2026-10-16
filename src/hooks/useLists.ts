import { useCallback } from 'react'
import { toAppError } from '../lib/errors'
import {
  listsAddItems,
  listsCreate,
  listsDelete,
  listsDetail,
  listsList,
  listsRemoveItems,
  listsSnapshotApply,
  listsSnapshotSync,
  listsUpdate,
} from '../lib/ipc'
import type { FilterSpec, ListDetailDto, ListDto } from '../types/api'
import { useAsync } from './useAsync'
import { useToast } from './useContexts'
import { useDbChanged } from './useTauriEvent'

/**
 * §10.3 `useLists()` — the Lists page index (§11.5). The backend returns rows
 * A→Z case-insensitive (c15 decision 6), so the pane never re-sorts.
 *
 * Any write anywhere (`wolfs://db-changed`, §9.2) re-reads: a snapshot applied
 * from the gallery, or a member added here, both have to move the counts.
 */
export function useLists() {
  const { data, error, loading, rerun } = useAsync<ListDto[]>(() => listsList(), [])
  useDbChanged(rerun)
  return { lists: data ?? [], error, loading, reload: rerun }
}

/**
 * §10.3 `useListDetail(id)` — the §11.5 right pane. `id === null` is the
 * empty-selection state and resolves to `null` without a round trip.
 */
export function useListDetail(id: string | null) {
  const load = useCallback(() => (id ? listsDetail(id) : Promise.resolve(null)), [id])
  const { data, error, loading, rerun } = useAsync<ListDetailDto | null>(load, [id])
  useDbChanged(rerun)
  return { detail: data, error, loading, reload: rerun }
}

/**
 * §10.3 mutation surface for §11.5. Every call maps its rejection to the §8.2
 * toast (exactly like `useTauriCommand`) and resolves `null` on failure, so a
 * caller can tell a real success from a swallowed error — the false-"done" bug
 * `useTauriCommand`'s doc comment records. `create` is the one command whose
 * result is used (the new row), hence a bespoke hook rather than the generic
 * `useTauriCommand`, whose contract is `Promise<void>`.
 */
export function useListActions() {
  const { pushError } = useToast()

  const run = useCallback(
    async <T>(fn: () => Promise<T>): Promise<T | null> => {
      try {
        return await fn()
      } catch (raw) {
        pushError(toAppError(raw))
        return null
      }
    },
    [pushError],
  )

  const create = useCallback((name: string) => run(() => listsCreate({ name })), [run])
  const rename = useCallback(
    (id: string, name: string) => run(() => listsUpdate({ id, name })),
    [run],
  )
  const remove = useCallback((id: string) => run(() => listsDelete(id)), [run])
  const addItems = useCallback(
    (listId: string, mediaIds: string[]) => run(() => listsAddItems({ listId, mediaIds })),
    [run],
  )
  const removeItems = useCallback(
    (listId: string, mediaIds: string[]) => run(() => listsRemoveItems({ listId, mediaIds })),
    [run],
  )
  const applySnapshot = useCallback(
    (listId: string, spec: FilterSpec) => run(() => listsSnapshotApply({ listId, spec })),
    [run],
  )
  const sync = useCallback((id: string) => run(() => listsSnapshotSync(id)), [run])

  return { create, rename, remove, addItems, removeItems, applySnapshot, sync }
}
