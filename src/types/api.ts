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

export type RootSourceDto = 'wolfsRootEnv' | 'markerWalk' | 'unrootedFallback'

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

/** `wolfs://toast` payload (§9.2). */
export type ToastKind = 'info' | 'success' | 'warn' | 'error'

export interface ToastPayload {
  kind: ToastKind
  message: string
}
