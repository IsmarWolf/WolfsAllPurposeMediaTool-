import { useCallback, useMemo, useState } from 'react'

/** §10.3 `useSelection()` — Set-based, with anchor + shift-range and a cap. */
export const SELECT_ALL_CAP = 1000

export function useSelection() {
  const [selected, setSelected] = useState<ReadonlySet<string>>(() => new Set())
  const [anchor, setAnchor] = useState<string | null>(null)

  const clear = useCallback(() => {
    setSelected(new Set())
    setAnchor(null)
  }, [])

  /** Membership toggle, used by the tile overlay checkbox. */
  const toggle = useCallback((id: string) => {
    setSelected((current) => {
      const next = new Set(current)
      if (next.has(id)) {
        next.delete(id)
      } else {
        next.add(id)
      }
      return next
    })
    setAnchor(id)
  }, [])

  /**
   * Range selection over the *filtered + sorted* order (§12.6), so chips and
   * ordering stay authoritative. The cap is the §10.6.5 performance guard.
   */
  const selectRange = useCallback((ids: readonly string[]) => {
    setSelected((current) => {
      const next = new Set(current)
      for (const id of ids.slice(0, SELECT_ALL_CAP)) {
        next.add(id)
      }
      return next
    })
  }, [])

  const selectAll = useCallback((ids: readonly string[]) => {
    setSelected(new Set(ids.slice(0, SELECT_ALL_CAP)))
  }, [])

  return useMemo(
    () => ({
      selected,
      count: selected.size,
      anchor,
      setAnchor,
      toggle,
      selectRange,
      selectAll,
      clear,
    }),
    [selected, anchor, toggle, selectRange, selectAll, clear],
  )
}

/** §10.3 `useInspector(mode)` — Zone 4 render mode, strictly selection-driven. */
export function useInspector(count: number): 'summary' | 'single' | 'bulk' {
  if (count === 0) {
    return 'summary'
  }
  return count === 1 ? 'single' : 'bulk'
}
