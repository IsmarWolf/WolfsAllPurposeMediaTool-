//! Vault commands (PLAN §8.1, c18). Thin wrappers over `core::vault`: the
//! credential check, the hide/restore moves and the attribute helper all live in
//! the core module (§7.11). Here we hold the session in `AppState`, require it to
//! be unlocked for the **reading** side of the vault (`vault_unhide`), and signal
//! `wolfs://vault` / `wolfs://db-changed` so open panels refetch (§9.2).
//!
//! **Hiding is not gated on the session** (§7.10 c19 decision 12, human
//! 2026-10-17): putting a file away needs no password, only a configured vault.
//! Only seeing the hidden set (the §11.6 grid, `vault_unhide`) requires unlock.
//!
//! **`vault_hide_folder` is the one command §8.1's catalog omits** — §12.5.6 made
//! folder-level hide a real operation in c17, and no catalog command can carry a
//! folder (`vault_hide` is a batch of ids). Recorded as a §8.1 amendment for c18,
//! the same way c15 added `lists_detail`.

use tauri::http::{Request, Response, StatusCode, header};
use tauri::{AppHandle, Emitter, Manager, State, UriSchemeContext, UriSchemeResponder};

use crate::core::db;
use crate::core::models::VaultStatusDto;
use crate::core::vault;
use crate::error::AppError;
use crate::state::{AppState, channels};

/// c19: the custom URI scheme the vault grid's tiles are served through.
///
/// The c10 asset-protocol scope could not carry this: Tauri v2's `forbid_*` has
/// no removal and always wins over `allow_*` (`tauri/src/scope/fs.rs`), so a
/// grant opened on unlock could never be closed on lock without poisoning every
/// later unlock. Instead the webview reads `<scheme>://localhost/<thumb name>`
/// and this handler re-checks the live session on **every** request — C6 holds
/// mid-session: locking the vault makes the very next tile request fail.
pub const VAULT_THUMB_SCHEME: &str = "wolfsvault";

/// c19: the §11.6 `[C-6g]` tile source. Serves one file from `.vault/thumbs/`.
///
/// The frontend passes only the thumbnail's **file name** (`convertFileSrc`
/// percent-encodes it into the URL path), so the handler never accepts a path
/// from the webview: a single, separator-free component is the only shape that
/// resolves, and anything else is a `400` (§8 preamble: no command takes an
/// absolute path). Locked (or a missing file) is the same honest blank tile the
/// gallery shows for a not-yet-rendered thumbnail.
pub fn vault_thumb_protocol(
    ctx: UriSchemeContext<'_, tauri::Wry>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let state = ctx.app_handle().state::<AppState>();
    if !state.vault_unlocked() {
        responder.respond(status(StatusCode::FORBIDDEN));
        return;
    }

    let requested = request.uri().path().trim_start_matches('/').to_string();
    // A bare, single-component name is the only accepted shape. `file_name()`
    // rejects `..`, `a/b` and absolute fragments in one check; the explicit
    // `..` test keeps a Windows-only separator trick from slipping through.
    let safe = !requested.is_empty()
        && !requested.contains("..")
        && std::path::Path::new(&requested)
            .file_name()
            .is_some_and(|name| name.to_string_lossy() == requested);
    if !safe {
        responder.respond(status(StatusCode::BAD_REQUEST));
        return;
    }

    // Resolve inside the current root's vault thumbnails and read off-thread: a
    // large library paints many tiles at once and a blocking read on the IPC
    // thread would serialize them.
    let abs = state.snapshot().resolver.vault_thumbs().join(&requested);
    tauri::async_runtime::spawn_blocking(move || match std::fs::read(&abs) {
        Ok(bytes) => {
            let response = Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "image/webp")
                // `no-store` on purpose: a locked vault must not keep serving a
                // cached tile from the webview's HTTP cache after lock.
                .header(header::CACHE_CONTROL, "no-store")
                .body(bytes)
                .expect("vault thumbnail response");
            responder.respond(response);
        }
        Err(_) => responder.respond(status(StatusCode::NOT_FOUND)),
    });
}

/// An empty-bodied response with the given status, for the handler's rejections.
fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(code)
        .body(Vec::new())
        .expect("empty vault thumbnail response")
}

/// §12.5.3 — the reading side of the vault only runs in Vault Mode. Requiring the
/// session here is what makes C6 hold at the command edge: a locked vault cannot
/// give a hidden file back, even if a stale UI still shows the selection.
fn require_unlocked(state: &State<'_, AppState>) -> Result<(), AppError> {
    if state.vault_unlocked() {
        Ok(())
    } else {
        Err(AppError::from(vault::VaultError::Locked))
    }
}

/// §8.1 `vault_status` — the Dashboard mini-card (`[C-1f]`) and the §11.6 nav dot.
/// `configured` comes from `vault_config`; `unlocked` is the live session flag.
#[tauri::command]
pub fn vault_status(state: State<'_, AppState>) -> Result<VaultStatusDto, AppError> {
    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
    let mut status = db::vault_probe(&pool)?;
    status.unlocked = state.vault_unlocked();
    Ok(status)
}

/// §8.1 `vault_setup` — first run. Returns the recovery key **exactly once** for
/// the §11.6 `[C-6e]` display; it is never retrievable again. The session is
/// deliberately **not** opened: §11.6 requires the user to re-enter the password
/// to enter Vault Mode after setup.
#[tauri::command]
pub fn vault_setup(
    app: AppHandle,
    state: State<'_, AppState>,
    password: String,
) -> Result<String, AppError> {
    let pool = state.snapshot().require_pool()?;
    let recovery_key = vault::setup(&pool, &password)?;
    let _ = app.emit(channels::VAULT, ());
    let _ = app.emit(channels::DB_CHANGED, ());
    Ok(recovery_key)
}

/// §8.1 `vault_unlock` — verify the password and open the session.
#[tauri::command]
pub fn vault_unlock(
    app: AppHandle,
    state: State<'_, AppState>,
    password: String,
) -> Result<(), AppError> {
    let pool = state.snapshot().require_pool()?;
    let session = state.vault_session();
    let mut guard = session.lock().expect("vault session poisoned");
    vault::unlock(&pool, &mut guard, &password)?;
    drop(guard);
    let _ = app.emit(channels::VAULT, ());
    Ok(())
}

/// §8.1 `vault_recover` — the §12.5.5 recovery key as an alternative unlock.
#[tauri::command]
pub fn vault_recover(
    app: AppHandle,
    state: State<'_, AppState>,
    recovery_key: String,
) -> Result<(), AppError> {
    let pool = state.snapshot().require_pool()?;
    let session = state.vault_session();
    let mut guard = session.lock().expect("vault session poisoned");
    vault::recover(&pool, &mut guard, &recovery_key)?;
    drop(guard);
    let _ = app.emit(channels::VAULT, ());
    Ok(())
}

/// §8.1 `vault_lock` — close Vault Mode. Idempotent: locking a closed vault is
/// not an error.
#[tauri::command]
pub fn vault_lock(app: AppHandle, state: State<'_, AppState>) -> Result<(), AppError> {
    let session = state.vault_session();
    vault::lock(&mut session.lock().expect("vault session poisoned"));
    let _ = app.emit(channels::VAULT, ());
    Ok(())
}

/// §8.1 `vault_hide` — hide a batch of ids (§12.5.2). Returns how many moved.
/// Needs only a configured vault, never an unlocked one (§7.10 c19 decision 12).
#[tauri::command]
pub fn vault_hide(
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> Result<usize, AppError> {
    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
    vault::ensure_configured(&pool)?;
    let hidden = vault::hide_batch(&snapshot.resolver, &pool, &ids)?;
    let _ = app.emit(channels::VAULT, ());
    let _ = app.emit(channels::DB_CHANGED, ());
    Ok(hidden)
}

/// §8.1 (amended in c18) `vault_hide_folder` — hide a whole folder (§12.5.6). The
/// `folder` is a root-relative `Media/...` path; every visible item under it moves
/// into the vault with its subtree preserved. Configured-only, like `vault_hide`.
#[tauri::command]
pub fn vault_hide_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    folder: String,
) -> Result<usize, AppError> {
    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
    vault::ensure_configured(&pool)?;
    let hidden = vault::hide_folder(&snapshot.resolver, &pool, &folder)?;
    let _ = app.emit(channels::VAULT, ());
    let _ = app.emit(channels::DB_CHANGED, ());
    Ok(hidden)
}

/// §8.1 `vault_unhide` — restore a batch of ids to their `restore_path` (§12.5.3).
#[tauri::command]
pub fn vault_unhide(
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> Result<usize, AppError> {
    require_unlocked(&state)?;
    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
    let restored = vault::unhide_batch(&snapshot.resolver, &pool, &ids)?;
    let _ = app.emit(channels::VAULT, ());
    let _ = app.emit(channels::DB_CHANGED, ());
    Ok(restored)
}
