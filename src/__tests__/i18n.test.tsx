import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { Providers } from '../app/providers'
import { useI18n } from '../hooks/useContexts'

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
