import { useContext } from 'react'
import { SkinContext, type SkinContextValue } from '../app/contexts'

export function useSkin(): SkinContextValue {
  const context = useContext(SkinContext)

  if (!context) {
    throw new Error('useSkin must be used inside Providers')
  }

  return context
}
