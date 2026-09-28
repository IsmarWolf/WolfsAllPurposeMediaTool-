import { useContext } from 'react'
import { I18nContext, SkinContext, ToastContext } from '../app/contexts'

/** §10.3 — the context is the store, the hook is the read. */
export function useI18n() {
  const context = useContext(I18nContext)
  if (!context) {
    throw new Error('useI18n must be used inside <Providers>')
  }
  return context
}

export function useSkin() {
  const context = useContext(SkinContext)
  if (!context) {
    throw new Error('useSkin must be used inside <Providers>')
  }
  return context
}

export function useToast() {
  const context = useContext(ToastContext)
  if (!context) {
    throw new Error('useToast must be used inside <Providers>')
  }
  return context
}
