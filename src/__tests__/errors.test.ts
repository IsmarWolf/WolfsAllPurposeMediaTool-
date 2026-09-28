import { describe, expect, it } from 'vitest'
import { AppError, toAppError } from '../lib/errors'

describe('toAppError', () => {
  it('maps the serialized { code, message } of the backend', () => {
    const error = toAppError({ code: 'E_ROOT', message: 'raiz ausente' })
    expect(error).toBeInstanceOf(AppError)
    expect(error.code).toBe('E_ROOT')
    expect(error.i18nKey).toBe('error.E_ROOT')
    expect(error.detail).toBe('raiz ausente')
  })

  it('never invents a code it does not know', () => {
    const error = toAppError({ code: 'E_MADE_UP', message: 'x' })
    expect(error.code).toBe('E_UNAVAILABLE')
    expect(error.i18nKey).toBe('error.E_UNAVAILABLE')
  })

  it('passes an existing AppError through untouched', () => {
    const original = new AppError('E_DB', 'locked')
    expect(toAppError(original)).toBe(original)
  })

  it('handles a bare string and a non-object', () => {
    expect(toAppError('boom').code).toBe('E_UNAVAILABLE')
    expect(toAppError(undefined).code).toBe('E_UNAVAILABLE')
  })

  // §8.2: a cancelled job must not raise a toast anywhere.
  it('marks E_CANCEL as silent and keyless', () => {
    const error = new AppError('E_CANCEL', 'cancelado pelo usuário')
    expect(error.isSilent).toBe(true)
    expect(error.i18nKey).toBeNull()
  })
})
