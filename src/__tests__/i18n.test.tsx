import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { Providers } from '../app/providers'
import { useI18n } from '../hooks/useContexts'
import en from '../i18n/en.json'
import ptBR from '../i18n/pt-BR.json'
import type { RootSourceDto } from '../types/api'

function Probe() {
  const { t } = useI18n()
  return (
    <>
      <span data-testid="known">{t('nav.media')}</span>
      <span data-testid="missing">{t('does.not.exist')}</span>
      <span data-testid="params">{t('bulk.count', { count: 7 })}</span>
    </>
  )
}

describe('i18n', () => {
  it('translates a known key', () => {
    render(
      <Providers>
        <Probe />
      </Providers>,
    )
    expect(screen.getByTestId('known').textContent).toBe('Mídia')
  })

  // §15: the fallback chain must end at the key, never at a blank.
  it('falls back to the key itself when nothing is translated', () => {
    render(
      <Providers>
        <Probe />
      </Providers>,
    )
    expect(screen.getByTestId('missing').textContent).toBe('does.not.exist')
  })

  it('interpolates params', () => {
    render(
      <Providers>
        <Probe />
      </Providers>,
    )
    expect(screen.getByTestId('params').textContent).toBe('7 selecionados')
  })
})

describe('i18n coverage', () => {
  // Settings renders `t(\`root.${info.source}\`)`, so every arm the Rust
  // `RootSource` enum can send needs a label. `RootSourceDto` came from the
  // backend contract, so deriving the list from it makes a new arm fail here
  // instead of showing a raw key on the screen.
  it('labels every RootSource the backend can report', () => {
    const sources: RootSourceDto[] = [
      'wolfsRootEnv',
      'markerWalk',
      'packagedAppDir',
      'unrootedFallback',
    ]
    for (const source of sources) {
      expect(ptBR[`root.${source}`], `pt-BR root.${source}`).toBeTruthy()
      expect(en[`root.${source}`], `en root.${source}`).toBeTruthy()
    }
  })

  it('keeps both locales in step', () => {
    expect(Object.keys(en).sort()).toEqual(Object.keys(ptBR).sort())
  })
})
