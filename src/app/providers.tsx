import { useMemo, useState, type ReactNode } from 'react'
import en from '../i18n/en.json'
import ptBR from '../i18n/pt-BR.json'
import { I18nContext, SkinContext, type I18nContextValue, type Locale } from './contexts'

const DEFAULT_LOCALE: Locale = 'pt-BR'

const dictionaries: Record<Locale, Record<string, string>> = {
  'pt-BR': ptBR,
  en,
}

export function Providers({ children }: { children: ReactNode }) {
  const [locale, setLocale] = useState<Locale>(DEFAULT_LOCALE)

  const value = useMemo<I18nContextValue>(
    () => ({
      locale,
      setLocale,
      t: (key: string) => dictionaries[locale][key] ?? dictionaries[DEFAULT_LOCALE][key] ?? key,
    }),
    [locale],
  )

  return (
    <SkinContext.Provider value={{ skin: 'md3' }}>
      <I18nContext.Provider value={value}>{children}</I18nContext.Provider>
    </SkinContext.Provider>
  )
}
