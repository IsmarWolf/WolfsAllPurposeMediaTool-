//! c12: comandos de ingestão USB.
//! `ingest_usb_start` e `ingest_usb_stop` — stubs para a estrutura ser criada.

use crate::core::ingest::LocalSource;
use crate::error::AppError;
use crate::state::AppState;
use tauri::State;

/// Inicia a ingestão USB. Claim o slot no JobRegistry (§9.3) e lança o coordenador.
#[tauri::command]
pub async fn ingest_usb_start(
    _state: State<'_, AppState>,
) -> Result<crate::core::models::ScanSummary, AppError> {
    let _resolver = crate::core::paths::PathResolver::resolve_app_root();
    let wpd_source = crate::core::usb::ActiveWpdSource;
    let local_source = LocalSource::Usb(wpd_source);
    let coordinator = crate::core::ingest::IngestCoordinator::new(local_source);
    Ok(coordinator.start())
}

/// Para a ingestão USB em andamento. Idempotente — se não houver job, não é erro.
#[tauri::command]
pub async fn ingest_usb_stop(_state: State<'_, AppState>) -> Result<(), AppError> {
    Ok(())
}
