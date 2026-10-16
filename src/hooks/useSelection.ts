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
   * ordering stay authoritative. REPLACE semantics (human 2026-10-15): the
   * selection becomes exactly the anchor→cursor range, the way Shift+arrow
   * behaves in Windows Explorer — a union would only ever grow. The cap is the
   * §10.6.5 performance guard.
   */
  const selectRange = useCallback((ids: readonly string[]) => {
    setSelected(new Set(ids.slice(0, SELECT_ALL_CAP)))
  }, [])

  /** Single-select with anchor move — a plain click / plain arrow step. */
  const selectOne = useCallback((id: string) => {
    setSelected(new Set([id]))
    setAnchor(id)
  }, [])

  /**
   * §12.6 Ctrl/Cmd+A, with the Windows Explorer toggle the human asked for
   * (2026-10-15): every page item already selected → clear, else select all.
   * The comparison uses the *capped* set, so a page over the 1 000 cap still
   * toggles (the excess was never selected in the first place).
   */
  const selectAll = useCallback(
    (ids: readonly string[]) => {
      const page = ids.slice(0, SELECT_ALL_CAP)
      const all = page.length > 0 && page.every((id) => selected.has(id))
      if (all) {
        clear()
      } else {
        setSelected(new Set(page))
      }
    },
    [selected, clear],
  )

  return useMemo(
    () => ({
      selected,
      count: selected.size,
      anchor,
      setAnchor,
      toggle,
      selectRange,
      selectOne,
      selectAll,
      clear,
    }),
    [selected, anchor, toggle, selectRange, selectOne, selectAll, clear],
  )
}

/** §10.3 `useInspector(mode)` — Zone 4 render mode, strictly selection-driven. */
export function useInspector(count: number): 'summary' | 'single' | 'bulk' {
  if (count === 0) {
    return 'summary'
  }
  return count === 1 ? 'single' : 'bulk'
}
