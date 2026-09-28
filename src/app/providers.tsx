import { useCallback, useMemo, useRef, useState, type ReactNode } from 'react'
import en from '../i18n/en.json'
import ptBR from '../i18n/pt-BR.json'
import { AppError } from '../lib/errors'
import type { ToastKind } from '../types/api'
import {
  I18nContext,
  SkinContext,
  ToastContext,
  type I18nContextValue,
  type Locale,
  type ToastContextValue,
  type ToastItem,
} from './contexts'

const DEFAULT_LOCALE: Locale = 'pt-BR'

const dictionaries: Record<Locale, Record<string, string>> = {
  'pt-BR': ptBR,
  en,
}

const TOAST_TIMEOUT_MS = 6000
/** Never more than 4 on screen (§19.1 state layer): the oldest falls out. */
const MAX_TOASTS = 4

export function Providers({ children }: { children: ReactNode }) {
  const [locale, setLocale] = useState<Locale>(DEFAULT_LOCALE)
  const [toasts, setToasts] = useState<ToastItem[]>([])
  const nextId = useRef(0)

  const i18n = useMemo<I18nContextValue>(
    () => ({
      locale,
      setLocale,
      // §15 asserts the fallback chain: chosen locale -> pt-BR -> the key itself,
      // so a missing translation shows the key instead of a blank.
      t: (key: string, params?: Record<string, string | number>) => {
        const template = dictionaries[locale][key] ?? dictionaries[DEFAULT_LOCALE][key] ?? key
        if (!params) {
          return template
        }
        return template.replace(/\{(\w+)\}/g, (match, name: string) =>
          params[name] === undefined ? match : String(params[name]),
        )
      },
    }),
    [locale],
  )

  const dismiss = useCallback((id: number) => {
    setToasts((current) => current.filter((toast) => toast.id !== id))
  }, [])

  const toast = useMemo<ToastContextValue>(() => {
    const push = (kind: ToastKind, message: string) => {
      nextId.current += 1
      const id = nextId.current
      setToasts((current) => [...current.slice(-(MAX_TOASTS - 1)), { id, kind, message }])
      window.setTimeout(() => dismiss(id), TOAST_TIMEOUT_MS)
    }

    return {
      toasts,
      push,
      dismiss,
      pushError: (error: AppError) => {
        // §8.2: a cancelled job is silent — no toast at all.
        if (error.isSilent) {
          return
        }
        push('error', error.i18nKey ?? error.detail)
      },
    }
  }, [toasts, dismiss])

  return (
    <SkinContext.Provider value={{ skin: 'md3' }}>
      <I18nContext.Provider value={i18n}>
        <ToastContext.Provider value={toast}>{children}</ToastContext.Provider>
      </I18nContext.Provider>
    </SkinContext.Provider>
  )
}
