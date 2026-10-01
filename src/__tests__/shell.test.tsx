import { act, fireEvent, render, screen } from '@testing-library/react'
import { useState } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { AppShell } from '../app/AppShell'
import { Providers } from '../app/providers'
import { useToast } from '../hooks/useContexts'
import { Inspector } from '../components/layout/Inspector'
import { AppError, type ErrorCode } from '../lib/errors'
import type { ScanProgressDto } from '../types/api'

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
/** What the native Windows folder dialog "confirms" — §11.3's Disco-local source. */
const PICKED = 'C:\\Users\\Ismar\\Pictures\\Familia'

// Both mocks are hoisted above these declarations, so each factory builds its
// own `vi.fn()` and the module under test receives the very same instance the
// test asserts on. A `const` captured from the test body would be in the TDZ
// when the hoisted factory runs.
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))

const { invoke } = await import('@tauri-apps/api/core')
const invokeMock = vi.mocked(invoke)
const { open } = await import('@tauri-apps/plugin-dialog')
const openMock = vi.mocked(open)

/** The happy path every screen needs; individual tests override one command. */
function defaultMock(command: string, _args?: unknown): unknown {
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
  invokeMock.mockImplementation(async (command: string, args?: unknown) =>
    defaultMock(command, args),
  )
  openMock.mockReset()
  openMock.mockResolvedValue(PICKED)
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

describe('Zone 4 as a drawer below 1280px (human-confirmed 2026-09-29)', () => {
  // The bug this pins: below 1280px `.zone4` was `display: none` and nothing in
  // the app could bring it back, so the inspector simply did not exist on a
  // narrow window. It is now an overlay drawer behind a gutter tab; the *size
  // class* stays in `shell.css` (§10.6.3), and JS only carries the state.
  function ControlledInspector({ mode }: { mode: 'summary' | 'single' | 'bulk' }) {
    const [open, setOpen] = useState(false)
    return (
      <Inspector
        mode={mode}
        count={1}
        onClose={vi.fn()}
        drawerOpen={open}
        onDrawerChange={setOpen}
      />
    )
  }

  function renderDrawer(mode: 'summary' | 'single' | 'bulk' = 'single') {
    const result = render(
      <Providers>
        <ControlledInspector mode={mode} />
      </Providers>,
    )
    return { ...result, toggle: () => screen.getByRole('button', { name: /inspetor/i }) }
  }

  it('exposes a labelled control that reports whether the drawer is open', () => {
    const { toggle } = renderDrawer()
    const tab = toggle()
    expect(tab.getAttribute('aria-expanded')).toBe('false')
    fireEvent.click(tab)
    expect(tab.getAttribute('aria-expanded')).toBe('true')
  })

  it('moves focus into the drawer and gives it back on close', () => {
    const { toggle } = renderDrawer()
    const tab = toggle()

    fireEvent.click(tab)
    const panel = document.querySelector('.zone4--drawer-open') as HTMLElement
    expect(panel.getAttribute('role')).toBe('dialog')
    expect(panel.getAttribute('aria-modal')).toBe('true')
    expect(document.activeElement).toBe(panel)

    fireEvent.click(tab)
    expect(document.activeElement).toBe(tab)
  })

  it('leaves the panel a plain region while it is a column, not a dialog', () => {
    // `role="dialog"` at >= 1280px would trap focus inside a panel the user never
    // opened as an overlay.
    render(
      <Providers>
        <Inspector mode="single" count={1} onClose={vi.fn()} />
      </Providers>,
    )
    const panel = document.querySelector('.zone4') as HTMLElement
    expect(panel.getAttribute('role')).toBeNull()
    expect(panel.className).not.toContain('zone4--drawer-open')
  })

  it('closes on a scrim click, and the scrim is not in the a11y tree', () => {
    const { container, toggle } = renderDrawer()
    fireEvent.click(toggle())
    const scrim = container.querySelector('.zone4__scrim') as HTMLElement
    expect(scrim.getAttribute('aria-hidden')).toBe('true')

    fireEvent.click(scrim)
    expect(container.querySelector('.zone4--drawer-open')).toBeNull()
  })

  // A tab that opens a panel with nothing to inspect is a dead control, and the
  // summary mode is exactly that today (its totals arrive in c8).
  it('is disabled when the panel has nothing to inspect', () => {
    const { toggle } = renderDrawer('summary')
    expect((toggle() as HTMLButtonElement).disabled).toBe(true)
  })

  it('renders no drawer control on a screen with no inspector at all', () => {
    // §10.6.3: Backup and Settings have no selection model, so Zone 4 collapses
    // to width 0 and there is nothing for a tab to open.
    render(
      <Providers>
        <Inspector
          mode="none"
          count={0}
          onClose={vi.fn()}
          drawerOpen={false}
          onDrawerChange={vi.fn()}
        />
      </Providers>,
    )
    expect(document.querySelector('.zone4')).toBeNull()
    expect(screen.queryByRole('button', { name: /inspetor/i })).toBeNull()
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

describe('Disco local (C-3n, c8) — the copy-only ingest (§11.3)', () => {
  const SCAN_DONE = {
    device: 'Fotos',
    found: 12,
    inserted: 3,
    duplicates: 8,
    repaired: 1,
    organized: 2,
    noMetadata: 1,
    located: 0,
    locatedNone: 0,
    skipped: 0,
    cancelled: false,
  }

  /**
   * Backup → Disco local → confirm `C:\Users\Ismar\Pictures\Familia` in the
   * native Windows dialog (§11.3, human-confirmed 2026-10-01). The dialog is
   * mocked, so "choosing" is the single `open()` call resolving a path; there is
   * no in-app navigation to walk.
   */
  async function openBackupLocal() {
    await renderShell()
    const backup = [...document.querySelectorAll('.zone2__item')].find((item) =>
      item.textContent?.includes('Backup'),
    ) as HTMLElement
    fireEvent.click(backup)
    fireEvent.click(await screen.findByRole('tab', { name: 'Disco local' }))
    fireEvent.click(screen.getByRole('button', { name: 'Escolher pasta…' }))
    // The chosen path shows up as the card's read-only hint, and the label the
    // backend derives is the folder's own name.
    expect(await screen.findByText(PICKED)).not.toBeNull()
    expect(openMock).toHaveBeenCalledWith(
      expect.objectContaining({ directory: true, multiple: false }),
    )
  }

  it('scan_start goes out with the folder and the summary becomes a toast', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'scan_start') {
        return SCAN_DONE
      }
      return defaultMock(command, args)
    })

    await openBackupLocal()
    fireEvent.click(screen.getByTestId('c-3n4-start'))

    const scanCalls = invokeMock.mock.calls.filter(([command]) => command === 'scan_start')
    expect(scanCalls).toHaveLength(1)
    expect(scanCalls[0][1]).toEqual({ device: 'Familia', folder: PICKED })

    expect(
      await screen.findByText('Escaneado: 3 novo(s), 8 repetido(s), 1 reparado(s).'),
    ).not.toBeNull()
  })

  it('cancelling the native dialog leaves the card without a folder', async () => {
    openMock.mockResolvedValue(null)

    await renderShell()
    const backup = [...document.querySelectorAll('.zone2__item')].find((item) =>
      item.textContent?.includes('Backup'),
    ) as HTMLElement
    fireEvent.click(backup)
    fireEvent.click(await screen.findByRole('tab', { name: 'Disco local' }))
    fireEvent.click(screen.getByRole('button', { name: 'Escolher pasta…' }))

    // Still asking, still no `scan_start`, and the button is offered again.
    expect(screen.queryByTestId('c-3n4-start')).toBeNull()
    expect(screen.getByRole('button', { name: 'Escolher pasta…' })).not.toBeNull()
    expect(invokeMock.mock.calls.filter(([command]) => command === 'scan_start')).toHaveLength(0)
  })

  it('a running import offers Cancelar instead of Iniciar, and it cancels', async () => {
    // The scan never resolves, so the button stays in the running branch.
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'scan_start') {
        return new Promise(() => {})
      }
      if (command === 'scan_cancel') {
        return undefined
      }
      return defaultMock(command, args)
    })

    await openBackupLocal()
    fireEvent.click(screen.getByTestId('c-3n4-start'))

    const cancelButton = await screen.findByRole('button', { name: 'Cancelar' })
    fireEvent.click(cancelButton)

    const cancelCalls = invokeMock.mock.calls.filter(([command]) => command === 'scan_cancel')
    expect(cancelCalls).toHaveLength(1)
    expect(screen.queryByTestId('c-3n4-start')).toBeNull()
  })

  it('a cancelled scan reports it instead of claiming success', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'scan_start') {
        return { ...SCAN_DONE, cancelled: true }
      }
      return defaultMock(command, args)
    })

    await openBackupLocal()
    fireEvent.click(screen.getByTestId('c-3n4-start'))

    expect(await screen.findByText('Escaneamento cancelado — nada foi perdido.')).not.toBeNull()
    expect(screen.queryByText(/Escaneado:/)).toBeNull()
  })

  it('the scan bar switches from indeterminate to a real total on the event', async () => {
    // The scan stays open while the bar is observed, so the running branch can
    // be asserted without racing a promise that resolves on the next microtask.
    let resolveScan: (summary: typeof SCAN_DONE) => void = () => {}
    const pending = new Promise<typeof SCAN_DONE>((resolve) => {
      resolveScan = resolve
    })
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'scan_start') {
        return pending
      }
      return defaultMock(command, args)
    })

    await openBackupLocal()
    fireEvent.click(screen.getByTestId('c-3n4-start'))

    // Before any event, `total` is 0, so the hint reads "scanning the folder".
    expect(await screen.findByText('Varrendo a pasta…')).not.toBeNull()

    const { listen } = await import('@tauri-apps/api/event')
    const listenMock = vi.mocked(listen)
    const handler = listenMock.mock.calls.find(
      ([channel]) => channel === 'wolfs://progress/scan',
    )?.[1]
    expect(handler).not.toBeUndefined()
    const emitScan = handler as (event: { payload: ScanProgressDto }) => void
    act(() => {
      emitScan({
        payload: { current: 3, total: 12, lastPath: 'Media/Fotos/2024/03/a.jpg', inserted: 1 },
      })
    })

    // A real total replaces the message with a fraction, and the bar is the
    // fill, not the message.
    expect(await screen.findByText('3 / 12')).not.toBeNull()
    expect(screen.queryByText('Varrendo a pasta…')).toBeNull()

    await act(async () => {
      resolveScan(SCAN_DONE)
    })
    expect(await screen.findByText(/Escaneado:/)).not.toBeNull()
  })
})
