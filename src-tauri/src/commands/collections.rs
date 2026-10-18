//! Collection commands (PLAN §8.1, c21). Thin wrappers over `core::collections`:
//! no semantics here (the core module owns the predicates), just the shell's read
//! surface. `media_set_favorite` lives in `commands::media` beside the other
//! id-based media commands (§24.2).

use tauri::State;

use crate::core::collections;
use crate::core::models::{CollectionDto, CollectionOpenDto, MediaQueryDto, MediaScope};
use crate::error::AppError;
use crate::state::AppState;

/// §8.1 `collections_list` — the five smart collections with counts + covers.
/// Scoped like every read (§6.6): the shell passes `Standard` (§7.2d).
#[tauri::command]
pub fn collections_list(
    state: State<'_, AppState>,
    scope: MediaScope,
) -> Result<Vec<CollectionDto>, AppError> {
    let pool = state.snapshot().require_pool()?;
    Ok(collections::list(&pool, scope)?)
}

/// §8.1 `collection_open` — `media_query` with the collection predicate applied
/// on top of the caller's spec (§7.2d/§24.1 decision 3). The scope is forced to
/// `standard` inside the core module (C6).
#[tauri::command]
pub fn collection_open(
    state: State<'_, AppState>,
    payload: CollectionOpenDto,
) -> Result<MediaQueryDto, AppError> {
    let pool = state.snapshot().require_pool()?;
    Ok(collections::open(&pool, &payload.key, &payload.spec)?)
}
