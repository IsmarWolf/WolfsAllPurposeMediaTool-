import { describe, expect, it } from 'vitest'
import { formatBytes, formatCount, ratioOf } from '../lib/format'

describe('format', () => {
  it('formats counts with pt-BR separators', () => {
    expect(formatCount(1234567)).toBe('1.234.567')
  })

  it('formats binary units', () => {
    expect(formatBytes(0)).toBe('0 B')
    expect(formatBytes(942)).toBe('942 B')
    expect(formatBytes(1536)).toBe('1,5 KB')
    expect(formatBytes(5 * 1024 * 1024)).toBe('5,0 MB')
    expect(formatBytes(128 * 1024 * 1024 * 1024)).toBe('128,0 GB')
  })

  it('clamps the meter ratio', () => {
    expect(ratioOf(5, 0)).toBe(0)
    expect(ratioOf(5, 10)).toBe(0.5)
    expect(ratioOf(20, 10)).toBe(1)
    expect(ratioOf(-1, 10)).toBe(0)
  })
})
