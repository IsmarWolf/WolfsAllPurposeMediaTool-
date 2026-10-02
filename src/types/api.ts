/**
 * Serde-mirrored types (PLAN §10.5). DTO fields are camelCase on the wire
 * (`rename_all = "camelCase"` in `core/models.rs`).
 *
 * No media DTO carries an absolute path; `RootInfoDto.root` is the one
 * deliberate exception, because Settings → Origem displays the resolved root
 * (§5.6).
 */

export type MediaScopeDto = 'standard' | 'vault'

export interface DeviceCountDto {
  name: string
  count: number
  bytes: number
}

/** §7.2 `stats(pool, scope)`. */
export interface StatsDto {
  totalItems: number
  images: number
  videos: number
  /** C7: no capture date **and** no GPS. */
  noMetadata: number
  hidden: number
  totalBytes: number
  imagesBytes: number
  videosBytes: number
  deviceCount: number
  topDevices: DeviceCountDto[]
}

export interface VaultStatusDto {
  configured: boolean
  unlocked: boolean
  hasRecoveryHint: boolean
}

export type RootSourceDto = 'wolfsRootEnv' | 'markerWalk' | 'packagedAppDir' | 'unrootedFallback'

/** §8.1 `resolve_app_root`, plus the boot facts Zone 1 and §11.7 display. */
export interface RootInfoDto {
  root: string
  source: RootSourceDto
  rooted: boolean
  firstRun: boolean
  ffmpegOk: boolean
}

/** §5.6 `[C-7b Recalcular raiz]`. */
export interface RecalcRootDto extends RootInfoDto {
  changed: boolean
}

/** §8.1 `app_versions` (added in c4 for the §11.7 Sistema card). */
export interface AppVersionsDto {
  appVersion: string
  os: string
  arch: string
}

/** §8.1 `scan_start` (added in c8). `cancelled: true` is a result, not an error. */
export interface ScanSummaryDto {
  device: string
  found: number
  inserted: number
  duplicates: number
  repaired: number
  organized: number
  noMetadata: number
  located: number
  locatedNone: number
  skipped: number
  cancelled: boolean
}

/** `wolfs://progress/scan` payload (§9.2): `total` is 0 until the walk is done. */
export interface ScanProgressDto {
  current: number
  total: number
  lastPath: string
  inserted: number
}

/** §8.1 `geo_lookup` (debug/testing only). */
export interface GeoDto {
  city: string
  state: string
  country: string
  latitude: number
  longitude: number
}

/** §7.1 `MediaDto` — one gallery row. */
export interface MediaDto {
  id: string
  relativePath: string
  thumb200: string
  thumb400: string
  fileHash: string
  fileSize: number
  fileType: string
  capturedAt: string | null
  hasMetadata: boolean
  deviceName: string | null
  isHidden: boolean
  location: {
    city: string | null
    state: string | null
    country: string | null
    latitude: number | null
    longitude: number | null
  } | null
}

/** §10.6.1 `FilterSpec` — the single filter object. */
export interface FilterSpec {
  scope: MediaScopeDto
  device: string | null
  cities: string[]
  month: string | null
  noMetadata: boolean
  search: string | null
  fileType: string | null
  sort: string
  limit: number
  offset: number
}

/** §8.1 `media_query` — one page of gallery results. */
export interface MediaQueryDto {
  items: MediaDto[]
  total: number
  hasMore: boolean
}

/** `wolfs://toast` payload (§9.2). */
export type ToastKind = 'info' | 'success' | 'warn' | 'error'

export interface ToastPayload {
  kind: ToastKind
  message: string
}
