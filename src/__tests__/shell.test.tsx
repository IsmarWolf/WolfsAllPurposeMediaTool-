import { render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { AppShell } from '../app/AppShell'
import { Providers } from '../app/providers'

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
  firstRun: true,
  ffmpegOk: true,
}

const invoke = vi.fn(async (command: string) => {
  switch (command) {
    case 'stats_get':
      return STATS
    case 'resolve_app_root':
      return ROOT
    case 'vault_status':
      return { configured: false, unlocked: false, hasRecoveryHint: false }
    case 'app_versions':
      return { appVersion: '0.1.0', os: 'windows', arch: 'x86_64' }
    default:
      throw new Error(`comando não mockado: ${command}`)
  }
})

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [string])),
}))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }))

/** Renders and waits for the boot reads to settle, so no assertion races React. */
async function renderShell() {
  const result = render(
    <Providers>
      <AppShell />
    </Providers>,
  )
  await screen.findByTestId('c-1a-stats')
  return result
}

describe('shell contract (§10.6.3 / §10.6.6, C16)', () => {
  beforeEach(() => {
    invoke.mockClear()
  })

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

  it('Zone 1 carries the shell controls and no filters (C16)', async () => {
    const { container } = await renderShell()
    const header = container.querySelector('.zone1') as HTMLElement
    expect(header.querySelector('[data-testid="z1d-db-health"]')).not.toBeNull()
    expect(header.textContent).toContain('Importar')
    expect(header.textContent).toContain('Ajustes')
    // The filter surface belongs to Mídia, never to the header.
    expect(header.textContent).not.toContain('Sem metadados')
  })

  it('shows the live counts from the database on the Dashboard', async () => {
    await renderShell()
    expect(screen.getByText('1.234')).toBeTruthy()
    expect(screen.getByTestId('c-1a-stats').textContent).toContain('900')
  })

  it('keeps Cofre detached in the security group', async () => {
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

describe('no refetch loop (§10.3 useAsync)', () => {
  beforeEach(() => {
    invoke.mockClear()
  })

  // A `useCallback([fn])` on an inline loader re-ran the effect on every render,
  // which showed up as numbers blinking between "loading" and 0.
  it('reads the stats exactly once per mount', async () => {
    await renderShell()
    const statsCalls = invoke.mock.calls.filter(([command]) => command === 'stats_get')
    expect(statsCalls).toHaveLength(1)
  })

  it('never shows a 0 before the first read lands', async () => {
    // Only `stats_get` stalls: the child effects of Zone 3 fire before the
    // shell's own, so `mockImplementationOnce` would have hijacked another
    // command.
    invoke.mockImplementation(async (command: string) => {
      if (command === 'stats_get') {
        return new Promise(() => {})
      }
      switch (command) {
        case 'resolve_app_root':
          return ROOT
        case 'vault_status':
          return { configured: false, unlocked: false, hasRecoveryHint: false }
        case 'app_versions':
          return { appVersion: '0.1.0', os: 'windows', arch: 'x86_64' }
        default:
          throw new Error(`comando não mockado: ${command}`)
      }
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
