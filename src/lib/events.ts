/**
 * §9.2 event channels. Background work reaches the UI **only** through these;
 * commands return final values. The names are mirrored from
 * `state.rs::channels` — keep both in sync.
 */

export const EVENTS = {
  ingestProgress: 'wolfs://progress/ingest',
  scanProgress: 'wolfs://progress/scan',
  thumbProgress: 'wolfs://thumb/progress',
  thumbDone: 'wolfs://thumb/done',
  vault: 'wolfs://vault',
  toast: 'wolfs://toast',
  dbChanged: 'wolfs://db-changed',
} as const

export type EventChannel = (typeof EVENTS)[keyof typeof EVENTS]
