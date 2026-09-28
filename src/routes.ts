/**
 * The five routes of the shell (C-15). USB and Wi-Fi are **source tabs inside
 * Backup**, not routes; Cofre is a route of its own because it is a guarded
 * space, not a menu item.
 */
export const ROUTES = ['dashboard', 'media', 'lists', 'backup', 'settings', 'vault'] as const

export type Route = (typeof ROUTES)[number]

export const DEFAULT_ROUTE: Route = 'dashboard'

/**
 * §10.6.3 — Zone 4 collapses (width 0) on the screens with no selection model
 * (Backup/ingest and Settings) and the canvas takes the space.
 */
export function hasSelectionModel(route: Route): boolean {
  return route === 'dashboard' || route === 'media' || route === 'lists' || route === 'vault'
}
