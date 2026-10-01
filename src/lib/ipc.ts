/**
 * The typed IPC surface (PLAN §10.4): one function per command, and **no command
 * string is ever typed by hand in a component**. Errors are converted to
 * `AppError` at this boundary and nowhere else.
 */

import { invoke } from '@tauri-apps/api/core'
import { toAppError } from './errors'
import type {
  AppVersionsDto,
  GeoDto,
  RecalcRootDto,
  RootInfoDto,
  ScanSummaryDto,
  StatsDto,
  VaultStatusDto,
} from '../types/api'

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args)
  } catch (raw) {
    throw toAppError(raw)
  }
}

/** §8.1 `stats_get` — Dashboard stat cards + Zone 4 summary. */
export const statsGet = () => call<StatsDto>('stats_get')

/** §8.1 `vault_status` — the C-1f vault mini-card. */
export const vaultStatus = () => call<VaultStatusDto>('vault_status')

/** §8.1 `resolve_app_root` — diagnostic; Zone 1 `[Z1d]` and Settings → Origem. */
export const resolveAppRoot = () => call<RootInfoDto>('resolve_app_root')

/** §8.1 `settings_recalc_root` — §5.6 `[C-7b Recalcular raiz]`. */
export const settingsRecalcRoot = () => call<RecalcRootDto>('settings_recalc_root')

/** §8.1 `settings_reveal_root` — §5.6 `[C-7a Abrir pasta]`. */
export const settingsRevealRoot = () => call<void>('settings_reveal_root')

/** §8.1 `settings_vacuum` — §11.7 `[C-7g BD: compactar]`. */
export const settingsVacuum = () => call<void>('settings_vacuum')

/** §8.1 `app_versions` — §11.7 Sistema card. */
export const appVersions = () => call<AppVersionsDto>('app_versions')

/**
 * §8.1 `scan_start` — §7.3's scanner. The promise resolves with the final
 * summary; progress arrives on `wolfs://progress/scan` meanwhile.
 *
 * `folder` picks the tree (§7.3.1 c8): absent = the in-place mop-up of
 * `Media/<device>/`; present = §11.3's Disco-local ingest, copy-only.
 */
export const scanStart = (device: string, folder?: string) =>
  call<ScanSummaryDto>('scan_start', { device, folder })

/** §8.1 `scan_cancel` — §9.3. Idempotent; a running scan ends with `cancelled`. */
export const scanCancel = () => call<void>('scan_cancel')

/** §8.1 `geo_lookup` — debug/testing only (§7.5.1 decision 8). */
export const geoLookup = (lat: number, lon: number) =>
  call<GeoDto | null>('geo_lookup', { lat, lon })
