import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { AppShell } from '../app/AppShell'
import { Providers } from '../app/providers'
import { useToast } from '../hooks/useContexts'
import { Inspector } from '../components/layout/Inspector'
import { AppError, type ErrorCode } from '../lib/errors'

const STATS = {
  totalItems: 1234,
  images: 900,
  videos: 334,
  noMetadata: 42,
  hidden: 7,
  totalBytes: 8 * 1024 * 1024 * 1024,
  imagesBytes: 6 * 1024 * 1024 * 1024,
  videosBytes: 2 * 1024 * 1024 * 1024,
  deviceCount: 2,
  topDevices: [{ name: 'iPhone', count: 1200, bytes: 8 * 1024 * 1024 * 1024 }],
}

const ROOT = {
  root: 'E:\\WolfsMedia',
  source: 'markerWalk' as const,
  rooted: true,
  firstRun: false,
  ffmpegOk: false,
}

const VAULT = { configured: false, unlocked: false, hasRecoveryHint: false }
const VERSIONS = { appVersion: '0.1.0', os: 'windows', arch: 'x86_64' }

// Both mocks are hoisted above these declarations, so each factory builds its
// own `vi.fn()` and the module under test receives the very same instance the
// test asserts on. A `const` captured from the test body would be in the TDZ
// when the hoisted factory runs.
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }))

const { invoke } = await import('@tauri-apps/api/core')
const invokeMock = vi.mocked(invoke)

/** The happy path every screen needs; individual tests override one command. */
function defaultMock(command: string): unknown {
  switch (command) {
    case 'stats_get':
      return STATS
    case 'resolve_app_root':
      return ROOT
    case 'vault_status':
      return VAULT
    case 'app_versions':
      return VERSIONS
    default:
      throw new Error(`comando não mockado: ${command}`)
  }
}

/** Every test starts from the happy path and overrides only the command it needs. */
beforeEach(() => {
  invokeMock.mockReset()
  invokeMock.mockImplementation(async (command: string) => defaultMock(command))
})

async function renderShell() {
  const result = render(
    <Providers>
      <AppShell />
    </Providers>,
  )
  await screen.findByTestId('c-1a-stats')
  return result
}

async function gotoSettings() {
  const { container } = await renderShell()
  const settings = [...container.querySelectorAll('.zone2__item')].find((item) =>
    item.textContent?.includes('Ajustes'),
  ) as HTMLElement
  fireEvent.click(settings)
  return { container }
}

describe('shell contract (§10.6.3 / §10.6.6, C16)', () => {
  it('renders all four zones', async () => {
    const { container } = await renderShell()
    expect(container.querySelector('.zone1')).not.toBeNull()
    expect(container.querySelector('.zone2')).not.toBeNull()
    expect(container.querySelector('.zone3')).not.toBeNull()
    expect(container.querySelector('.zone4')).not.toBeNull()
  })

  // The review rule is a test, not a convention.
  it('Zone 2 contains only nav items', async () => {
    await renderShell()
    const sidebar = document.querySelector('.zone2') as HTMLElement
    const interactive = sidebar.querySelectorAll('button, a, input, select')
    expect(interactive.length).toBeGreaterThan(0)
    for (const node of interactive) {
      expect(node.getAttribute('data-testid')).toBe('nav-item')
    }
  })

  it('marks the Cofre as a security nav item', async () => {
    const { container } = await renderShell()
    const vault = container.querySelector('.zone2__item--vault') as HTMLElement
    expect(vault).not.toBeNull()
    expect(vault.className).toContain('zone2__item--vault')
    expect(vault.textContent).toContain('Cofre')
  })

  it('hides the bulk toolbar while nothing is selected', async () => {
    const { container } = await renderShell()
    expect(container.querySelector('.zone5')).toBeNull()
  })
})

describe('Zone 4 close control (human-confirmed 2026-09-28)', () => {
  function renderInspector(mode: 'summary' | 'single' | 'bulk', onClose = vi.fn()) {
    render(
      <Providers>
        <Inspector
          mode={mode}
          count={mode === 'bulk' ? 3 : mode === 'single' ? 1 : 0}
          onClose={onClose}
        />
      </Providers>,
    )
    return { onClose }
  }

  // The rule: a close control only when it closes something. The X clears the
  // selection, so in `summary` - where there is no selection - it was a button
  // wired to a no-op that read as "closes Resumo".
  it('has no close button in the summary tab', () => {
    renderInspector('summary')
    expect(screen.queryByRole('button', { name: 'Fechar' })).toBeNull()
  })

  it('offers the close button when a selection is what it would close', () => {
    const { onClose } = renderInspector('single')
    fireEvent.click(screen.getByRole('button', { name: 'Fechar' }))
    expect(onClose).toHaveBeenCalledTimes(1)
  })

  it('still names the mode it is showing', () => {
    renderInspector('summary')
    expect(screen.getByRole('heading', { name: 'Resumo' })).toBeTruthy()
  })
})

describe('error surfaces are translated (§8.2)', () => {
  // The bug this pins: Settings rendered `error.i18nKey ?? error.detail`, so a
  // rejected command put the literal `error.E_ROOT` on screen - the key instead
  // of the sentence, in a screen whose whole job is diagnostics.
  it('Settings shows the translated sentence, never the key', async () => {
    invokeMock.mockImplementation(async (command: string) => {
      if (command === 'resolve_app_root') {
        throw { code: 'E_ROOT', message: 'raiz ausente' }
      }
      return defaultMock(command)
    })

    await gotoSettings()

    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toBe('A pasta raiz do SSD não pôde ser usada.')
  })

  // Zone 1 has no error card of its own — it keeps the "unknown" badge — so the
  // toast is the only honest surface for an unresolvable root (§8.2).
  it('reports a failed root resolution from Zone 1', async () => {
    invokeMock.mockImplementation(async (command: string) => {
      if (command === 'resolve_app_root') {
        throw { code: 'E_ROOT', message: 'raiz ausente' }
      }
      return defaultMock(command)
    })

    const { container } = await renderShell()

    // The header also owns a `role="status"`, so the toast is found by its text
    // and identified by its kind modifier.
    const toast = await screen.findByText('A pasta raiz do SSD não pôde ser usada.')
    expect(toast.closest('.md-toast--error')).not.toBeNull()
    expect(container.querySelector('.dash-system__root')?.textContent).toBe('Raiz desconhecida')
  })

  function renderProbes(codes: readonly ErrorCode[]) {
    function Probes() {
      const { errorText } = useToast()
      return (
        <>
          {codes.map((code) => (
            <p key={code} data-testid={`probe-${code}`}>
              {errorText(new AppError(code, 'detalhe técnico'))}
            </p>
          ))}
        </>
      )
    }
    render(
      <Providers>
        <Probes />
      </Providers>,
    )
  }

  it('routes every code with a key through the dictionary', () => {
    renderProbes(['E_DB', 'E_AUTH'])
    expect(screen.getByTestId('probe-E_DB').textContent).toBe(
      'Não foi possível ler o banco de dados.',
    )
    expect(screen.getByTestId('probe-E_AUTH').textContent).toBe(
      'Acesso negado. Verifique a autorização do cofre.',
    )
  })

  // A null key is the only shape left for the detail to be the honest text, so
  // the fallback must not render a blank line.
  it('uses the detail when the code carries no key', () => {
    renderProbes(['E_CANCEL'])
    expect(screen.getByTestId('probe-E_CANCEL').textContent).toBe('detalhe técnico')
  })

  // The false "done" bug: `useTauriCommand` swallows the rejection into a toast,
  // so a chained `.then()` fired its success message even when the command had
  // failed - the user saw "concluído" and an error at the same time.
  it('never reports a vacuum that failed as done', async () => {
    invokeMock.mockImplementation(async (command: string) => {
      if (command === 'settings_vacuum') {
        throw { code: 'E_DB', message: 'banco bloqueado' }
      }
      return defaultMock(command)
    })

    await gotoSettings()
    fireEvent.click(await screen.findByRole('button', { name: 'Compactar banco' }))

    const toasts = await screen.findByText('Não foi possível ler o banco de dados.')
    expect(toasts.closest('.md-toast--error')).not.toBeNull()
    expect(screen.queryByText('Banco compactado.')).toBeNull()
  })
})

describe('no refetch loop (§10.3 useAsync)', () => {
  // A `useCallback([fn])` on an inline loader re-ran the effect on every render,
  // which showed up as numbers blinking between "loading" and 0.
  it('reads the stats exactly once per mount', async () => {
    await renderShell()
    const statsCalls = invokeMock.mock.calls.filter(([command]) => command === 'stats_get')
    expect(statsCalls).toHaveLength(1)
  })

  it('never shows a 0 before the first read lands', async () => {
    // Only `stats_get` stalls: the child effects of Zone 3 fire before the
    // shell's own, so `mockImplementationOnce` would have hijacked another
    // command.
    invokeMock.mockImplementation(async (command: string) => {
      if (command === 'stats_get') {
        return new Promise(() => {})
      }
      return defaultMock(command)
    })

    const { container } = render(
      <Providers>
        <AppShell />
      </Providers>,
    )

    const card = await screen.findByTestId('c-1a-stats')
    expect(card.querySelector('.md-skeleton')).not.toBeNull()
    expect(card.textContent).not.toContain('0')
    expect(container.querySelector('.zone5')).toBeNull()
  })
})
