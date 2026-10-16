//! List commands (PLAN §8.1, c15). Thin wrappers over `core::lists`: validate
//! nothing here (the core module owns the semantics), map errors, and signal
//! `wolfs://db-changed` after a mutation so open panels refetch (§9.2).
//!
//! `lists_detail` is the one command §8.1's catalog omits but §7.1's
//! `ListDetailDto` and §10.3's `useListDetail(id)` require — the §11.5 right pane
//! needs a list's members, and no other command can return them (a `FilterSpec`
//! has no list id). Recorded as a §8.1 amendment for c15.

use tauri::{AppHandle, Emitter, State};

use crate::core::lists;
use crate::core::models::{
    ListCreateDto, ListDetailDto, ListDto, ListItemsDto, ListSnapshotApplyDto, ListUpdateDto,
};
use crate::error::AppError;
use crate::state::{AppState, channels};

/// §8.1 `lists_list` — every list with its visible member count.
#[tauri::command]
pub fn lists_list(state: State<'_, AppState>) -> Result<Vec<ListDto>, AppError> {
    let pool = state.snapshot().require_pool()?;
    Ok(lists::list(&pool)?)
}

/// §8.1 (amended in c15) `lists_detail` — one list plus its members.
#[tauri::command]
pub fn lists_detail(
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<ListDetailDto>, AppError> {
    let pool = state.snapshot().require_pool()?;
    Ok(lists::detail(&pool, &id)?)
}

/// §8.1 `lists_create` — a new manual list; returns it so the UI can select it.
#[tauri::command]
pub fn lists_create(
    app: AppHandle,
    state: State<'_, AppState>,
    payload: ListCreateDto,
) -> Result<ListDto, AppError> {
    let pool = state.snapshot().require_pool()?;
    let created = lists::create(&pool, &payload.name)?;
    let _ = app.emit(channels::DB_CHANGED, ());
    Ok(created)
}

/// §8.1 `lists_update` — rename only.
#[tauri::command]
pub fn lists_update(
    app: AppHandle,
    state: State<'_, AppState>,
    payload: ListUpdateDto,
) -> Result<(), AppError> {
    let pool = state.snapshot().require_pool()?;
    lists::rename(&pool, &payload.id, &payload.name)?;
    let _ = app.emit(channels::DB_CHANGED, ());
    Ok(())
}

/// §8.1 `lists_delete` — delete the list and its membership.
#[tauri::command]
pub fn lists_delete(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), AppError> {
    let pool = state.snapshot().require_pool()?;
    lists::delete(&pool, &id)?;
    let _ = app.emit(channels::DB_CHANGED, ());
    Ok(())
}

/// §8.1 `lists_add_items` — manual lists only.
#[tauri::command]
pub fn lists_add_items(
    app: AppHandle,
    state: State<'_, AppState>,
    payload: ListItemsDto,
) -> Result<(), AppError> {
    let pool = state.snapshot().require_pool()?;
    lists::add_items(&pool, &payload.list_id, &payload.media_ids)?;
    let _ = app.emit(channels::DB_CHANGED, ());
    Ok(())
}

/// §8.1 `lists_remove_items` — manual lists only.
#[tauri::command]
pub fn lists_remove_items(
    app: AppHandle,
    state: State<'_, AppState>,
    payload: ListItemsDto,
) -> Result<(), AppError> {
    let pool = state.snapshot().require_pool()?;
    lists::remove_items(&pool, &payload.list_id, &payload.media_ids)?;
    let _ = app.emit(channels::DB_CHANGED, ());
    Ok(())
}

/// §8.1 `lists_snapshot_apply` — store the active filter and replace membership.
#[tauri::command]
pub fn lists_snapshot_apply(
    app: AppHandle,
    state: State<'_, AppState>,
    payload: ListSnapshotApplyDto,
) -> Result<(), AppError> {
    let pool = state.snapshot().require_pool()?;
    lists::snapshot_apply(&pool, &payload.list_id, &payload.spec)?;
    let _ = app.emit(channels::DB_CHANGED, ());
    Ok(())
}

/// §8.1 `lists_snapshot_sync` — re-run the stored criteria and re-sync members.
#[tauri::command]
pub fn lists_snapshot_sync(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), AppError> {
    let pool = state.snapshot().require_pool()?;
    lists::snapshot_sync(&pool, &id)?;
    let _ = app.emit(channels::DB_CHANGED, ());
    Ok(())
}
