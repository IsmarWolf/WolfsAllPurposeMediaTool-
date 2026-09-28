import { useCallback, useEffect, useRef, useState } from 'react'
import { toAppError } from '../lib/errors'
import { useToast } from './useContexts'

/**
 * §10.3 `useAsync(fn, deps)` -> `{ data, error, loading, rerun }`.
 *
 * The four fields *are* the input of `useStateMachine`, so a screen cannot
 * forget a branch of §10.6.4.
 */
export function useAsync<T>(fn: () => Promise<T>, deps: readonly unknown[] = []) {
  const [data, setData] = useState<T | null>(null)
  const [error, setError] = useState<ReturnType<typeof toAppError> | null>(null)
  const [loading, setLoading] = useState(true)

  // The loader lives in a ref, NOT in the effect's dependency list. A caller
  // passes an inline arrow, so a `[fn]`-keyed callback would produce a new
  // identity on every render and the effect below would refetch forever - which
  // showed up as numbers blinking between "loading" and 0.
  const loader = useRef(fn)
  useEffect(() => {
    loader.current = fn
  }, [fn])

  const run = useCallback(async (restart: boolean) => {
    if (restart) {
      setLoading(true)
      setError(null)
    }
    try {
      setData(await loader.current())
      setError(null)
    } catch (raw) {
      setError(toAppError(raw))
      setData(null)
    } finally {
      setLoading(false)
    }
  }, [])

  /** Retry / invalidate: goes back to the loading branch of §10.6.4. */
  const rerun = useCallback(() => run(true), [run])

  useEffect(() => {
    // The first read does not "restart" anything: the hook already mounted in
    // the loading state, and setting state synchronously here would schedule a
    // second render before the request even leaves.
    //
    // Both suppressions below are the cost of the §10.3 signature: the caller
    // owns the dependency list, so the array cannot be statically verified, and
    // the linter cannot see that every `setState` in `run` happens after the
    // `await` (a false positive for an async loader).
    // oxlint-disable-next-line react/set-state-in-effect, react-hooks/exhaustive-deps
    void run(false)
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, run])

  return { data, error, loading, rerun }
}

/**
 * §10.3 `useTauriCommand(fn, args)` — an imperative typed await whose error
 * mapping to a toast happens here, so no screen repeats the try/catch.
 *
 * A second invocation while one is in flight is dropped: the busy state of
 * §10.6.4 must be visible, and a double click must not run the command twice.
 */
export function useTauriCommand<Args extends unknown[]>(command: (...args: Args) => Promise<void>) {
  const { pushError } = useToast()
  const [busy, setBusy] = useState(false)
  const inFlight = useRef(false)

  const run = useCallback(
    async (...args: Args) => {
      if (inFlight.current) {
        return
      }
      inFlight.current = true
      setBusy(true)
      try {
        await command(...args)
      } catch (raw) {
        pushError(toAppError(raw))
      } finally {
        inFlight.current = false
        setBusy(false)
      }
    },
    [command, pushError],
  )

  return { run, busy }
}
