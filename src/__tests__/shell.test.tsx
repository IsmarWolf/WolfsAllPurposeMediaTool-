import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
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

const MEDIA_PAGE = {
  items: [
    {
      id: 'g1',
      relativePath: 'Media/PC/2024/03/IMG_1.jpg',
      thumb200: 'Thumbnails/200/PC/2024/03/IMG_1.webp',
      thumb400: 'Thumbnails/400/PC/2024/03/IMG_1.webp',
      fileHash: 'h1',
      fileSize: 1000,
      fileType: 'image',
      capturedAt: '2024-03-15T10:00:00',
      hasMetadata: true,
      deviceName: 'PC',
      isHidden: false,
      location: null,
    },
    {
      id: 'g2',
      relativePath: 'Media/PC/2024/04/VID_2.mov',
      thumb200: 'Thumbnails/200/PC/2024/04/VID_2.webp',
      thumb400: 'Thumbnails/400/PC/2024/04/VID_2.webp',
      fileHash: 'h2',
      fileSize: 2000,
      fileType: 'video',
      capturedAt: '2024-04-01T10:00:00',
      hasMetadata: true,
      deviceName: 'PC',
      isHidden: false,
      location: null,
    },
  ],
  total: 2,
  hasMore: false,
}

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
    case 'media_query':
      return { items: [], total: 0, hasMore: false }
    case 'media_detail':
      return MEDIA_PAGE.items[0]
    case 'media_scope_video':
      return null
    // Lists (c16) — the Lists screen owns these; the defaults keep any
    // accidental mount honest (an empty index, no detail) instead of throwing.
    case 'lists_list':
      return []
    case 'lists_detail':
      return null
    case 'lists_create':
    case 'lists_update':
    case 'lists_delete':
    case 'lists_add_items':
    case 'lists_remove_items':
    case 'lists_snapshot_apply':
    case 'lists_snapshot_sync':
      return null
    // The Backup screen mounts the USB hook for every tab (the job lives in the
    // Rust `JobRegistry`, so the subscription must not depend on which tab is
    // open). The truthful default in a test is an empty cable.
    case 'devices_list':
      return []
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

/**
 * Broadcast an `wolfs://progress/ingest` payload to **every** subscriber.
 *
 * §9.2 gives USB and Wi-Fi one channel with no job id, so the app really does
 * have two listeners on it and the real runtime delivers to both. A test that
 * picks `listen.mock.calls.find(...)` is therefore testing mount order, not
 * behaviour: it passed only while `useWifiIngest` happened to register before
 * `useUsbIngest`, and it broke the moment the provider ordered them the other
 * way — which is the whole reason this helper exists.
 */
async function ingestAll(): Promise<(payload: unknown) => void> {
  const { listen } = await import('@tauri-apps/api/event')
  const handlers = vi
    .mocked(listen)
    .mock.calls.filter(([channel]) => channel === 'wolfs://progress/ingest')
    .map(([, handler]) => handler as (event: { payload: unknown }) => void)
  expect(handlers.length).toBeGreaterThan(0)
  return (payload: unknown) => {
    for (const handler of handlers) {
      handler({ payload })
    }
  }
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

    // Scoped to the pane: the shell's bottom bar carries its own Cancelar for the
    // same job (§9.3's job outlives the screen), so a bare role+name query now
    // matches two buttons — and clicking the wrong one would still "pass".
    const pane = within(screen.getByTestId('c-3n-local'))
    const cancelButton = await pane.findByRole('button', { name: 'Cancelar' })
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

    const pane = within(screen.getByTestId('c-3n-local'))

    // Before any event, `total` is 0, so the hint reads "scanning the folder".
    expect(await pane.findByText('Varrendo a pasta…')).not.toBeNull()
    // The bar says the same thing in its own words and stays indeterminate: a
    // `total: 0` pass must not paint a fraction it does not have.
    const bar = screen.getByTestId('transfer-bar')
    expect(bar.getAttribute('data-phase')).toBe('listing')
    expect(within(bar).getByText('contagem desconhecida')).not.toBeNull()
    expect(within(bar).getByRole('progressbar').hasAttribute('aria-valuenow')).toBe(false)

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
    // fill, not the message. Both surfaces show it now, so each is asserted in
    // its own scope.
    expect(await pane.findByText('3 / 12')).not.toBeNull()
    expect(screen.getByTestId('transfer-bar-count').textContent).toBe('3 / 12')
    expect(screen.getByTestId('transfer-bar').getAttribute('data-phase')).toBe('index')
    expect(screen.getByTestId('transfer-bar-fill').style.width).toBe('25%')
    expect(pane.queryByText('Varrendo a pasta…')).toBeNull()

    await act(async () => {
      resolveScan(SCAN_DONE)
    })
    // The summary closes the job, so the bar leaves with it — a strip that stayed
    // would claim work that is already over.
    expect(await screen.findByText(/Escaneado:/)).not.toBeNull()
    expect(screen.queryByTestId('transfer-bar')).toBeNull()
  })
})

describe("Backup → Wi-Fi (c13) — the pane over c11's real listener (§11.3)", () => {
  const WIFI_INFO = {
    lanIp: '192.168.0.12',
    qrMatrix: [
      [true, false, true],
      [false, true, false],
      [true, false, true],
    ],
    port: 8642,
    uploads: 0,
  }

  /** Backup → the Wi-Fi source tab. */
  async function openWifi() {
    await renderShell()
    const backup = [...document.querySelectorAll('.zone2__item')].find((item) =>
      item.textContent?.includes('Backup'),
    ) as HTMLElement
    fireEvent.click(backup)
    fireEvent.click(await screen.findByRole('tab', { name: 'Wi-Fi' }))
  }

  function serveWith(info: unknown = WIFI_INFO) {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'ingest_wifi_start') {
        return info
      }
      return defaultMock(command, args)
    })
  }

  it('starts the server and shows the address, the QR and the upload counter', async () => {
    serveWith()
    await openWifi()

    fireEvent.click(screen.getByTestId('c-3i-start'))

    expect(
      invokeMock.mock.calls.filter(([command]) => command === 'ingest_wifi_start'),
    ).toHaveLength(1)
    expect(await screen.findByText('http://192.168.0.12:8642')).not.toBeNull()
    expect(screen.getByRole('img', { name: 'QR code com o endereço de importação' })).not.toBeNull()
    expect(screen.getByTestId('c-3i-waiting').textContent).toContain('0 recebido(s)')

    // The start button is replaced by Stop, not shown alongside it.
    expect(screen.queryByTestId('c-3i-start')).toBeNull()
    expect(screen.getByTestId('c-3k-stop')).not.toBeNull()
  })

  it('stop calls the command and offers the start button back', async () => {
    serveWith()
    await openWifi()
    fireEvent.click(screen.getByTestId('c-3i-start'))
    await screen.findByText('http://192.168.0.12:8642')

    fireEvent.click(screen.getByTestId('c-3k-stop'))

    expect(
      invokeMock.mock.calls.filter(([command]) => command === 'ingest_wifi_stop'),
    ).toHaveLength(1)
    expect(await screen.findByTestId('c-3i-start')).not.toBeNull()
    expect(screen.queryByTestId('c-3i-waiting')).toBeNull()
  })

  it('an ingest progress event drives the fraction (the channel is live)', async () => {
    serveWith()
    await openWifi()
    fireEvent.click(screen.getByTestId('c-3i-start'))
    await screen.findByText('http://192.168.0.12:8642')

    const handler = await ingestAll()

    act(() => {
      handler({
        kind: 'progress',
        current: 4,
        total: 20,
        lastPath: 'Media/PC/2024/03/IMG_4.jpg',
        inserted: 4,
      })
    })
    // Two places now paint it: the pane's meter and the shell's bottom bar. The
    // bar is the point of this slice, so both are asserted — by their own scope,
    // because a bare `getByText` would only prove one of them exists.
    expect(await screen.findAllByText('4 / 20')).toHaveLength(2)
    expect(screen.getByTestId('transfer-bar-count').textContent).toBe('4 / 20')

    act(() => {
      handler({
        kind: 'summary',
        device: 'PC',
        found: 20,
        inserted: 9,
        duplicates: 11,
        repaired: 0,
        organized: 0,
        noMetadata: 2,
        located: 0,
        locatedNone: 0,
        skipped: 0,
        cancelled: false,
      })
    })
    const summary = await screen.findByTestId('c-3i-summary')
    expect(summary.textContent).toContain('9 novo(s)')
    expect(summary.textContent).toContain('11 repetido(s)')
    // The summary replaces the live fraction rather than stacking under it, and
    // it takes the bottom bar down with it: nothing is running any more.
    expect(screen.queryByText('4 / 20')).toBeNull()
    expect(screen.queryByTestId('transfer-bar')).toBeNull()
  })

  it('a device failure is shown, and names the device (§12.1)', async () => {
    serveWith()
    await openWifi()
    fireEvent.click(screen.getByTestId('c-3i-start'))
    await screen.findByText('http://192.168.0.12:8642')

    const handler = await ingestAll()

    act(() => {
      handler({ kind: 'failed', device: 'iPhone', message: 'MTP timeout' })
    })

    expect(await screen.findByText(/Falha em iPhone: MTP timeout/)).not.toBeNull()
  })

  /**
   * The cross-talk that §9.2's channel shape makes possible, and the part of it the
   * liveness guard closes: both ingest hooks are subscribed to one channel with no
   * job id, so with nothing running a stray `failed` has no business painting any
   * pane.
   *
   * What it does **not** close is the overlap — a USB pass failing while the Wi-Fi
   * server is up, or an upload failing during a USB pass — because liveness is
   * true on both sides then. That needs a source tag on `IngestEventDto` (§8.1),
   * which is a contract decision and not one to take silently. Until then the
   * bar's precedence (USB first) keeps the *bar* honest about which job it is
   * painting, but the Wi-Fi pane can still show a phone error.
   */
  it('a failure with nothing running lands in no pane at all', async () => {
    await openWifi()

    const handler = await ingestAll()
    act(() => {
      handler({ kind: 'failed', device: 'iPhone', message: 'cabo solto' })
    })

    expect(screen.queryByText(/Falha em iPhone: cabo solto/)).toBeNull()
    expect(screen.queryByTestId('transfer-bar')).toBeNull()
  })

  /**
   * The bar offers no Cancelar here, and that is a deliberate absence rather than
   * a missing feature: `ingest_wifi_stop` tears the listener down, it does not
   * abort the upload in flight. A button that said "Cancelar" and then stopped
   * the server would leave the phone mid-sentence with no way to retry, so the bar
   * claims only what the command can actually do.
   */
  it('the bar has no cancel for an upload, because the command cannot abort one', async () => {
    serveWith()
    await openWifi()
    fireEvent.click(screen.getByTestId('c-3i-start'))
    await screen.findByText('http://192.168.0.12:8642')

    const handler = await ingestAll()
    act(() => {
      handler({
        kind: 'progress',
        current: 4,
        total: 20,
        lastPath: 'Media/PC/2024/03/IMG_4.jpg',
        inserted: 4,
      })
    })

    const bar = await screen.findByTestId('transfer-bar')
    expect(bar.getAttribute('data-kind')).toBe('wifi')
    expect(within(bar).queryByTestId('transfer-bar-cancel')).toBeNull()
  })

  /**
   * `ingest_wifi_start` is idempotent and doubles as the pane's poll (§8.1), so a
   * poll already in flight when the human presses Stop must not resurrect the
   * listener. This is the one invariant that is invisible in the UI and easy to
   * regress, so it is pinned here: the late answer is discarded.
   */
  it('a poll still in flight when Stop lands does not bring the server back', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    try {
      let release: (value: unknown) => void = () => {}
      const inFlight = new Promise((resolve) => {
        release = resolve
      })
      let polls = 0
      invokeMock.mockImplementation(async (command: string, args?: unknown) => {
        if (command === 'ingest_wifi_start') {
          polls += 1
          // The first call starts the server and answers; the poll hangs.
          return polls === 1 ? WIFI_INFO : inFlight
        }
        return defaultMock(command, args)
      })

      await openWifi()
      fireEvent.click(screen.getByTestId('c-3i-start'))
      await screen.findByText('http://192.168.0.12:8642')

      // Fire the poll, which will not have answered by the time Stop is pressed.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2000)
      })
      expect(polls).toBe(2)

      await act(async () => {
        fireEvent.click(screen.getByTestId('c-3k-stop'))
      })

      // The hung poll finally answers — too late, and it must be ignored.
      await act(async () => {
        release(WIFI_INFO)
        await inFlight
      })

      expect(screen.queryByTestId('c-3i-waiting')).toBeNull()
      expect(screen.getByTestId('c-3i-start')).not.toBeNull()
    } finally {
      vi.useRealTimers()
    }
  })
})

describe("Backup → USB (c12) — the pane over c12's real WPD/MTP transport (§11.3)", () => {
  const PHONE = { id: 'usb\\VID_1234', name: 'iPhone', friendlyName: 'iPhone do Ismar' }
  const SECOND_PHONE = { id: 'usb\\VID_5678', name: 'Android', friendlyName: 'Pixel do Ismar' }

  /** Backup → the USB source tab. */
  async function openUsb() {
    await renderShell()
    const backup = [...document.querySelectorAll('.zone2__item')].find((item) =>
      item.textContent?.includes('Backup'),
    ) as HTMLElement
    fireEvent.click(backup)
    fireEvent.click(await screen.findByRole('tab', { name: 'USB' }))
  }

  function withPhone(extra: (command: string, args?: unknown) => unknown = () => undefined) {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'devices_list') {
        return [PHONE]
      }
      if (command === 'ingest_usb_start') {
        return 'job-1'
      }
      const answer = extra(command, args)
      return answer === undefined ? defaultMock(command, args) : answer
    })
  }

  /** Two phones attached: `resolved` must not auto-select (§5.4 — picking the
   *  wrong one files photos under the wrong library folder). */
  function withTwoPhones() {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'devices_list') {
        return [PHONE, SECOND_PHONE]
      }
      return defaultMock(command, args)
    })
  }

  /** The list keeps answering, but the phone is gone from it. */
  function withoutPhones() {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'devices_list') {
        return []
      }
      return defaultMock(command, args)
    })
  }

  /** `devices_list` fails, everything else answers normally. The rejection is the
   *wire shape* Rust actually sends (§8.2's `{ code, message }`), not an `Error`:
   * a thrown `Error` carries neither field, so `toAppError` cannot classify it and
   * falls back to "falha inesperada" — which is how a real reason gets replaced by
   * a vague one in a test instead of only in production. */
  function withBrokenCable() {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'devices_list') {
        throw {
          code: 'E_DEVICE',
          message:
            'MTP indisponível neste sistema: The application called CoInitializeEx (0x80010106)',
        }
      }
      return defaultMock(command, args)
    })
  }

  /**
   * The live `wolfs://progress/ingest` handlers, as `useTauriEvent` registered
   * them. Both ingest hooks listen to the one channel §9.2 defines, so the event
   * is broadcast rather than delivered to whichever registered first — see
   * `ingestAll`.
   */
  async function ingest(): Promise<(payload: unknown) => void> {
    return ingestAll()
  }

  const DONE = {
    kind: 'summary',
    device: 'iPhone',
    found: 3,
    inserted: 2,
    duplicates: 1,
    repaired: 0,
    organized: 2,
    noMetadata: 0,
    located: 0,
    locatedNone: 0,
    skipped: 0,
    cancelled: false,
  }

  it('lists the attached device and shows where it will land (§11.3)', async () => {
    withPhone()
    await openUsb()

    expect(await screen.findByText('iPhone do Ismar')).not.toBeNull()
    // §5.4's label, not the raw friendly name: two phones can report the same one.
    expect(screen.getByText(/vai para Media\/iPhone\//)).not.toBeNull()
    expect(screen.getByText('Destino: Media/iPhone/')).not.toBeNull()
  })

  /**
   * §8.1's payload, verbatim. `copy` and `only_new: false` are the defaults on
   * purpose: copying is what cannot lose a photo, so it is not the thing a
   * mis-click should change.
   */
  it('starts a copy pass with the §8.1 payload', async () => {
    withPhone()
    await openUsb()
    fireEvent.click(await screen.findByTestId('c-3j-start'))

    await screen.findByTestId('c-3k-stop')
    const call = invokeMock.mock.calls.find(([command]) => command === 'ingest_usb_start')
    // Tauri wraps a struct argument in the parameter's name, and the command's
    // parameter *is* `payload` (§8.1) — so this is the exact wire shape.
    expect(call?.[1]).toEqual({
      payload: { deviceFriendly: 'iPhone do Ismar', mode: 'copy', onlyNew: false },
    })
  })

  it('move is opt-in, and only then does the pane warn about deleting', async () => {
    withPhone()
    await openUsb()
    fireEvent.click(await screen.findByTestId('c-3f-move'))
    fireEvent.click(screen.getByTestId('c-3f-only-new'))
    fireEvent.click(screen.getByTestId('c-3j-start'))

    await screen.findByTestId('c-3k-stop')
    const call = invokeMock.mock.calls.find(([command]) => command === 'ingest_usb_start')
    expect(call?.[1]).toEqual({
      payload: { deviceFriendly: 'iPhone do Ismar', mode: 'move', onlyNew: true },
    })
    // The destructive mode says so, in the language the user reads.
    expect(screen.getByText(/já foi indexado não é apagado/)).not.toBeNull()
  })

  /**
   * The pull is its own bar with its own count, because a 4 GB video is minutes
   * of pulling and a single scanned row. Conflating them is the bug this pins.
   */
  it('a pull event drives its own bar and names the file in flight', async () => {
    withPhone()
    await openUsb()
    fireEvent.click(await screen.findByTestId('c-3j-start'))
    const emitEvent = await ingest()

    act(() => {
      emitEvent({ kind: 'pull', current: 1, total: 3, lastName: 'IMG_0002.jpg' })
    })

    const pull = await screen.findByTestId('c-3e-pull')
    expect(pull.textContent).toContain('1 / 3')
    expect(within(pull).getByText('IMG_0002.jpg')).not.toBeNull()
    expect(screen.queryByTestId('c-3e-summary')).toBeNull()
  })

  /**
   * The one event c12's hardware runs turned on, and the reason the bar exists in
   * its current shape: `list_media` walks the whole device **before the first
   * byte**, and on the iPhone that measured ~4 minutes for 3 288 objects over
   * USB 2. `pull_phase` emits `0/0` for it, which means "I cannot know the size
   * yet" — not "nothing has happened".
   *
   * So all three of these are the same claim from three directions: the bar is
   * indeterminate (no `aria-valuenow`), the pane says it is reading the *phone*
   * rather than scanning a folder that does not exist, and the human is told why
   * the wait is long. Without this the pass looks hung and the obvious reaction
   * is to unplug the phone.
   */
  it('the listing is announced, indeterminate, and never reads as a folder scan', async () => {
    withPhone()
    await openUsb()
    fireEvent.click(await screen.findByTestId('c-3j-start'))
    const emitEvent = await ingest()

    act(() => {
      emitEvent({ kind: 'pull', current: 0, total: 0, lastName: '' })
    })

    const bar = await screen.findByTestId('transfer-bar')
    expect(bar.getAttribute('data-phase')).toBe('listing')
    expect(bar.getAttribute('data-kind')).toBe('usb')
    expect(within(bar).getByText('contagem desconhecida')).not.toBeNull()
    expect(within(bar).getByRole('progressbar').hasAttribute('aria-valuenow')).toBe(false)

    const pull = await screen.findByTestId('c-3e-pull')
    expect(pull.textContent).toContain('Lendo o aparelho')
    expect(screen.queryByText('Varrendo a pasta…')).toBeNull()
  })

  /**
   * The reason the bar is in the shell and not in this pane. §9.3's job lives in
   * the Rust `JobRegistry` and outlives the screen, so the work keeps going after
   * the human walks off to do something else — and before this slice the only
   * progress indicator was inside the pane they just left. `useTauriEvent`
   * unlistens on unmount, so a bar mounted here would have gone blank on
   * navigation and then caught up when they came back.
   */
  it('the bar survives leaving the Backup screen mid-pass', async () => {
    withPhone()
    await openUsb()
    fireEvent.click(await screen.findByTestId('c-3j-start'))
    const emitEvent = await ingest()

    act(() => {
      emitEvent({ kind: 'pull', current: 7, total: 3288, lastName: 'IMG_0007.HEIC' })
    })
    expect((await screen.findByTestId('transfer-bar-count')).textContent).toBe('7 / 3.288')

    // Navigate away, to a screen that mounts none of the ingest hooks.
    const dashboard = [...document.querySelectorAll('.zone2__item')].find((item) =>
      item.textContent?.includes('Nº geral'),
    ) as HTMLElement
    fireEvent.click(dashboard)

    expect(screen.queryByTestId('c-3e-pull')).toBeNull()
    const bar = screen.getByTestId('transfer-bar')
    expect(bar.getAttribute('data-phase')).toBe('copy')
    expect(within(bar).getByText('IMG_0007.HEIC')).not.toBeNull()
    expect(screen.getByTestId('transfer-bar-count').textContent).toBe('7 / 3.288')

    // And it is still live off-screen, not frozen at the value it had on unmount.
    act(() => {
      emitEvent({ kind: 'pull', current: 8, total: 3288, lastName: 'IMG_0008.HEIC' })
    })
    expect(screen.getByTestId('transfer-bar-count').textContent).toBe('8 / 3.288')
  })

  /** The bar's Cancelar is the same §8.1 command the pane's stop button calls. */
  it('the bar cancels the pass it is showing', async () => {
    withPhone()
    await openUsb()
    fireEvent.click(await screen.findByTestId('c-3j-start'))
    const emitEvent = await ingest()

    act(() => {
      emitEvent({ kind: 'pull', current: 1, total: 3, lastName: 'IMG_0002.jpg' })
    })
    fireEvent.click(await screen.findByTestId('transfer-bar-cancel'))

    expect(invokeMock.mock.calls.filter(([command]) => command === 'ingest_usb_stop')).toHaveLength(
      1,
    )
  })

  it('the summary reports the counts and ends the pass', async () => {
    withPhone()
    await openUsb()
    fireEvent.click(await screen.findByTestId('c-3j-start'))
    const emitEvent = await ingest()

    act(() => {
      emitEvent(DONE)
    })

    const summary = await screen.findByTestId('c-3e-summary')
    expect(summary.textContent).toContain('2 novo(s)')
    expect(summary.textContent).toContain('1 repetido(s)')
    // Start is offered back; the pass is over.
    expect(screen.getByTestId('c-3j-start')).not.toBeNull()
    expect(screen.queryByTestId('c-3k-stop')).toBeNull()
  })

  /**
   * A cancelled pass still sends a summary, with `cancelled: true` and zero
   * counters. Rendering the counters alone would read as a finished, empty import
   * — the exact fabrication c12's audit rejected — so the flag gets its own line.
   */
  it('a cancelled pass says cancelled instead of "0 imported"', async () => {
    withPhone()
    await openUsb()
    fireEvent.click(await screen.findByTestId('c-3j-start'))
    const emitEvent = await ingest()

    act(() => {
      emitEvent({ ...DONE, inserted: 0, cancelled: true })
    })

    expect((await screen.findByTestId('c-3e-cancelled')).textContent).toContain(
      'Escaneamento cancelado',
    )
    expect(screen.queryByTestId('c-3e-summary')).toBeNull()
    expect(screen.getByTestId('c-3j-start')).not.toBeNull()
  })

  /**
   * Stop flips a flag on the other side of the wire, so the button must stay a
   * Stop button until the backend's terminal event arrives. Offering Start back
   * early invites a second click that the §9.3 registry answers with
   * `E_CONFLICT`.
   */
  it('stop calls the command but does not offer start back until the job ends', async () => {
    withPhone()
    await openUsb()
    fireEvent.click(await screen.findByTestId('c-3j-start'))
    fireEvent.click(await screen.findByTestId('c-3k-stop'))

    expect(invokeMock.mock.calls.filter(([command]) => command === 'ingest_usb_stop')).toHaveLength(
      1,
    )
    expect(screen.queryByTestId('c-3j-start')).toBeNull()

    const emitEvent = await ingest()
    act(() => {
      emitEvent({ ...DONE, cancelled: true })
    })
    expect(await screen.findByTestId('c-3j-start')).not.toBeNull()
  })

  it('no device means an explanation, and nothing to press', async () => {
    await openUsb()

    expect((await screen.findByTestId('c-3e-none')).textContent).toContain(
      'Nenhum aparelho identificado',
    )
    // The button stays put so the pane does not jump when a phone is plugged in —
    // it is just not clickable yet.
    expect(screen.getByTestId('c-3j-start')).toHaveProperty('disabled', true)
  })

  /**
   * A refresh that fails with no list at all is the fatal kind: it replaces the
   * device area rather than sitting under it, because there is nothing truthful
   * left to show.
   */
  it('a failed device read says so instead of claiming nothing is plugged in', async () => {
    withBrokenCable()
    await openUsb()

    expect(await screen.findByTestId('c-3e-list-error')).not.toBeNull()
    expect(screen.queryByTestId('c-3e-none')).toBeNull()
  })

  /**
   * The bug this pins, found by the human on real hardware: the pane rendered
   * `t(error.code)`, and `E_DEVICE` is a *code*, not a dictionary key, so the
   * fallback chain (§15) printed the bare string `E_DEVICE` — and the toast showed
   * only the generic sentence. Together they told the human nothing about a failure
   * that was happening right in front of them. The backend's `detail` (which
   * carries the HRESULT) has to be on screen.
   */
  it('a read failure shows the backend reason, never the bare code', async () => {
    withBrokenCable()
    await openUsb()

    const hint = await screen.findByTestId('c-3e-list-error')
    // The HRESULT survives all the way to the screen — it is the one part of the
    // failure that identifies which of a dozen driver problems this is.
    expect(hint.textContent).toContain('0x80010106')
    // The localized sentence is kept — it is the part that generalizes.
    expect(hint.textContent).toContain('dispositivo')
    // The code itself is never what a user is shown.
    expect(hint.textContent).not.toContain('E_DEVICE')
  })

  /**
   * The affirmative state needs a sentence of its own. A radio list appearing
   * silently is not a status: the human cannot tell "Windows published one
   * device" from "the pane finally re-rendered", and the whole point of the count
   * is that it is the only evidence the cable is alive.
   */
  it('an attached phone is announced with a count, not just a silent radio', async () => {
    withPhone()
    await openUsb()

    expect((await screen.findByTestId('c-3e-found')).textContent).toContain(
      '1 aparelho(s) identificado(s)',
    )
  })

  /**
   * Two phones and no auto-selection, deliberately: picking the wrong one is how
   * files land in the wrong `Media/<label>/`. The line has to say so, because the
   * radio group looks identical to the one-phone case otherwise.
   */
  it('several phones are counted and the pane asks which to use', async () => {
    withTwoPhones()
    await openUsb()

    expect((await screen.findByTestId('c-3e-found')).textContent).toContain(
      '2 aparelho(s) identificado(s)',
    )
    expect(screen.getByTestId('c-3e-found').textContent).toContain('Escolha qual usar')
    // Nothing selected, so Start cannot be pressed — and no "Destino:" line, which
    // would name a library folder for a device nobody chose.
    expect(screen.getByTestId('c-3j-start')).toHaveProperty('disabled', true)
    expect(screen.queryByText(/^Destino: /)).toBeNull()
  })

  /**
   * The human's own "look again": Backup is opened, the phone is plugged in after
   * the first read, and the 4 s poll has not fired yet. Pressing the control must
   * read the list again immediately and enable Start — the alternative is
   * concluding the cable is dead.
   */
  it('the search button re-reads the list and finds a phone that arrived late', async () => {
    let plugged = false
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'devices_list') {
        return plugged ? [PHONE] : []
      }
      return defaultMock(command, args)
    })
    await openUsb()
    expect(await screen.findByTestId('c-3e-none')).not.toBeNull()

    plugged = true
    await act(async () => {
      fireEvent.click(screen.getByTestId('c-3e-refresh'))
    })

    expect(await screen.findByText('iPhone do Ismar')).not.toBeNull()
    expect(screen.queryByTestId('c-3e-none')).toBeNull()
    expect(screen.getByTestId('c-3j-start')).toHaveProperty('disabled', false)
  })

  /**
   * A refresh that fails while a working list is on screen must not throw that list
   * away: the device is still selected and still importable, so the failure is
   * reported as staleness. Wiping it would be the worse lie (§10.6.4).
   */
  it('a failed refresh warns the list may be stale instead of dropping the phone', async () => {
    withPhone()
    await openUsb()
    expect(await screen.findByText('iPhone do Ismar')).not.toBeNull()

    withBrokenCable()
    await act(async () => {
      fireEvent.click(screen.getByTestId('c-3e-refresh'))
    })

    expect(await screen.findByTestId('c-3e-stale')).not.toBeNull()
    // The device survives the failure, and so does the way to start a pass.
    expect(screen.getByText('iPhone do Ismar')).not.toBeNull()
    expect(screen.getByTestId('c-3j-start')).toHaveProperty('disabled', false)
    expect(screen.queryByTestId('c-3e-list-error')).toBeNull()
  })

  /**
   * The other half of that contract, and the reason it is safe to keep a list
   * across failures: a read that *succeeds* and finds nothing does clear it. An
   * unplugged phone must disappear — otherwise "no device" could never be shown
   * again after the first successful enumeration.
   */
  it('a successful read that finds no device drops the phone from the list', async () => {
    withPhone()
    await openUsb()
    expect(await screen.findByText('iPhone do Ismar')).not.toBeNull()

    withoutPhones()
    await act(async () => {
      fireEvent.click(screen.getByTestId('c-3e-refresh'))
    })

    expect(await screen.findByTestId('c-3e-none')).not.toBeNull()
    expect(screen.queryByText('iPhone do Ismar')).toBeNull()
    expect(screen.queryByTestId('c-3e-found')).toBeNull()
    expect(screen.queryByTestId('c-3e-stale')).toBeNull()
    expect(screen.getByTestId('c-3j-start')).toHaveProperty('disabled', true)
  })

  /**
   * Once a list exists, a later failure is never allowed to turn into "no device
   * identified" — the phone may well still be plugged in and merely unreadable, and
   * claiming otherwise is the lie §8.2 forbids. The list stays, the staleness is
   * stated, and the retry stays pressable no matter how many times it is pressed.
   */
  it('repeated read failures never degrade into "no device identified"', async () => {
    withPhone()
    await openUsb()
    expect(await screen.findByText('iPhone do Ismar')).not.toBeNull()

    withBrokenCable()
    for (let press = 0; press < 3; press += 1) {
      await act(async () => {
        fireEvent.click(screen.getByTestId('c-3e-refresh'))
      })
    }

    expect(await screen.findByTestId('c-3e-stale')).not.toBeNull()
    expect(screen.queryByTestId('c-3e-none')).toBeNull()
    expect(screen.getByText('iPhone do Ismar')).not.toBeNull()
    // Still usable, and still retryable — the point of the control.
    expect(screen.getByTestId('c-3j-start')).toHaveProperty('disabled', false)
    expect(screen.getByTestId('c-3e-refresh')).toHaveProperty('disabled', false)
  })

  /**
   * Enumerating MTP while a pass streams gigabytes spends USB round-trips the
   * transfer needs, and the selection must not move under a live job.
   */
  it('the search control is disabled while a pass runs', async () => {
    withPhone()
    await openUsb()
    expect(screen.getByTestId('c-3e-refresh')).toHaveProperty('disabled', false)

    fireEvent.click(await screen.findByTestId('c-3j-start'))

    expect(await screen.findByTestId('c-3k-stop')).not.toBeNull()
    expect(screen.getByTestId('c-3e-refresh')).toHaveProperty('disabled', true)
  })
})

describe('Mídia gallery (c10) — grid, states, chips (§11.2)', () => {
  async function openMedia() {
    await renderShell()
    const media = [...document.querySelectorAll('.zone2__item')].find((item) =>
      item.textContent?.includes('Mídia'),
    ) as HTMLElement
    fireEvent.click(media)
  }

  it('renders tiles for the queried page and the live total', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'media_query') {
        return MEDIA_PAGE
      }
      return defaultMock(command, args)
    })

    await openMedia()

    expect(await screen.findAllByTestId('media-tile')).toHaveLength(2)
    // The video tile carries the duration badge; the image does not.
    expect(screen.getByText('▶')).not.toBeNull()
    expect(await screen.findByTestId('media-grid')).not.toBeNull()
  })

  it('shows the empty state when the library has nothing and no filter is set', async () => {
    await openMedia()

    expect(await screen.findByText('Esta pasta está vazia.')).not.toBeNull()
  })

  it('asks for the standard scope and the default sort on the first read', async () => {
    await openMedia()
    await screen.findByText('Esta pasta está vazia.')

    const queryCalls = invokeMock.mock.calls.filter(([command]) => command === 'media_query')
    expect(queryCalls.length).toBeGreaterThan(0)
    expect(queryCalls[0][1]).toEqual({
      spec: expect.objectContaining({ scope: 'standard', sort: 'capturedDesc', offset: 0 }),
    })
  })

  it('opens the lightbox on tile double click and closes it on Esc', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'media_query') {
        return MEDIA_PAGE
      }
      return defaultMock(command, args)
    })

    await openMedia()
    const tiles = await screen.findAllByTestId('media-tile')
    fireEvent.doubleClick(tiles[0])

    const dialog = await screen.findByRole('dialog', { name: 'Visualização de mídia' })
    expect(within(dialog).getByText('IMG_1.jpg')).not.toBeNull()

    fireEvent.keyDown(window, { key: 'Escape' })
    expect(screen.queryByRole('dialog', { name: 'Visualização de mídia' })).toBeNull()
  })

  it('re-queries the gallery when an ingest broadcasts db-changed (c13 fix)', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'media_query') {
        return MEDIA_PAGE
      }
      return defaultMock(command, args)
    })

    await openMedia()
    await screen.findAllByTestId('media-tile')
    const before = invokeMock.mock.calls.filter(([command]) => command === 'media_query').length

    // Feed every `wolfs://db-changed` subscriber, the way `ingestAll` does for
    // the ingest channel: the real runtime delivers to all of them, and the
    // gallery's own subscription is what re-queries the page.
    const { listen } = await import('@tauri-apps/api/event')
    const dbHandlers = vi
      .mocked(listen)
      .mock.calls.filter(([channel]) => channel === 'wolfs://db-changed')
      .map(([, handler]) => handler as (event: { payload: null }) => void)
    expect(dbHandlers.length).toBeGreaterThan(0)
    act(() => {
      for (const handler of dbHandlers) {
        handler({ payload: null })
      }
    })

    await waitFor(() => {
      const after = invokeMock.mock.calls.filter(([command]) => command === 'media_query').length
      expect(after).toBeGreaterThan(before)
    })
  })

  it('load more appends the next page without unmounting the grid (c13 fix)', async () => {
    const first = { items: [MEDIA_PAGE.items[0]], total: 2, hasMore: true }
    const second = { items: [MEDIA_PAGE.items[1]], total: 2, hasMore: false }
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'media_query') {
        const offset = (args as { spec: { offset: number } }).spec.offset
        return offset === 0 ? first : second
      }
      return defaultMock(command, args)
    })

    await openMedia()
    expect(await screen.findAllByTestId('media-tile')).toHaveLength(1)

    fireEvent.click(screen.getByRole('button', { name: 'Carregar mais' }))

    // The second tile *appends* — the first one is still mounted (a window
    // replace would have sent the scroll to the top and shown VID_2 only).
    await waitFor(() => {
      expect(screen.getAllByTestId('media-tile')).toHaveLength(2)
    })
    expect(screen.getByText('IMG_1.jpg')).not.toBeNull()
    // The page is exhausted, so the button leaves.
    expect(screen.queryByRole('button', { name: 'Carregar mais' })).toBeNull()
  })

  it('plays a video in the lightbox via the scoped Media/ file (c13 fix)', async () => {
    const play = vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue(undefined)
    vi.spyOn(HTMLMediaElement.prototype, 'pause').mockImplementation(() => {})

    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'media_query') {
        return { items: [MEDIA_PAGE.items[1]], total: 1, hasMore: false }
      }
      if (command === 'media_detail') {
        return MEDIA_PAGE.items[1]
      }
      return defaultMock(command, args)
    })

    await openMedia()
    fireEvent.doubleClick(await screen.findByTestId('media-tile'))

    const dialog = await screen.findByRole('dialog', { name: 'Visualização de mídia' })
    await waitFor(() => {
      expect(dialog.querySelector('video')).not.toBeNull()
    })
    expect(
      invokeMock.mock.calls.filter(([command]) => command === 'media_scope_video'),
    ).toHaveLength(1)
    // 2026-10-11: opening a video starts it playing — the poster is only a frame
    // until playback begins, never a result the human has to press play on.
    await waitFor(() => expect(play).toHaveBeenCalled())
  })

  it('shows Clear All only when two or more filters are active (C10)', async () => {
    await openMedia()
    await screen.findByText('Esta pasta está vazia.')

    // One filter: chips render, Clear All does not.
    fireEvent.change(screen.getByRole('searchbox'), { target: { value: 'IMG' } })
    expect(await screen.findByText(/Buscar: IMG/)).not.toBeNull()
    expect(screen.queryByText('Limpar tudo')).toBeNull()

    // Two filters: Clear All appears and clears both.
    fireEvent.change(screen.getByLabelText('Tipo'), { target: { value: 'image' } })
    expect(await screen.findByText('Limpar tudo')).not.toBeNull()
    fireEvent.click(screen.getByText('Limpar tudo'))
    expect(screen.queryByText(/Buscar: IMG/)).toBeNull()
  })

  it('filters by city from the L2 tag-cloud and clears it with the chip (c14 follow-up)', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'media_query') {
        return MEDIA_PAGE
      }
      if (command === 'city_counts') {
        return [
          { city: 'São Paulo', country: 'BR', count: 42 },
          { city: 'Lisboa', country: 'PT', count: 3 },
        ]
      }
      return defaultMock(command, args)
    })

    await openMedia()
    await screen.findAllByTestId('media-tile')

    // The drawer promised a Localização tag-cloud (§10.6.1); it is a real scoped
    // `city_counts` read, not a text box.
    fireEvent.click(screen.getByRole('button', { name: 'Filtros' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Localização: São Paulo' }))

    expect(await screen.findByText(/Localização: São Paulo/)).not.toBeNull()

    const calls = invokeMock.mock.calls.filter(([command]) => command === 'media_query')
    const last = calls[calls.length - 1][1] as { spec: { cities: string[] } }
    expect(last.spec.cities).toEqual(['São Paulo'])
    expect(
      invokeMock.mock.calls.some(
        ([command, args]) =>
          command === 'city_counts' && (args as { scope: string }).scope === 'standard',
      ),
    ).toBe(true)

    // The chip's ✕ removes just that city.
    fireEvent.click(screen.getByRole('button', { name: 'Remover: Localização: São Paulo' }))
    expect(screen.queryByText(/Localização: São Paulo/)).toBeNull()
  })

  /** Every gallery-shortcut test needs the two-item page. */
  function withPage() {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'media_query') {
        return MEDIA_PAGE
      }
      if (command === 'media_detail') {
        const id = (args as { id: string }).id
        return MEDIA_PAGE.items.find((item) => item.id === id) ?? null
      }
      return defaultMock(command, args)
    })
  }

  // §12.6: "Ctrl/Cmd+A → select all in the current spec", and the Windows
  // Explorer toggle the human asked for (2026-10-15): pressing it again clears.
  it('Ctrl/Cmd+A selects the whole page, and again clears it (c14 §12.6)', async () => {
    withPage()
    await openMedia()
    await screen.findAllByTestId('media-tile')

    act(() => {
      fireEvent.keyDown(window, { key: 'a', ctrlKey: true })
    })

    let tiles = screen.getAllByTestId('media-tile')
    expect(tiles.every((tile) => tile.getAttribute('aria-pressed') === 'true')).toBe(true)
    expect(await screen.findByText('2 selecionados')).not.toBeNull()

    act(() => {
      fireEvent.keyDown(window, { key: 'a', ctrlKey: true })
    })

    tiles = screen.getAllByTestId('media-tile')
    expect(tiles.every((tile) => tile.getAttribute('aria-pressed') === 'false')).toBe(true)
    expect(screen.queryByText('2 selecionados')).toBeNull()
  })

  // The I-beam guard is the hook's, tested through the binding that would
  // otherwise steal the caret's own Ctrl+A.
  it('leaves Ctrl+A to the search field while it has the caret (§12.6)', async () => {
    withPage()
    await openMedia()
    await screen.findAllByTestId('media-tile')

    fireEvent.keyDown(screen.getByRole('searchbox'), { key: 'a', ctrlKey: true })

    expect(document.querySelector('.zone5')).toBeNull()
  })

  // The Explorer click map (human 2026-10-15, §12.6): click selects, double
  // click opens, Ctrl+click toggles without opening, Shift+click ranges.
  it('selects on click, opens on double click, Ctrl+click and Shift+click (c14 §12.6)', async () => {
    withPage()
    await openMedia()
    const tiles = await screen.findAllByTestId('media-tile')

    fireEvent.click(tiles[0])
    expect(tiles[0].getAttribute('aria-pressed')).toBe('true')
    expect(tiles[1].getAttribute('aria-pressed')).toBe('false')
    // A single click must NOT open the lightbox any more.
    expect(screen.queryByRole('dialog', { name: 'Visualização de mídia' })).toBeNull()

    fireEvent.doubleClick(tiles[0])
    expect(await screen.findByRole('dialog', { name: 'Visualização de mídia' })).not.toBeNull()
    fireEvent.keyDown(window, { key: 'Escape' })
    await waitFor(() =>
      expect(screen.queryByRole('dialog', { name: 'Visualização de mídia' })).toBeNull(),
    )

    // Ctrl+click adds without opening…
    fireEvent.click(tiles[1], { ctrlKey: true })
    expect(tiles[1].getAttribute('aria-pressed')).toBe('true')
    expect(screen.queryByRole('dialog', { name: 'Visualização de mídia' })).toBeNull()
    // …and Ctrl+click again removes.
    fireEvent.click(tiles[1], { ctrlKey: true })
    expect(tiles[1].getAttribute('aria-pressed')).toBe('false')

    // Shift+click ranges from the anchor. Ctrl+click above moved the anchor to
    // g2, so re-click g1 first (a plain click is select-and-anchor).
    fireEvent.click(tiles[0])
    fireEvent.click(tiles[1], { shiftKey: true })
    expect(tiles[0].getAttribute('aria-pressed')).toBe('true')
    expect(tiles[1].getAttribute('aria-pressed')).toBe('true')
    expect(await screen.findByText('2 selecionados')).not.toBeNull()
  })

  // Explorer's keyboard cursor: arrows replace the selection, Shift+arrows extend.
  it('walks the selection with arrow keys while the lightbox is closed (c14 §12.6)', async () => {
    withPage()
    await openMedia()
    const tiles = await screen.findAllByTestId('media-tile')

    // Anchor g1, then step right: selection follows the cursor.
    fireEvent.click(tiles[0])
    fireEvent.keyDown(tiles[0], { key: 'ArrowRight' })
    expect(tiles[1].getAttribute('aria-pressed')).toBe('true')
    expect(tiles[0].getAttribute('aria-pressed')).toBe('false')
    expect(screen.queryByRole('dialog', { name: 'Visualização de mídia' })).toBeNull()

    // Anchor is g2 now; Shift+ArrowLeft extends back over g1.
    fireEvent.keyDown(tiles[1], { key: 'ArrowLeft', shiftKey: true })
    expect(tiles[0].getAttribute('aria-pressed')).toBe('true')
    expect(tiles[1].getAttribute('aria-pressed')).toBe('true')
  })

  // §12.6: "Space → quick-look" was replaced (human 2026-10-15) by the Explorer
  // toggle of the focused tile; Enter is the open key.
  it('Space toggles the focused tile and Enter opens it (c14 §12.6)', async () => {
    withPage()
    await openMedia()
    await screen.findAllByTestId('media-tile')

    const tile = screen.getAllByTestId('media-tile')[0]
    tile.focus()
    fireEvent.keyDown(tile, { key: ' ' })

    expect(tile.getAttribute('aria-pressed')).toBe('true')
    expect(screen.queryByRole('dialog', { name: 'Visualização de mídia' })).toBeNull()

    tile.focus()
    fireEvent.keyDown(tile, { key: 'Enter' })
    expect(await screen.findByRole('dialog', { name: 'Visualização de mídia' })).not.toBeNull()

    fireEvent.keyDown(window, { key: 'Escape' })
    await waitFor(() =>
      expect(screen.queryByRole('dialog', { name: 'Visualização de mídia' })).toBeNull(),
    )
  })

  // §11.4's keyboard: `←/→` navigate. Both keys and the visible arrow row have to
  // move together, and the row has to leave at the ends rather than dangle.
  it('navigates the lightbox with ←/→ and with its arrows (c14 §11.4)', async () => {
    withPage()
    await openMedia()
    const tiles = await screen.findAllByTestId('media-tile')
    fireEvent.doubleClick(tiles[0])

    let dialog = await screen.findByRole('dialog', { name: 'Visualização de mídia' })
    expect(within(dialog).getByText('IMG_1.jpg')).not.toBeNull()
    // First of two: nowhere back to go, so the control is absent.
    expect(within(dialog).queryByRole('button', { name: 'Anterior' })).toBeNull()

    act(() => {
      fireEvent.keyDown(window, { key: 'ArrowRight' })
    })
    dialog = await screen.findByRole('dialog', { name: 'Visualização de mídia' })
    expect(within(dialog).getByText('VID_2.mov')).not.toBeNull()
    // Last of two: forward is the one that leaves.
    expect(within(dialog).queryByRole('button', { name: 'Próxima' })).toBeNull()
    // With the overlay up, `←/→` belong to it: the grid selection is untouched.
    expect(tiles[1].getAttribute('aria-pressed')).toBe('false')

    act(() => {
      fireEvent.keyDown(window, { key: 'ArrowLeft' })
    })
    dialog = await screen.findByRole('dialog', { name: 'Visualização de mídia' })
    expect(within(dialog).getByText('IMG_1.jpg')).not.toBeNull()

    // And the visible arrows do the same as the keys.
    fireEvent.click(within(dialog).getByRole('button', { name: 'Próxima' }))
    dialog = await screen.findByRole('dialog', { name: 'Visualização de mídia' })
    expect(within(dialog).getByText('VID_2.mov')).not.toBeNull()
  })

  // Esc must close this screen's own overlays; the filter drawer has no other way
  // out besides its backdrop and its Fechar button.
  it('Esc closes the L2 filter drawer (c14 §10.6.5)', async () => {
    withPage()
    await openMedia()
    await screen.findAllByTestId('media-tile')

    fireEvent.click(screen.getByRole('button', { name: 'Filtros' }))
    expect(await screen.findByRole('dialog', { name: 'Filtros' })).not.toBeNull()

    act(() => {
      fireEvent.keyDown(window, { key: 'Escape' })
    })
    expect(screen.queryByRole('dialog', { name: 'Filtros' })).toBeNull()
  })
})

describe('Listas (c16) — index, snapshot builder, pick-media (§11.5/§12.4)', () => {
  const SMART_SPEC = {
    scope: 'standard' as const,
    device: null,
    cities: ['Paris'],
    month: null,
    noMetadata: false,
    search: null,
    fileType: 'image' as const,
    sort: 'capturedDesc' as const,
    limit: 240,
    offset: 0,
  }
  const LIST_MANUAL = {
    id: 'l1',
    name: 'Viagens',
    filterCriteria: null,
    createdAt: '2024-01-01T00:00:00',
    memberCount: 1,
  }
  const LIST_SMART = {
    id: 'l2',
    name: '2024',
    filterCriteria: SMART_SPEC,
    createdAt: '2024-01-01T00:00:00',
    memberCount: 0,
  }
  const DETAIL_MANUAL = { ...LIST_MANUAL, members: [MEDIA_PAGE.items[0]] }
  const DETAIL_SMART = { ...LIST_SMART, members: [] }

  /** The Lists screen mounts `lists_list`; each row's detail mounts on select. */
  function withLists() {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'lists_list') {
        return [LIST_MANUAL, LIST_SMART]
      }
      if (command === 'lists_detail') {
        return (args as { id: string }).id === 'l2' ? DETAIL_SMART : DETAIL_MANUAL
      }
      return defaultMock(command, args)
    })
  }

  async function openLists() {
    await renderShell()
    const lists = [...document.querySelectorAll('.zone2__item')].find((item) =>
      item.textContent?.includes('Listas'),
    ) as HTMLElement
    fireEvent.click(lists)
  }

  async function openMedia() {
    await renderShell()
    const media = [...document.querySelectorAll('.zone2__item')].find((item) =>
      item.textContent?.includes('Mídia'),
    ) as HTMLElement
    fireEvent.click(media)
  }

  it('renders the index with a manual/smart badge and the selected list members', async () => {
    withLists()
    await openLists()

    const items = await screen.findAllByTestId('lists-index-item')
    expect(items.map((row) => row.textContent)).toEqual([
      expect.stringContaining('Viagens'),
      expect.stringContaining('2024'),
    ])
    // The first list is auto-selected; a manual list shows its members.
    expect((await screen.findByTestId('lists-detail-name')).textContent).toBe('Viagens')
    expect(screen.getByTestId('lists-members')).not.toBeNull()
    expect(screen.getAllByTestId('lists-member')).toHaveLength(1)
  })

  it('a smart list shows its stored criteria as chips and the smart bar', async () => {
    withLists()
    await openLists()
    fireEvent.click((await screen.findAllByTestId('lists-index-item'))[1])

    const criteria = await screen.findByTestId('lists-criteria')
    // Rendered by the same helper as the gallery chips (§15 parity).
    expect(within(criteria).getByText('Localização: Paris')).not.toBeNull()
    expect(within(criteria).getByText('Tipo: image')).not.toBeNull()
    expect(screen.getByTestId('lists-smartbar')).not.toBeNull()
    // A smart list never offers the manual picker.
    expect(screen.queryByTestId('lists-add-media')).toBeNull()
  })

  it('creates a list and selects the created row', async () => {
    withLists()
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'lists_create') {
        const name = (args as { payload: { name: string } }).payload.name
        return { ...LIST_MANUAL, id: 'l9', name, memberCount: 0 }
      }
      if (command === 'lists_detail') {
        return { ...DETAIL_MANUAL, id: 'l9', name: 'Nova' }
      }
      if (command === 'lists_list') {
        return [LIST_MANUAL, LIST_SMART]
      }
      return defaultMock(command, args)
    })
    await openLists()

    fireEvent.click(await screen.findByTestId('lists-new'))
    fireEvent.change(screen.getByLabelText('Nova lista'), { target: { value: 'Nova' } })
    fireEvent.click(screen.getByRole('button', { name: 'Criar' }))

    await waitFor(() => {
      const call = invokeMock.mock.calls.find(([command]) => command === 'lists_create')
      expect(call?.[1]).toEqual({ payload: { name: 'Nova' } })
    })
  })

  it('renames the active list through lists_update', async () => {
    withLists()
    await openLists()
    await screen.findByTestId('lists-detail-name')

    fireEvent.click(screen.getByTestId('lists-rename'))
    fireEvent.change(screen.getByLabelText('Renomear'), { target: { value: 'Férias' } })
    fireEvent.click(screen.getByTestId('lists-rename-save'))

    await waitFor(() => {
      const call = invokeMock.mock.calls.find(([command]) => command === 'lists_update')
      expect(call?.[1]).toEqual({ payload: { id: 'l1', name: 'Férias' } })
    })
  })

  it('[Aplicar à lista] snapshots the gallery’s active spec into an existing list (§12.4)', async () => {
    withLists()
    await openMedia()
    await screen.findByRole('button', { name: 'Aplicar à lista' })

    fireEvent.click(screen.getByRole('button', { name: 'Aplicar à lista' }))
    // The picker reads the index, then applying replaces the list's membership.
    fireEvent.click(await screen.findByRole('button', { name: /Viagens/ }))

    await waitFor(() => {
      const call = invokeMock.mock.calls.find(([command]) => command === 'lists_snapshot_apply')
      expect(call?.[1]).toEqual({
        payload: { listId: 'l1', spec: expect.objectContaining({ scope: 'standard' }) },
      })
    })
  })

  it('[Criar e aplicar] makes the list and snapshots into it in one step', async () => {
    withLists()
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'lists_create') {
        return { ...LIST_MANUAL, id: 'l9', name: 'Nova' }
      }
      if (command === 'lists_list') {
        return [LIST_MANUAL]
      }
      return defaultMock(command, args)
    })
    await openMedia()
    await screen.findByRole('button', { name: 'Aplicar à lista' })

    fireEvent.click(screen.getByRole('button', { name: 'Aplicar à lista' }))
    fireEvent.change(await screen.findByLabelText('Nova lista'), { target: { value: 'Nova' } })
    fireEvent.click(screen.getByRole('button', { name: 'Criar e aplicar' }))

    await waitFor(() => {
      const call = invokeMock.mock.calls.find(([command]) => command === 'lists_snapshot_apply')
      expect(call?.[1]).toEqual({
        payload: { listId: 'l9', spec: expect.objectContaining({ scope: 'standard' }) },
      })
    })
  })

  it('picks media into a manual list through lists_add_items (§11.5 [C-5g])', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'media_query') {
        return MEDIA_PAGE
      }
      if (command === 'lists_list') {
        return [LIST_MANUAL]
      }
      if (command === 'lists_detail') {
        return DETAIL_MANUAL
      }
      return defaultMock(command, args)
    })
    await openLists()

    fireEvent.click(await screen.findByTestId('lists-add-media'))
    const tiles = await screen.findAllByTestId('lists-pick-media-grid')
    expect(tiles).toHaveLength(1)

    const mediaTiles = screen.getAllByTestId('media-tile')
    fireEvent.click(mediaTiles[0])
    fireEvent.click(screen.getByRole('button', { name: 'Adicionar selecionados' }))

    await waitFor(() => {
      const call = invokeMock.mock.calls.find(([command]) => command === 'lists_add_items')
      expect(call?.[1]).toEqual({ payload: { listId: 'l1', mediaIds: ['g1'] } })
    })
  })

  it('[Sincronizar] re-runs a smart list’s stored criteria (§11.5 [C-5f])', async () => {
    withLists()
    await openLists()
    fireEvent.click((await screen.findAllByTestId('lists-index-item'))[1])

    fireEvent.click(await screen.findByTestId('lists-sync'))

    await waitFor(() => {
      const call = invokeMock.mock.calls.find(([command]) => command === 'lists_snapshot_sync')
      expect(call?.[1]).toEqual({ id: 'l2' })
    })
  })
})

describe('Cofre (c19) — gate, mode and the hide actions (§11.6/§11.8/§11.9, C6)', () => {
  const UNCONFIGURED = { configured: false, unlocked: false, hasRecoveryHint: false }
  const LOCKED = { configured: true, unlocked: false, hasRecoveryHint: true }
  const UNLOCKED = { configured: true, unlocked: true, hasRecoveryHint: true }

  /** A hidden row: its thumbnail lives under `.vault/thumbs/` (both slots equal). */
  const VAULT_ITEM = {
    id: 'v1',
    relativePath: 'Media/PC/2024/03/IMG_h.jpg',
    thumb200: '.vault/thumbs/h1.webp',
    thumb400: '.vault/thumbs/h1.webp',
    fileHash: 'h1',
    fileSize: 1000,
    fileType: 'image',
    capturedAt: '2024-03-15T10:00:00',
    hasMetadata: true,
    deviceName: 'PC',
    isHidden: true,
    location: null,
  }

  function withVault(status: typeof UNCONFIGURED) {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'vault_status') return status
      return defaultMock(command, args)
    })
  }

  function navItem(container: HTMLElement, label: string): HTMLElement {
    return [...container.querySelectorAll('.zone2__item')].find((item) =>
      item.textContent?.includes(label),
    ) as HTMLElement
  }

  async function gotoVault() {
    const { container } = await renderShell()
    fireEvent.click(navItem(container, 'Cofre'))
    return { container }
  }

  async function gotoMedia() {
    const { container } = await renderShell()
    fireEvent.click(navItem(container, 'Mídia'))
    return { container }
  }

  /** Broadcast `wolfs://vault` — the §9.2 signal every vault command emits. */
  async function emitVault() {
    const { listen } = await import('@tauri-apps/api/event')
    const handlers = vi
      .mocked(listen)
      .mock.calls.filter(([channel]) => channel === 'wolfs://vault')
      .map(([, handler]) => handler as (event: { payload: unknown }) => void)
    for (const handler of handlers) handler({ payload: null })
  }

  it('paints the Cofre dot grey, then red, then green — live state, not a guess (§11.6)', async () => {
    withVault(UNCONFIGURED)
    const { container } = await renderShell()
    expect(container.querySelector('.zone2__lock-dot')?.className).toContain(
      'zone2__lock-dot--unconfigured',
    )

    withVault(LOCKED)
    await act(async () => {
      await emitVault()
    })
    expect(container.querySelector('.zone2__lock-dot')?.className).toContain(
      'zone2__lock-dot--locked',
    )

    withVault(UNLOCKED)
    await act(async () => {
      await emitVault()
    })
    expect(container.querySelector('.zone2__lock-dot')?.className).toContain(
      'zone2__lock-dot--unlocked',
    )
  })

  it('the first-run gate sets a password and shows the one-time recovery key (§11.6 [C-6d/e])', async () => {
    const KEY = 'ABCD2345EFGH6789JKMN2345'
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'vault_status') return UNCONFIGURED
      if (command === 'vault_setup') return KEY
      return defaultMock(command, args)
    })

    await gotoVault()
    expect(await screen.findByTestId('vault-setup')).not.toBeNull()

    fireEvent.change(screen.getByLabelText('Senha'), { target: { value: 'senha-forte-1' } })
    fireEvent.change(screen.getByLabelText('Repita a senha'), {
      target: { value: 'senha-forte-1' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Continuar' }))

    await waitFor(() => {
      const call = invokeMock.mock.calls.find(([command]) => command === 'vault_setup')
      expect(call?.[1]).toEqual({ password: 'senha-forte-1' })
    })
    const card = await screen.findByTestId('vault-recovery-key')
    expect(card.textContent).toContain(KEY)
  })

  it('a too-short password is refused inline and never reaches the backend', async () => {
    withVault(UNCONFIGURED)
    await gotoVault()
    await screen.findByTestId('vault-setup')

    fireEvent.change(screen.getByLabelText('Senha'), { target: { value: 'abc' } })
    fireEvent.change(screen.getByLabelText('Repita a senha'), { target: { value: 'abc' } })
    fireEvent.click(screen.getByRole('button', { name: 'Continuar' }))

    expect(screen.getByRole('alert').textContent).toBe('A senha precisa ter ao menos 8 caracteres.')
    expect(invokeMock.mock.calls.find(([command]) => command === 'vault_setup')).toBeUndefined()
  })

  it('the locked gate keeps a wrong password inline — E_AUTH, never a toast (§11.6)', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'vault_status') return LOCKED
      if (command === 'vault_unlock') throw { code: 'E_AUTH', message: 'senha incorreta' }
      return defaultMock(command, args)
    })

    await gotoVault()
    await screen.findByTestId('vault-locked')

    fireEvent.change(screen.getByLabelText('Senha'), { target: { value: 'errada' } })
    fireEvent.click(screen.getByRole('button', { name: 'Desbloquear' }))

    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toBe('Acesso negado. Verifique a autorização do cofre.')
    // §8.2: a wrong password is the one error that must NOT become a toast.
    expect(document.querySelector('.md-toast')).toBeNull()
  })

  it('a correct password unlocks and the mode grid reads scope: Vault (§11.6 [C-6b/g], C6)', async () => {
    let status = LOCKED
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'vault_status') return status
      if (command === 'vault_unlock') {
        status = UNLOCKED
        return null
      }
      if (command === 'media_query') {
        return { items: [VAULT_ITEM], total: 1, hasMore: false }
      }
      return defaultMock(command, args)
    })

    await gotoVault()
    await screen.findByTestId('vault-locked')
    fireEvent.change(screen.getByLabelText('Senha'), { target: { value: 'certa' } })
    fireEvent.click(screen.getByRole('button', { name: 'Desbloquear' }))

    await screen.findByTestId('vault-mode')
    const query = invokeMock.mock.calls.find(([command]) => command === 'media_query')
    const spec = (query?.[1] as { spec: { scope: string } } | undefined)?.spec
    expect(spec?.scope).toBe('vault')
    expect(await screen.findAllByTestId('media-tile')).toHaveLength(1)
  })

  it('locking again goes through vault_lock (§11.6 [C-6j])', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'vault_status') return UNLOCKED
      if (command === 'vault_lock') return null
      return defaultMock(command, args)
    })

    await gotoVault()
    await screen.findByTestId('vault-mode')
    fireEvent.click(screen.getByTestId('c-6j-lock'))

    await waitFor(() => {
      expect(invokeMock.mock.calls.find(([command]) => command === 'vault_lock')).toBeDefined()
    })
  })

  it('[C-6h Restaurar] sends the selected hidden ids to vault_unhide (§11.6)', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'vault_status') return UNLOCKED
      if (command === 'media_query') {
        return { items: [VAULT_ITEM], total: 1, hasMore: false }
      }
      if (command === 'vault_unhide') return 1
      return defaultMock(command, args)
    })

    await gotoVault()
    await screen.findByTestId('vault-mode')
    fireEvent.click((await screen.findAllByTestId('media-tile'))[0])
    fireEvent.click(screen.getByTestId('c-6h-restore'))

    await waitFor(() => {
      const call = invokeMock.mock.calls.find(([command]) => command === 'vault_unhide')
      expect(call?.[1]).toEqual({ ids: ['v1'] })
    })
  })

  it('[C-6i Apagar def.] asks first, then removes the rows — never before (§11.6, C7)', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'vault_status') return UNLOCKED
      if (command === 'media_query') {
        return { items: [VAULT_ITEM], total: 1, hasMore: false }
      }
      if (command === 'media_remove') return 1
      return defaultMock(command, args)
    })

    await gotoVault()
    await screen.findByTestId('vault-mode')
    fireEvent.click((await screen.findAllByTestId('media-tile'))[0])
    fireEvent.click(screen.getByTestId('c-6i-delete'))

    const dialog = await screen.findByRole('dialog')
    expect(invokeMock.mock.calls.find(([command]) => command === 'media_remove')).toBeUndefined()

    fireEvent.click(within(dialog).getByRole('button', { name: 'Apagar def.' }))
    await waitFor(() => {
      const call = invokeMock.mock.calls.find(([command]) => command === 'media_remove')
      expect(call?.[1]).toEqual({ ids: ['v1'] })
    })
  })

  it('[C-4j] hides the open media from the Mídia lightbox (§11.6 hide actions)', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'vault_status') return UNLOCKED
      if (command === 'media_query') return MEDIA_PAGE
      if (command === 'vault_hide') return 1
      return defaultMock(command, args)
    })

    await gotoMedia()
    const tiles = await screen.findAllByTestId('media-tile')
    fireEvent.doubleClick(tiles[0])
    fireEvent.click(await screen.findByTestId('c-4j-hide'))

    await waitFor(() => {
      const call = invokeMock.mock.calls.find(([command]) => command === 'vault_hide')
      expect(call?.[1]).toEqual({ ids: ['g1'] })
    })
  })

  it('[C-4j] also offers hiding the whole parent folder, by root-relative path (§11.6)', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'vault_status') return UNLOCKED
      if (command === 'media_query') return MEDIA_PAGE
      if (command === 'vault_hide_folder') return 3
      return defaultMock(command, args)
    })

    await gotoMedia()
    const tiles = await screen.findAllByTestId('media-tile')
    fireEvent.doubleClick(tiles[0])
    fireEvent.click(await screen.findByTestId('c-4j-hide-folder'))

    await waitFor(() => {
      const call = invokeMock.mock.calls.find(([command]) => command === 'vault_hide_folder')
      expect(call?.[1]).toEqual({ folder: 'Media/PC/2024/03' })
    })
  })

  it('[Z5d] hides the whole selection from the bulk toolbar when configured (§11.9)', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'vault_status') return UNLOCKED
      if (command === 'media_query') return MEDIA_PAGE
      if (command === 'vault_hide') return 1
      return defaultMock(command, args)
    })

    await gotoMedia()
    const tiles = await screen.findAllByTestId('media-tile')
    fireEvent.click(tiles[0])
    fireEvent.click(await screen.findByTestId('z5d-hide'))

    await waitFor(() => {
      const call = invokeMock.mock.calls.find(([command]) => command === 'vault_hide')
      expect(call?.[1]).toEqual({ ids: ['g1'] })
    })
  })

  it('renders no hide control at all when the vault is not configured (§11.9)', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'vault_status') return UNCONFIGURED
      if (command === 'media_query') return MEDIA_PAGE
      return defaultMock(command, args)
    })

    await gotoMedia()
    const tiles = await screen.findAllByTestId('media-tile')
    fireEvent.doubleClick(tiles[0])
    await screen.findByRole('dialog', { name: 'Visualização de mídia' })
    expect(screen.queryByTestId('c-4j-hide')).toBeNull()
  })

  it('hiding while locked still hides — no password, no gate (§7.10 c19 decision 12)', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'vault_status') return LOCKED
      if (command === 'media_query') return MEDIA_PAGE
      if (command === 'vault_hide') return 1
      return defaultMock(command, args)
    })

    await gotoMedia()
    const tiles = await screen.findAllByTestId('media-tile')
    fireEvent.doubleClick(tiles[0])
    fireEvent.click(await screen.findByTestId('c-4j-hide'))

    // Putting a file away is a write, not a look: it goes through while locked.
    await waitFor(() => {
      const call = invokeMock.mock.calls.find(([command]) => command === 'vault_hide')
      expect(call?.[1]).toEqual({ ids: ['g1'] })
    })
    // And the human is *not* dragged to the gate for it.
    expect(screen.queryByTestId('vault-locked')).toBeNull()
  })

  it('the Cofre route never shows the Mídia bulk toolbar (Zone 5 belongs to the gallery)', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'vault_status') return UNLOCKED
      if (command === 'media_query') return MEDIA_PAGE
      return defaultMock(command, args)
    })

    const { container } = await gotoMedia()
    fireEvent.click((await screen.findAllByTestId('media-tile'))[0])
    expect(container.querySelector('.zone5')).not.toBeNull()

    fireEvent.click(navItem(container, 'Cofre'))
    await screen.findByTestId('vault-mode')
    expect(container.querySelector('.zone5')).toBeNull()
  })

  it('re-locks the vault when leaving the Cofre route — auto-lock (§12.5.7)', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'vault_status') return UNLOCKED
      if (command === 'media_query') return MEDIA_PAGE
      if (command === 'vault_lock') return null
      return defaultMock(command, args)
    })

    const { container } = await gotoVault()
    await screen.findByTestId('vault-mode')
    // Still on the guarded surface: nothing has locked yet.
    expect(invokeMock.mock.calls.find(([c]) => c === 'vault_lock')).toBeUndefined()

    fireEvent.click(navItem(container, 'Mídia'))
    await waitFor(() => {
      expect(invokeMock.mock.calls.find(([c]) => c === 'vault_lock')).toBeDefined()
    })
  })

  it('does not lock when the leave is not from the Cofre (no stray vault_lock)', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'vault_status') return UNLOCKED
      if (command === 'media_query') return MEDIA_PAGE
      if (command === 'vault_lock') return null
      return defaultMock(command, args)
    })

    const { container } = await gotoMedia()
    fireEvent.click(navItem(container, 'Ajustes'))
    expect(invokeMock.mock.calls.find(([c]) => c === 'vault_lock')).toBeUndefined()
  })

  it('the tile context menu hides the clicked media when configured (§10.6.2)', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'vault_status') return UNLOCKED
      if (command === 'media_query') return MEDIA_PAGE
      if (command === 'vault_hide') return 1
      return defaultMock(command, args)
    })

    await gotoMedia()
    fireEvent.contextMenu((await screen.findAllByTestId('media-tile'))[0])
    const menu = await screen.findByTestId('media-context-menu')
    fireEvent.click(within(menu).getByTestId('ctx-hide'))

    await waitFor(() => {
      const call = invokeMock.mock.calls.find(([command]) => command === 'vault_hide')
      expect(call?.[1]).toEqual({ ids: ['g1'] })
    })
  })

  it('the context menu opens the lightbox through "Ver metadados" (§10.6.2)', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'vault_status') return UNLOCKED
      if (command === 'media_query') return MEDIA_PAGE
      return defaultMock(command, args)
    })

    await gotoMedia()
    fireEvent.contextMenu((await screen.findAllByTestId('media-tile'))[0])
    const menu = await screen.findByTestId('media-context-menu')
    fireEvent.click(within(menu).getByRole('menuitem', { name: 'Ver metadados' }))

    expect(await screen.findByRole('dialog', { name: 'Visualização de mídia' })).not.toBeNull()
  })

  it('the context menu omits "Ocultar no cofre" when the vault is not configured (§10.6.2)', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'vault_status') return UNCONFIGURED
      if (command === 'media_query') return MEDIA_PAGE
      return defaultMock(command, args)
    })

    await gotoMedia()
    fireEvent.contextMenu((await screen.findAllByTestId('media-tile'))[0])
    const menu = await screen.findByTestId('media-context-menu')
    expect(within(menu).queryByTestId('ctx-hide')).toBeNull()
  })

  it('the Cofre grid never opens the media context menu (mockup: no ctx in vault view)', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'vault_status') return UNLOCKED
      if (command === 'media_query') {
        return { items: [VAULT_ITEM], total: 1, hasMore: false }
      }
      return defaultMock(command, args)
    })

    await gotoVault()
    await screen.findByTestId('vault-mode')
    fireEvent.contextMenu((await screen.findAllByTestId('media-tile'))[0])
    expect(screen.queryByTestId('media-context-menu')).toBeNull()
  })

  it('the empty Cofre reads as "nada oculto", never "importe mídia" (§7.10 c19 decision 13)', async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === 'vault_status') return UNLOCKED
      if (command === 'media_query') return { items: [], total: 0, hasMore: false }
      return defaultMock(command, args)
    })

    await gotoVault()
    await screen.findByTestId('vault-mode')
    expect(await screen.findByText('Nenhuma mídia oculta.')).not.toBeNull()
    expect(screen.queryByText('Importe mídia para começar.')).toBeNull()
  })
})
