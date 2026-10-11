import { useContext } from 'react'
import {
  I18nContext,
  SkinContext,
  ToastContext,
  TransferContext,
  type TransferContextValue,
} from '../app/contexts'

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

/**
 * §11.3's ingest hooks, owned by `TransferProvider` for the session.
 *
 * Throws rather than returning a default: every consumer here renders a bar or a
 * pane whose whole content *is* this state, so a silent `null` would produce an
 * empty pane that looks like a backend which never reports — the exact class of
 * quiet lie the pane comments keep refusing.
 */
export function useTransfer(): TransferContextValue {
  const context = useContext(TransferContext)
  if (!context) {
    throw new Error('useTransfer must be used inside <TransferProvider>')
  }
  return context
}
