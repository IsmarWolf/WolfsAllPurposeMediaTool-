//! Wi-Fi commands (PLAN §8.1): `ingest_wifi_start` and `ingest_wifi_stop`.
//!
//! Thin wrappers, §7.11: bind (or report the running server), and stop. The
//! upload pipeline itself is background work started by the listener, so its
//! facts travel on `wolfs://progress/ingest` (§9.2) — the Backup pane is c13.

use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};

use crate::core::models::{IngestEventDto, WifiInfoDto};
use crate::core::wifi;
use crate::error::AppError;
use crate::state::{AppState, channels};

/// §8.1 `ingest_wifi_start` — §7.8's listener, the LAN address and the QR matrix
/// that §11.3 paints on a `<canvas>`.
///
/// Idempotent: pressing `[C-3i Iniciar servidor]` twice returns the *same*
/// server's info (with its live upload count) instead of failing with
/// `E_CONFLICT`, because the pane polls this to show "aguardando uploads… n
/// enviados" and a poll must never look like an error.
#[tauri::command]
pub fn ingest_wifi_start(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<WifiInfoDto, AppError> {
    if let Some(running) = state.wifi_server() {
        return Ok(running.info());
    }

    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
    let app_for_events = app.clone();
    let shared = wifi::Shared::new(
        snapshot.resolver.clone(),
        pool,
        Arc::clone(&snapshot.geocoder),
        ffmpeg_path(&snapshot),
        state.jobs(),
    )
    .with_event_sink(move |event: IngestEventDto| {
        let _ = app_for_events.emit(channels::INGEST_PROGRESS, event);
    });

    let server = Arc::new(wifi::start(shared)?);
    state.set_wifi_server(Some(Arc::clone(&server)));
    Ok(server.info())
}

/// §8.1 `ingest_wifi_stop` — §12.2's `[C-3k Stop]`: the accept loop closes,
/// in-flight uploads finish, and the pipeline job keeps running. Idempotent:
/// stopping a stopped server is not an error.
#[tauri::command]
pub fn ingest_wifi_stop(state: State<'_, AppState>) -> Result<(), AppError> {
    state.stop_wifi_server();
    Ok(())
}

fn ffmpeg_path(snapshot: &crate::state::Snapshot) -> Option<std::path::PathBuf> {
    if snapshot.ffmpeg_ok {
        Some(snapshot.resolver.ffmpeg_exe())
    } else {
        None
    }
}
