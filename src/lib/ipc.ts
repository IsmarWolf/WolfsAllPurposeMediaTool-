/**
 * The typed IPC surface (PLAN §10.4): one function per command, and **no command
 * string is ever typed by hand in a component**. Errors are converted to
 * `AppError` at this boundary and nowhere else.
 */

import { invoke } from '@tauri-apps/api/core'
import { toAppError } from './errors'
import type {
  AppVersionsDto,
  CityCountDto,
  DeviceDto,
  FilterSpec,
  GeoDto,
  IngestUsbStartDto,
  MediaDto,
  MediaQueryDto,
  MediaScopeDto,
  RecalcRootDto,
  RootInfoDto,
  ScanSummaryDto,
  StatsDto,
  VaultStatusDto,
  WifiInfoDto,
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

/**
 * §8.1 `ingest_wifi_start` — §11.3's Wi-Fi pane. Idempotent by design: a second
 * call returns the *running* server's facts (with its live upload count) instead
 * of `E_CONFLICT`, because the pane polls this to show "aguardando uploads… n
 * enviados" and a poll must never look like an error. The server outlives the
 * pane, so the promise resolves as soon as the listener is up.
 */
export const ingestWifiStart = () => call<WifiInfoDto>('ingest_wifi_start')

/**
 * §8.1 `ingest_wifi_stop` — §12.2's `[C-3k Stop]`. The accept loop closes,
 * in-flight uploads finish, and the pipeline job keeps running. Idempotent.
 */
export const ingestWifiStop = () => call<void>('ingest_wifi_stop')

/**
 * §8.1 `devices_list` (c12) — attached MTP/PTP devices for §12.1's
 * `[C-3e Conectar iPhone]` flow. Resolves an empty list when nothing is plugged
 * in *and* on a build without the MTP transport: the two are indistinguishable to
 * the pane, and `ingestUsbStart` is where the difference surfaces as an error.
 */
export const devicesList = () => call<DeviceDto[]>('devices_list')

/**
 * §8.1 `ingest_usb_start` (c12) — claims the §9.3 job slot and returns its
 * `JobId` immediately.
 *
 * The promise resolving is *not* the import finishing: a 4 GB video takes minutes,
 * so the counts arrive on `wolfs://progress/ingest` as `pull` → `progress` →
 * `summary` (`useUsbIngest`). `mode: 'move'` deletes from the phone only after the
 * §7.3 pass has hashed and indexed the files — see `core/usb.rs`.
 */
export const ingestUsbStart = (payload: IngestUsbStartDto) =>
  call<string>('ingest_usb_start', { payload })

/**
 * §8.1 `ingest_usb_stop` (c12) — §12.1's `[C-3k Parar]`. Flips the shared
 * `CancelFlag`: the pull stops before the next object and the delete phase is
 * never reached. Idempotent.
 */
export const ingestUsbStop = () => call<void>('ingest_usb_stop')

/** §8.1 `geo_lookup` — debug/testing only (§7.5.1 decision 8). */
export const geoLookup = (lat: number, lon: number) =>
  call<GeoDto | null>('geo_lookup', { lat, lon })

/** §8.1 `media_query` — one page of gallery results. */
export const mediaQuery = (spec: FilterSpec) => call<MediaQueryDto>('media_query', { spec })

/** §8.1 `city_counts` — the L2 `Localização` tag-cloud (§10.6.1), scoped. */
export const cityCounts = (scope: MediaScopeDto) => call<CityCountDto[]>('city_counts', { scope })

/** §8.1 `media_detail` — one media item for the lightbox. */
export const mediaDetail = (id: string) => call<MediaDto | null>('media_detail', { id })

/** §8.1 `media_reveal` — Explorer on the media's folder. */
export const mediaReveal = (id: string) => call<void>('media_reveal', { id })

/** §8.1 `media_scope_video` — grant the asset protocol this one video file so
 *  the lightbox can play it (§6.7/c10 amendment, 2026-10-05). */
export const mediaScopeVideo = (id: string) => call<void>('media_scope_video', { id })

/** §8.1 `media_remove` — delete media rows by id. */
export const mediaRemove = (ids: string[]) => call<number>('media_remove', { ids })
