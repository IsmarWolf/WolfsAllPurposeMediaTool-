import { describe, expect, it } from 'vitest'
import { deriveState } from '../hooks/useStateMachine'

const base = { loading: false, error: null, data: [] as string[], count: 0, filtered: false }

describe('useStateMachine branch derivation (§10.6.4)', () => {
  it('is loading while a request is in flight', () => {
    expect(deriveState({ ...base, loading: true, data: null, count: 0 })).toBe('loading')
  })

  it('is error when the command failed', () => {
    expect(deriveState({ ...base, error: new Error('E_DB'), data: null })).toBe('error')
  })

  // Precedence: an in-flight request is loading even if data is still around.
  it('prefers loading over a stale error', () => {
    expect(deriveState({ ...base, loading: true, error: new Error('E_DB') })).toBe('loading')
  })

  it('is empty when a truly empty folder comes back', () => {
    expect(deriveState({ ...base, data: [], count: 0, filtered: false })).toBe('empty')
  })

  it('is noresults when filters are active and nothing matched', () => {
    expect(deriveState({ ...base, data: [], count: 0, filtered: true })).toBe('noresults')
  })

  it('is ideal when items came back', () => {
    expect(deriveState({ ...base, data: ['a'], count: 1 })).toBe('ideal')
  })
})
