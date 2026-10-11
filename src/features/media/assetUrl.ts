import { convertFileSrc } from '@tauri-apps/api/core'

/**
 * §10.6.1 + c10 decision (human 2026-10-01): tile sources are served by
 * Tauri's asset protocol, so the webview can read files off the SSD.
 *
 * `root` is `RootInfoDto.root` — the one absolute path the frontend is allowed
 * (§22 note on media DTOs) — and `relative` is the row's `thumb200`/`thumb400`.
 * The backend scopes that directory to `Thumbnails/` only, so an empty string
 * here (root not resolved yet) is the safe state: the grid shows its
 * no-thumbnail placeholder instead of a broken-image icon.
 *
 * It never throws either: `convertFileSrc` needs Tauri's webview internals, and
 * a tile is not worth taking the gallery down for.
 */
export function thumbUrl(root: string | null | undefined, relative: string): string {
  return scopedUrl(root, relative)
}

/**
 * §8.1/c13 fix (human-approved 2026-10-05): the lightbox plays a video from its
 * `Media/` file, which is only possible because the backend granted that one
 * file to the asset protocol (`media_scope_video`) — the `Thumbnails/` scope
 * cannot serve a frame. Images never come this way. Same empty-string-on-failure
 * contract as `thumbUrl`; a not-yet-scoped video is the safe blank state.
 */
export function mediaUrl(root: string | null | undefined, relative: string): string {
  return scopedUrl(root, relative)
}

function scopedUrl(root: string | null | undefined, relative: string): string {
  if (!root || !relative) return ''
  const absolute = `${root.replace(/\\/g, '/').replace(/\/+$/, '')}/${relative.replace(/\\/g, '/')}`
  try {
    return convertFileSrc(absolute)
  } catch {
    return ''
  }
}
