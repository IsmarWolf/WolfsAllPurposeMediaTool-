//! Root, maintenance and system commands (PLAN §5.6, §8.1, §11.7).
//!
//! The root is a *settings* concern, so the diagnostic `resolve_app_root` and
//! the `[Recalcular Raiz]` action live here together.

use tauri::State;

use crate::core::db;
use crate::core::models::{AppVersionsDto, RecalcRootDto, RootInfoDto};
use crate::error::{AppError, SpawnError};
use crate::state::AppState;

/// §8.1 `resolve_app_root` — the catalog says `{ root: string }`; c4 returns the
/// full `RootInfoDto` because Zone 1 `[Z1d]`, Settings → Origem and the C-1k card
/// all need `source`, `rooted`, `firstRun` and `ffmpegOk`, and a second command
/// for the same boot fact would just be noise. `root` is still there.
#[tauri::command]
pub fn resolve_app_root(state: State<'_, AppState>) -> Result<RootInfoDto, AppError> {
    Ok(state.root_info())
}

/// §5.6 `[C-7b Recalcular raiz]` — re-runs §5.2 and re-validates ffmpeg. When
/// the root moved, the tree and the pool are rebuilt; the UI says so via
/// `changed` and re-reads the Dashboard numbers.
#[tauri::command]
pub fn settings_recalc_root(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<RecalcRootDto, AppError> {
    // c10: the asset scope is bound to a root, so a moved root re-grants it —
    // the previous `Thumbnails/` is forbidden, since v2's scope has no removal.
    let previous = state.snapshot().resolver.root().to_path_buf();
    let (info, changed) = state.recalc_root()?;
    if changed {
        let snapshot = state.snapshot();
        crate::core::thumbs::scope_thumbnails(&app, &snapshot.resolver, Some(&previous));
    }
    Ok(RecalcRootDto { changed, info })
}

/// §5.6 `[C-7a Abrir pasta]` / §8.1 `settings_reveal_root` — Explorer on the
/// root. The path is derived inside from the resolver, never accepted from the
/// frontend (§8 preamble: no command takes an absolute path).
#[tauri::command]
pub fn settings_reveal_root(state: State<'_, AppState>) -> Result<(), AppError> {
    let snapshot = state.snapshot();
    reveal(snapshot.resolver.root())
}

/// §7.2 `settings_vacuum` / §11.7 `[C-7g BD: compactar]`. Blocking, so it is
/// moved off the async runtime: a full-file rewrite must never stall the UI.
#[tauri::command]
pub async fn settings_vacuum(state: State<'_, AppState>) -> Result<(), AppError> {
    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
    tauri::async_runtime::spawn_blocking(move || db::vacuum(&pool))
        .await
        .map_err(|error| AppError::Unavailable(error.to_string()))?
        .map_err(AppError::from)
}

/// §8.1 `app_versions` — the Sistema card of §11.7. Added in c4: the catalog had
/// no command for "app & crate versions, OS" even though §11.7 requires it.
#[tauri::command]
pub fn app_versions() -> AppVersionsDto {
    AppVersionsDto {
        app_version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
    }
}

/// §8.1 `media_reveal` will reuse this: `explorer /select,<abs>`. Both the
/// `/select` form and the plain-folder form live in one place so the quoting
/// rule (a path with spaces needs no extra quoting — it is one argv entry).
pub(crate) fn reveal(path: &std::path::Path) -> Result<(), AppError> {
    std::process::Command::new("explorer")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|source| {
            AppError::Device(
                SpawnError {
                    program: "explorer".into(),
                    source,
                }
                .to_string(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_reported_from_the_build_and_the_host() {
        let versions = app_versions();
        assert_eq!(versions.app_version, env!("CARGO_PKG_VERSION"));
        assert!(!versions.os.is_empty());
        assert!(!versions.arch.is_empty());
    }
}
