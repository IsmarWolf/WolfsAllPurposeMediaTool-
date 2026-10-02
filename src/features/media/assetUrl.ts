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
  if (!root || !relative) return ''
  const absolute = `${root.replace(/\\/g, '/').replace(/\/+$/, '')}/${relative.replace(/\\/g, '/')}`
  try {
    return convertFileSrc(absolute)
  } catch {
    return ''
  }
}
