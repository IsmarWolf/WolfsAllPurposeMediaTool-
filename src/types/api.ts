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

/**
 * §8.1 `ingest_wifi_start` (c11) — the live facts of the Wi-Fi server, as the
 * Backup → Wi-Fi pane paints them.
 */
export interface WifiInfoDto {
  lanIp: string
  /**
   * Row-major QR modules with `true` = dark. Generated in Rust (c11 §7.8) so the
   * pane only draws squares and the QR encoder stays in one place.
   */
  qrMatrix: boolean[][]
  port: number
  /** Live counter: uploads already accepted by this session. */
  uploads: number
}

/**
 * §8.1 `devices_list` (c12) — one attached MTP/PTP device.
 *
 * Deliberately small. The `winmtp` wrapper covers enumeration and content but not
 * the device-level capability properties, so §8.1's `capacity` / `free` /
 * `protocol` are **not** claimed here: guessing them from a Windows handle would
 * be worse than omitting them, and PLAN §8.1 needs an honest amendment rather than
 * a fabricated zero. What the pane needs in order to act is present.
 */
export interface DeviceDto {
  /** Opaque WPD device id. Windows owns it; the start payload addresses the
   *  device by `friendlyName` instead, because this id is not stable. */
  id: string
  /** §5.4-sanitized label: what `Media/<name>/` will be called. */
  name: string
  /** Exactly as the device reports it, e.g. "iPhone" or "Pixel 8 Pro". */
  friendlyName: string
}

/** §8.1 `ingest_usb_start` payload. */
export interface IngestUsbStartDto {
  deviceFriendly: string
  /** `copy` keeps the phone's file; `move` deletes it after the §7.3 pass. */
  mode: UsbModeDto
  /** Overrides the `Media/<label>/` name; omitted keeps the device's own. */
  destLabel?: string | null
  /** §8.1's "só arquivos novos": skip objects already indexed by name+size. */
  onlyNew?: boolean
}

/** §8.1's `mode: copy | move`. `move` is the destructive one and is gated on the
 *  §12.1 order (write → hash → DB insert → *then* delete). */
export type UsbModeDto = 'copy' | 'move'

/**
 * `wolfs://progress/ingest` payload (§9.2) — one tagged enum, because the pane
 * needs all five facts and the channel carries no job id: how the pull is going,
 * how the scan is going, what it finally did, that the counts moved, and that one
 * device failed while the pass carried on (§12.1).
 */
export type IngestEventDto =
  | { kind: 'pull'; current: number; total: number; lastName: string }
  | ({ kind: 'progress' } & ScanProgressDto)
  | ({ kind: 'summary' } & ScanSummaryDto)
  | { kind: 'dbChanged' }
  | { kind: 'failed'; device: string; message: string }

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
