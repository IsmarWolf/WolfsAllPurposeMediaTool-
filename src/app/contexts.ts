import { createContext } from 'react'

export type Locale = 'pt-BR' | 'en'

export type I18nContextValue = {
  locale: Locale
  setLocale: (locale: Locale) => void
  t: (key: string) => string
}

export type SkinContextValue = {
  skin: string
}

export const I18nContext = createContext<I18nContextValue | null>(null)

export const SkinContext = createContext<SkinContextValue | null>(null)
