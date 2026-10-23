import { useCallback } from 'react'
import { toAppError } from '../lib/errors'
import { mediaSetFavorite } from '../lib/ipc'
import { useToast } from './useContexts'

/**
 * §24.2 (c22) — the one bridge from every ♡ affordance (tile, context menu,
 * inspector [Z4h], lightbox [C-4l], bulk [Z5f]) to §8.1's `media_set_favorite`.
 *
 * The grids refresh through the `wolfs://db-changed` the backend emits, never
 * through a local flip: a batch that only *partly* succeeds (a row deleted
 * under the click) would leave every optimistic mirror lying about the rest.
 * The one exception is the lightbox, which keeps a same-session override so its
 * own button tracks the click without refetching a detail the backend has
 * already updated.
 */
export function useSetFavorite() {
  const { pushError } = useToast()
  return useCallback(
    async (ids: readonly string[], on: boolean): Promise<boolean> => {
      if (ids.length === 0) {
        return false
      }
      try {
        await mediaSetFavorite([...ids], on)
        return true
      } catch (raw) {
        pushError(toAppError(raw))
        return false
      }
    },
    [pushError],
  )
}
