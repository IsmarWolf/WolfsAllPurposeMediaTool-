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
import { useSetFavorite } from '../hooks/useFavorites'
import { useVaultActions, useVaultStatus } from '../hooks/useVault'
import { toAppError } from '../lib/errors'
import { mediaRemove, resolveAppRoot } from '../lib/ipc'
import { Button } from '../components/ui'
import { AddToListModal } from '../features/media/AddToListModal'
import { BackupScreen } from '../features/backup'
import { CollectionsScreen, type OpenCollection } from '../features/collections'
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
  const { push, pushError } = useToast()
  const [route, setRoute] = useState<Route>(DEFAULT_ROUTE)
  const selection = useSelection()
  const { data: stats, loading: statsLoading, error: statsError, rerun } = useDashStats()
  const { status: vault } = useVaultStatus()
  const { hide, hideFolder, lock } = useVaultActions()
  const setFavorite = useSetFavorite()

  // c22/§11.10: the open collection lives in the shell because opening one is
  // a *route move* (Coleções → Mídia, C-8e) and both screens are its children.
  // Leaving the gallery for any other route drops the scope — coming back to a
  // half-applied filter set nobody asked for (same rule as the drawer state).
  const [collection, setCollection] = useState<OpenCollection | null>(null)
  const openCollection = useCallback(
    (target: OpenCollection) => {
      selection.clear()
      setCollection(target)
      setRoute('media')
    },
    [selection],
  )
  const exitCollection = useCallback(() => {
    setCollection(null)
    setRoute('collections')
  }, [])

  // c22/§12.4: the add-to-list modal is shell state because both the header
  // button (via MediaScreen) and the bulk bar [Z5b] open it, and the toast's
  // [Desfazer] outlives whichever of them clicked.
  const [addToListIds, setAddToListIds] = useState<string[] | null>(null)

  // c22/§11.9 [Z5e]: the destructive confirm is shell state for the same reason
  // — the bar button only *opens* it, and C7 requires a second click before
  // `media_remove` runs. The ids are frozen when the confirm opens, so an
  // Esc-driven selection clear cannot yank the dialog shut mid-decision.
  const [removeIds, setRemoveIds] = useState<string[] | null>(null)
  const requestRemove = useCallback(
    async (ids: string[]) => {
      if (ids.length === 0) return
      try {
        // §14/c22: the backend deletes the file (and its thumbnails) first and
        // answers with how many rows actually went — a locked file keeps its
        // row, and the count is how the UI tells the partial case apart.
        const removed = await mediaRemove(ids)
        if (removed < ids.length) {
          push('warn', t('media.deletePartial', { count: removed, total: ids.length }))
        } else {
          push('success', t('media.deleted', { count: removed }))
        }
        selection.clear()
      } catch (raw) {
        pushError(toAppError(raw))
      } finally {
        setRemoveIds(null)
      }
    },
    [push, pushError, t, selection],
  )

  // c22/§24.2: MediaScreen reports the selection's favorite aggregate through
  // `onFavoriteInfo`; the shell never mirrors the gallery's items, which would
  // be a second source of truth for the page. `favInfo === 'mixed'` leaves the
  // inspector button hidden ([Z5f] in the bulk bar is where a mixed selection
  // acts, flipping everything on).
  const [favInfo, setFavInfo] = useState<'on' | 'off' | 'mixed' | null>(null)
  const onFavoriteInfo = useCallback((info: 'on' | 'off' | 'mixed' | null) => {
    setFavInfo(info)
  }, [])
  const singleId = selection.count === 1 ? ([...selection.selected][0] ?? null) : null

  // §11.6: entering the guarded space drops the shell selection, so the Mídia
  // bulk bar / inspector selection can never appear over the Cofre.
  // §12.5.7: leaving it re-locks the session. The route is the vault's only
  // "exit", so this is where the auto-lock belongs — the `wolfsvault` scheme
  // stops serving, in-flight/next opens re-gate, and the dot drops to red
  // without a manual `[C-6j]`. `vault_lock` is idempotent (§8.1), so a leave
  // while already locked is a harmless no-op.
  // c22: any navigation through `navigate` (sidebar, header, dashboard cards)
  // drops the collection scope (C-8f). The collection is only ever *entered*
  // through `openCollection`, which does not come through here — so this also
  // makes the sidebar's Mídia click read as "leave the collection, show me the
  // whole gallery", which is what a nav item is supposed to mean.
  const navigate = useCallback(
    (next: Route) => {
      if (next === 'vault') {
        selection.clear()
      } else if (route === 'vault' && vault?.unlocked) {
        void lock()
      }
      setCollection(null)
      if (next !== 'media') {
        setFavInfo(null)
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
              // c22/C-8f: the scope is chosen when the query hook *mounts* (its
              // collection ref is a mount-time capture, on purpose — a scope is
              // not a filter the user edits mid-flight). Leaving a collection
              // through the sidebar's Mídia keeps the route at 'media', so
              // without a key the gallery would stay mounted and keep querying
              // the collection it was told it had left.
              key={collection?.key ?? 'all'}
              selection={selection}
              vault={{
                configured: Boolean(vault?.configured),
                onHide: (id: string) => void requestHide([id]),
                onHideFolder: (folder: string) => void requestHideFolder(folder),
              }}
              collection={collection}
              onExitCollection={exitCollection}
              onRequestAddToList={setAddToListIds}
              onFavoriteInfo={onFavoriteInfo}
            />
          ) : null}
          {route === 'collections' ? <CollectionsScreen onOpen={openCollection} /> : null}
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
          favoriteOn={
            route === 'media' ? (favInfo === 'on' ? true : favInfo === 'off' ? false : null) : null
          }
          onToggleFavorite={
            route === 'media' && singleId && favInfo !== null
              ? () => void setFavorite([singleId], favInfo !== 'on')
              : undefined
          }
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
          // c22/§12.4 [Z5b] + §24.2 [Z5f]: both act on the media selection,
          // so they only exist on the media surface — the dashboard/lists
          // selection is a different model and neither control knows what to
          // favorite or which page to add.
          onAddToList={
            route === 'media' ? () => setAddToListIds([...selection.selected]) : undefined
          }
          onToggleFavorite={
            route === 'media' && favInfo !== null
              ? () => void setFavorite([...selection.selected], favInfo !== 'on')
              : undefined
          }
          favoriteAllOn={favInfo === 'on'}
          // c22/§11.9 [Z5e]: deleting is a media-surface act — the shell
          // selection on other routes is a different model (the dead-control
          // rule, same as [Z5b]/[Z5f]). Collections open *as* the media route,
          // so an `[Apagar]` inside a scoped gallery deletes the real files.
          onDelete={route === 'media' ? () => setRemoveIds([...selection.selected]) : undefined}
        />
      ) : null}
      {/* c22/§12.4: the add-to-list modal outlives whichever control opened
          it; the ids are the payload, the toast's [Desfazer] the exit ramp. */}
      {addToListIds ? (
        <AddToListModal ids={addToListIds} onClose={() => setAddToListIds(null)} />
      ) : null}
      {/* c22/§11.9 [Z5e]: C7's second click. Same `.md-modal` shape the Cofre
          uses for `[C-6i]` — no native ask/confirm is granted (only
          `dialog:allow-open`), and the copy says the *disk* is involved now. */}
      {removeIds ? (
        <div
          className="md-modal"
          role="dialog"
          aria-modal="true"
          aria-label={t('media.deleteTitle')}
          data-testid="media-delete-dialog"
        >
          <div className="md-modal__backdrop" onClick={() => setRemoveIds(null)} />
          <div className="md-modal__panel">
            <h2>{t('media.deleteTitle')}</h2>
            <p className="md-hint">{t('media.deleteConfirm', { count: removeIds.length })}</p>
            <div className="md-modal__footer">
              <Button variant="text" onClick={() => setRemoveIds(null)}>
                {t('common.cancel')}
              </Button>
              <button
                type="button"
                className="md-btn md-btn--filled md-btn--danger"
                data-testid="media-delete-confirm"
                onClick={() => void requestRemove(removeIds)}
              >
                {t('bulk.delete')}
              </button>
            </div>
          </div>
        </div>
      ) : null}
      <ToastSock />
    </div>
  )
}
