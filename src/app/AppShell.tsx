import { useCallback, useMemo, useState } from 'react'
import { BulkToolbar } from '../components/layout/BulkToolbar'
import { Header } from '../components/layout/Header'
import { Inspector } from '../components/layout/Inspector'
import { Sidebar } from '../components/layout/Sidebar'
import { ToastSock } from '../components/layout/ToastSock'
import { TransferBar } from '../components/layout/TransferBar'
import { useDashStats } from '../hooks/useTauriEvent'
import { useInspector, useSelection } from '../hooks/useSelection'
import { useShortcuts, type Shortcut } from '../hooks/useShortcuts'
import { useI18n, useToast } from '../hooks/useContexts'
import { useVaultActions, useVaultStatus } from '../hooks/useVault'
import { resolveAppRoot } from '../lib/ipc'
import { BackupScreen } from '../features/backup'
import { DashboardScreen } from '../features/dashboard'
import { ListsScreen } from '../features/lists'
import { MediaScreen } from '../features/media'
import { SettingsScreen } from '../features/settings'
import { VaultScreen } from '../features/vault'
import { DEFAULT_ROUTE, hasSelectionModel, type Route } from '../routes'
import { TransferProvider } from './TransferProvider'

/**
 * §10.6.3 the 4 zones + the floating Zone 5 bar.
 *
 * Zone-role purity (C16) is enforced here, not by convention: Zone 2 receives
 * only the Mídia count and the vault lock dot, Zone 1 receives only the DB dot,
 * and the canvas never learns about the sidebar.
 *
 * `TransferProvider` wraps the shell rather than living inside a route because
 * §9.2's progress channels have no job id: exactly one subscriber has to exist
 * for the session, or a pass that is still copying reports to nobody the moment
 * the human leaves the Backup page. `TransferBar` is therefore a sibling of the
 * body, below it — a bottom strip, not a fifth zone: it carries no navigation
 * and no selection.
 */
export function AppShell() {
  return (
    <TransferProvider>
      <ShellBody />
    </TransferProvider>
  )
}

function ShellBody() {
  const { t } = useI18n()
  const { push } = useToast()
  const [route, setRoute] = useState<Route>(DEFAULT_ROUTE)
  const selection = useSelection()
  const { data: stats, loading: statsLoading, error: statsError, rerun } = useDashStats()
  const { status: vault } = useVaultStatus()
  const { hide, hideFolder, lock } = useVaultActions()

  // §11.6: entering the guarded space drops the shell selection, so the Mídia
  // bulk bar / inspector selection can never appear over the Cofre.
  // §12.5.7: leaving it re-locks the session. The route is the vault's only
  // "exit", so this is where the auto-lock belongs — the `wolfsvault` scheme
  // stops serving, in-flight/next opens re-gate, and the dot drops to red
  // without a manual `[C-6j]`. `vault_lock` is idempotent (§8.1), so a leave
  // while already locked is a harmless no-op.
  const navigate = useCallback(
    (next: Route) => {
      if (next === 'vault') {
        selection.clear()
      } else if (route === 'vault' && vault?.unlocked) {
        void lock()
      }
      setRoute(next)
    },
    [selection, route, vault, lock],
  )

  // §11.6: grey (unconfigured) until the status read lands, then the live state.
  const vaultState: 'unconfigured' | 'locked' | 'unlocked' = !vault?.configured
    ? 'unconfigured'
    : vault.unlocked
      ? 'unlocked'
      : 'locked'

  /**
   * The §11.6 hide bridge shared by `[C-4j]`, `[Z4e]` and `[Z5d]`. Hiding is
   * **not** gated on the session (§7.10 c19 decision 12, human 2026-10-17): the
   * controls are what make the gate reachable, so an unconfigured vault routes
   * the human to the first-run gate with a warn toast, but a *configured* vault
   * hides without asking for the password — putting a file away is not a look at
   * what is inside. Only reading the hidden set (the Cofre grid) requires unlock.
   */
  const requestHide = useCallback(
    async (ids: string[]) => {
      if (ids.length === 0) return
      if (!vault?.configured) {
        push('warn', t('media.hideNotConfigured'))
        navigate('vault')
        return
      }
      const moved = await hide(ids)
      if (moved === null) return
      push('success', t('media.hidden', { count: moved }))
      selection.clear()
    },
    [vault, push, t, navigate, hide, selection],
  )

  const requestHideFolder = useCallback(
    async (folder: string) => {
      if (!vault?.configured) {
        push('warn', t('media.hideNotConfigured'))
        navigate('vault')
        return
      }
      const moved = await hideFolder(folder)
      if (moved === null) return
      push('success', t('media.hidden', { count: moved }))
      selection.clear()
    },
    [vault, push, t, navigate, hideFolder, selection],
  )

  // The Zone 4 drawer (below 1280px, human-confirmed 2026-09-29) is state the
  // *shell* owns, not the panel: §10.6.5 makes `Esc` "clear selection or close
  // the topmost overlay", so the shortcut map below has to know whether the drawer
  // is up, and the map is the shell's. The state carries its route with it and a
  // stale one is ignored, so navigating away closes the overlay *derived from the
  // render* instead of through a `setState` inside an effect (which would also
  // cost an extra render). Coming back to an inspector that is still open after a
  // detour through Settings is a state nobody asked for.
  const [drawer, setDrawer] = useState<{ route: Route; open: boolean }>({ route, open: false })
  const drawerOpen = drawer.open && drawer.route === route
  const setDrawerOpen = useCallback((open: boolean) => setDrawer({ route, open }), [route])
  const closeDrawer = useCallback(() => setDrawerOpen(false), [setDrawerOpen])

  // Only the shell-scoped bindings live here. `Esc` closes the inspector drawer
  // or clears the selection, both of which are shell state; `Ctrl/Cmd+A` is
  // deliberately NOT here even though §12.6 lists it — "select all in the
  // current spec" needs the media page's own items, so it belongs to the surface
  // that owns them (`MediaScreen`), not to the chrome. (It used to clear the
  // selection, which was the opposite of the map.)
  const shortcuts = useMemo<Shortcut[]>(
    () => [
      {
        key: 'escape',
        run: () => {
          if (drawerOpen) {
            closeDrawer()
          } else {
            selection.clear()
          }
        },
      },
    ],
    [selection, drawerOpen, closeDrawer],
  )
  useShortcuts(shortcuts)

  // Zone 4 collapses on the screens with no selection model (§10.6.3): the
  // canvas takes the space and the panel is never empty chrome.
  const selectionMode = useInspector(selection.count)
  const inspectorMode = hasSelectionModel(route) ? selectionMode : 'none'

  return (
    <div className="shell" data-route={route}>
      <Header
        route={route}
        onNavigate={navigate}
        dbHealthy={Boolean(stats) && !statsError}
        dbUnknown={statsLoading}
      />

      <div className="shell__body">
        <Sidebar
          route={route}
          onNavigate={navigate}
          mediaCount={stats ? stats.totalItems : null}
          vaultState={vaultState}
        />

        <main className="zone3" aria-label={route}>
          {route === 'dashboard' ? (
            <DashboardScreen
              stats={stats}
              loading={statsLoading}
              error={statsError}
              onRetry={rerun}
              resolve={resolveAppRoot}
              onNavigate={navigate}
            />
          ) : null}
          {route === 'media' ? (
            <MediaScreen
              selection={selection}
              vault={{
                configured: Boolean(vault?.configured),
                onHide: (id: string) => void requestHide([id]),
                onHideFolder: (folder: string) => void requestHideFolder(folder),
              }}
            />
          ) : null}
          {route === 'lists' ? <ListsScreen /> : null}
          {route === 'backup' ? <BackupScreen /> : null}
          {route === 'settings' ? <SettingsScreen /> : null}
          {route === 'vault' ? <VaultScreen /> : null}
        </main>

        <Inspector
          mode={inspectorMode}
          count={selection.count}
          onClose={selection.clear}
          drawerOpen={drawerOpen}
          onDrawerChange={setDrawerOpen}
          vaultConfigured={Boolean(vault?.configured)}
          onHideSelected={() => void requestHide([...selection.selected])}
        />
      </div>

      {/* Order matters and it is not cosmetic: `.transfer ~ .zone5` in
          shell.css lifts the floating bulk bar clear of the strip, and a
          sibling combinator only reaches *later* siblings. */}
      <TransferBar />
      {/* Zone 5 belongs to the screens that own the shell selection (dashboard /
          media / lists). It is not the Cofre's — that screen has its own grid
          toolbar (§11.6) — and not Backup/Settings' either. */}
      {hasSelectionModel(route) && route !== 'vault' ? (
        <BulkToolbar
          count={selection.count}
          onClear={selection.clear}
          vaultConfigured={Boolean(vault?.configured)}
          onHideSelected={() => void requestHide([...selection.selected])}
        />
      ) : null}
      <ToastSock />
    </div>
  )
}
