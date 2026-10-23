import { useCallback } from 'react'
import { toAppError } from '../lib/errors'
import {
  listsAddItems,
  listsCreate,
  listsDelete,
  listsDetail,
  listsList,
  listsRemoveItems,
  listsUpdate,
} from '../lib/ipc'
import type { ListDetailDto, ListDto } from '../types/api'
import { useAsync } from './useAsync'
import { useToast } from './useContexts'
import { useDbChanged } from './useTauriEvent'

/**
 * §10.3 `useLists()` — the Lists page index (§11.5). The backend returns rows
 * A→Z case-insensitive (c15 decision 6), so the pane never re-sorts.
 *
 * Any write anywhere (`wolfs://db-changed`, §9.2) re-reads: a member added
 * from the gallery (c22's add-to-list modal) or here both have to move the
 * counts.
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
 * `useTauriCommand`'s doc comment records.
 *
 * The four `Result<(), AppError>` commands resolve to `true` on success, never
 * to the raw payload: `invoke` turns Rust's `()` into `null`, which is exactly
 * what `run` returns on failure, so a bare `listsUpdate(...)` would make the
 * success path and the swallowed-error path the same value — and a modal that
 * waits for `result !== null` to show its toast would never show it.
 *
 * `create` is the one command whose result is used (the new row), hence a
 * bespoke hook rather than the generic `useTauriCommand`, whose contract is
 * `Promise<void>`.
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
    (id: string, name: string) => run(() => listsUpdate({ id, name }).then(() => true)),
    [run],
  )
  const remove = useCallback((id: string) => run(() => listsDelete(id).then(() => true)), [run])
  const addItems = useCallback(
    (listId: string, mediaIds: string[]) =>
      run(() => listsAddItems({ listId, mediaIds }).then(() => true)),
    [run],
  )
  const removeItems = useCallback(
    (listId: string, mediaIds: string[]) =>
      run(() => listsRemoveItems({ listId, mediaIds }).then(() => true)),
    [run],
  )
  // c22/§24.1: `applySnapshot`/`sync` are retired from the UI. The frontend
  // bindings in `lib/ipc.ts` stay (deprecated, callable) so the backend
  // commands do not rot, but nothing in the app offers them: lists are
  // manual-only now, and a legacy smart list renders as a plain list.

  return { create, rename, remove, addItems, removeItems }
}
