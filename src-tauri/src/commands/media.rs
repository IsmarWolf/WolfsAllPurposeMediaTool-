//! Read-side commands over the media table (PLAN §8.1).
//!
//! c4 ships only what the Dashboard needs; `media_query`, `media_detail`,
//! `media_reveal` and `media_remove` land with the gallery (c8/c10).

use tauri::State;

use crate::core::db;
use crate::core::models::{MediaScope, StatsDto, VaultStatusDto};
use crate::error::AppError;
use crate::state::AppState;

/// §8.1 `stats_get` — Dashboard stat cards (C-1a..1d) and the Zone 4 summary.
///
/// The scope is a parameter, never a UI choice hidden inside the query (§6.6):
/// the shell always asks for `Standard`; only Vault Mode passes `Vault`.
#[tauri::command]
pub fn stats_get(state: State<'_, AppState>) -> Result<StatsDto, AppError> {
    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
    Ok(db::stats(&pool, MediaScope::Standard)?)
}

/// §8.1 `vault_status` — the Dashboard vault mini-card (C-1f). `configured` is a
/// real read of `vault_config`; `unlocked` is c18's session flag.
#[tauri::command]
pub fn vault_status(state: State<'_, AppState>) -> Result<VaultStatusDto, AppError> {
    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
    Ok(db::vault_probe(&pool)?)
}
