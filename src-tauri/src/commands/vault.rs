//! Vault commands (PLAN §8.1, c18). Thin wrappers over `core::vault`: the
//! credential check, the hide/restore moves and the attribute helper all live in
//! the core module (§7.11). Here we hold the session in `AppState`, require it to
//! be unlocked for the mutating commands, and signal `wolfs://vault` /
//! `wolfs://db-changed` so open panels refetch (§9.2).
//!
//! **`vault_hide_folder` is the one command §8.1's catalog omits** — §12.5.6 made
//! folder-level hide a real operation in c17, and no catalog command can carry a
//! folder (`vault_hide` is a batch of ids). Recorded as a §8.1 amendment for c18,
//! the same way c15 added `lists_detail`.

use tauri::{AppHandle, Emitter, State};

use crate::core::db;
use crate::core::models::VaultStatusDto;
use crate::core::vault;
use crate::error::AppError;
use crate::state::{AppState, channels};

/// §12.5.3/§12.5.2 — the mutating commands only run in Vault Mode. Requiring the
/// session here is what makes C6 hold at the command edge: a locked vault cannot
/// move a file, even if a stale UI still shows the selection.
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
#[tauri::command]
pub fn vault_hide(
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> Result<usize, AppError> {
    require_unlocked(&state)?;
    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
    let hidden = vault::hide_batch(&snapshot.resolver, &pool, &ids)?;
    let _ = app.emit(channels::VAULT, ());
    let _ = app.emit(channels::DB_CHANGED, ());
    Ok(hidden)
}

/// §8.1 (amended in c18) `vault_hide_folder` — hide a whole folder (§12.5.6). The
/// `folder` is a root-relative `Media/...` path; every visible item under it moves
/// into the vault with its subtree preserved.
#[tauri::command]
pub fn vault_hide_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    folder: String,
) -> Result<usize, AppError> {
    require_unlocked(&state)?;
    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
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
