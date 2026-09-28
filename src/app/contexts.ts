import { createContext } from 'react'
import type { AppError } from '../lib/errors'
import type { ToastKind } from '../types/api'

export type Locale = 'pt-BR' | 'en'

export type I18nContextValue = {
  locale: Locale
  setLocale: (locale: Locale) => void
  /** `{count}` style interpolation, so a translated sentence keeps its order. */
  t: (key: string, params?: Record<string, string | number>) => string
}

export type SkinContextValue = {
  skin: string
}

export type ToastItem = {
  id: number
  kind: ToastKind
  message: string
}

export type ToastContextValue = {
  toasts: ToastItem[]
  /** `push` is imperative by design (§10.3): any module can report a problem. */
  push: (kind: ToastKind, message: string) => void
  dismiss: (id: number) => void
  /** Maps an `AppError` to its i18n key + pushes it, unless it is silent (§8.2). */
  pushError: (error: AppError) => void
}

export const I18nContext = createContext<I18nContextValue | null>(null)
export const SkinContext = createContext<SkinContextValue | null>(null)
export const ToastContext = createContext<ToastContextValue | null>(null)
