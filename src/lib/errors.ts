/**
 * The single error surface of the frontend (PLAN §8.2, §10.4).
 *
 * Rust serializes failures as `{ code, message }`; every command rejection is
 * converted into an `AppError` that carries the **i18n key** of its code, so a
 * component never parses a message string and never invents its own wording.
 */

export type ErrorCode =
  | 'E_DB'
  | 'E_ROOT'
  | 'E_FFMPEG'
  | 'E_DEVICE'
  | 'E_CONFLICT'
  | 'E_AUTH'
  | 'E_INGEST'
  | 'E_CANCEL'
  | 'E_UNAVAILABLE'

/** §8.2: code → i18n key. `E_CANCEL` is silent in the UI, so it has no key. */
export const ERROR_I18N_KEY: Record<ErrorCode, string | null> = {
  E_DB: 'error.E_DB',
  E_ROOT: 'error.E_ROOT',
  E_FFMPEG: 'error.E_FFMPEG',
  E_DEVICE: 'error.E_DEVICE',
  E_CONFLICT: 'error.E_CONFLICT',
  E_AUTH: 'error.E_AUTH',
  E_INGEST: 'error.E_INGEST',
  E_CANCEL: null,
  E_UNAVAILABLE: 'error.E_UNAVAILABLE',
}

export class AppError extends Error {
  readonly code: ErrorCode
  /** Humanized text for the retry card, straight from the backend. */
  readonly detail: string

  constructor(code: ErrorCode, detail: string) {
    super(detail)
    this.name = 'AppError'
    this.code = code
    this.detail = detail
  }

  get i18nKey(): string | null {
    return ERROR_I18N_KEY[this.code]
  }

  /** §8.2: a cancelled job is silent — never a toast, never an error card. */
  get isSilent(): boolean {
    return this.code === 'E_CANCEL'
  }
}

const KNOWN_CODES: readonly ErrorCode[] = [
  'E_DB',
  'E_ROOT',
  'E_FFMPEG',
  'E_DEVICE',
  'E_CONFLICT',
  'E_AUTH',
  'E_INGEST',
  'E_CANCEL',
  'E_UNAVAILABLE',
]

function isErrorCode(value: unknown): value is ErrorCode {
  return typeof value === 'string' && (KNOWN_CODES as readonly string[]).includes(value)
}

type SerializedError = { code?: unknown; message?: unknown }

/** Anything thrown by `invoke` becomes an `AppError`; unknown shapes do not leak. */
export function toAppError(raw: unknown): AppError {
  if (raw instanceof AppError) {
    return raw
  }

  if (typeof raw === 'object' && raw !== null) {
    const { code, message } = raw as SerializedError
    if (isErrorCode(code)) {
      return new AppError(code, typeof message === 'string' ? message : code)
    }
  }

  if (typeof raw === 'string') {
    return new AppError('E_UNAVAILABLE', raw)
  }

  return new AppError('E_UNAVAILABLE', 'falha inesperada na comunicação com o aplicativo')
}
