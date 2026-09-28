/**
 * The typed IPC surface (PLAN §10.4): one function per command, and **no command
 * string is ever typed by hand in a component**. Errors are converted to
 * `AppError` at this boundary and nowhere else.
 */

import { invoke } from '@tauri-apps/api/core'
import { toAppError } from './errors'
import type {
  AppVersionsDto,
  RecalcRootDto,
  RootInfoDto,
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
