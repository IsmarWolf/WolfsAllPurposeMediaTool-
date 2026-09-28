/**
 * §10.6.4 / §10.3 `useStateMachine(stage)` — the one place a state is derived,
 * so no screen can forget a branch (C13).
 *
 * Precedence is the table's order: a request in flight is *loading* even if a
 * previous result is still on screen; an error outranks everything; a result
 * with no items is `empty` or `noresults` depending on whether a spec/search is
 * active (the distinction is the caller's job, not the machine's).
 */

export type UiState = 'ideal' | 'empty' | 'noresults' | 'loading' | 'error'

export type StageInput<T> = {
  loading: boolean
  error: unknown
  data: T | null
  /** 0 items after a successful read. */
  count: number
  /** Are filters/search active? Decides `empty` vs `noresults`. */
  filtered: boolean
}

export function deriveState<T>(stage: StageInput<T>): UiState {
  if (stage.loading) {
    return 'loading'
  }
  if (stage.error) {
    return 'error'
  }
  if (stage.data === null) {
    return 'loading'
  }
  if (stage.count === 0) {
    return stage.filtered ? 'noresults' : 'empty'
  }
  return 'ideal'
}

export function useStateMachine<T>(stage: StageInput<T>): UiState {
  return deriveState(stage)
}
