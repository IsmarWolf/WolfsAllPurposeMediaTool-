import { useCallback } from 'react'
import { toAppError, type AppError } from '../lib/errors'
import { EVENTS } from '../lib/events'
import {
  vaultHide,
  vaultHideFolder,
  vaultLock,
  vaultRecover,
  vaultSetup,
  vaultStatus,
  vaultUnhide,
  vaultUnlock,
} from '../lib/ipc'
import type { VaultStatusDto } from '../types/api'
import { useAsync } from './useAsync'
import { useToast } from './useContexts'
import { useTauriEvent } from './useTauriEvent'

/**
 * §10.3 `useVaultStatus()` — the §11.6 gate and the Cofre nav dot read this one
 * shape.
 *
 * Every vault command publishes `wolfs://vault` (§7.10 c18 decision 8), so setup,
 * unlock, lock and recover all move those surfaces without a manual reload — the
 * dot is *live state* (§11.6), never a value the shell guesses.
 */
export function useVaultStatus() {
  const { data, error, loading, rerun } = useAsync<VaultStatusDto>(() => vaultStatus(), [])
  useTauriEvent(EVENTS.vault, rerun)
  return { status: data, error, loading, reload: rerun }
}

/**
 * §10.3 mutation surface for §11.6.
 *
 * Two error styles on purpose, mirroring §11.6's gate semantics:
 * - `setup`/`lock`/`hide`/`hideFolder`/`unhide` map a rejection to the §8.2
 *   toast and resolve `null` on failure, so a caller can tell a real success
 *   from a swallowed error.
 * - `unlock`/`recover` are **quiet**: a wrong password must be an inline
 *   `[E_AUTH]` on the gate (with an input shake), never a toast spam — so they
 *   hand the `AppError` back to the caller instead of pushing it.
 */
export function useVaultActions() {
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

  const quiet = useCallback(async (fn: () => Promise<void>): Promise<AppError | null> => {
    try {
      await fn()
      return null
    } catch (raw) {
      return toAppError(raw)
    }
  }, [])

  const setup = useCallback((password: string) => run(() => vaultSetup(password)), [run])
  const unlock = useCallback((password: string) => quiet(() => vaultUnlock(password)), [quiet])
  const lock = useCallback(() => run(() => vaultLock()), [run])
  const recover = useCallback(
    (recoveryKey: string) => quiet(() => vaultRecover(recoveryKey)),
    [quiet],
  )
  const hide = useCallback((ids: string[]) => run(() => vaultHide(ids)), [run])
  const hideFolder = useCallback((folder: string) => run(() => vaultHideFolder(folder)), [run])
  const unhide = useCallback((ids: string[]) => run(() => vaultUnhide(ids)), [run])

  return { setup, unlock, lock, recover, hide, hideFolder, unhide }
}
