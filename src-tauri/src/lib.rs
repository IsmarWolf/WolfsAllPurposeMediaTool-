//! Builder wiring (PLAN §7): commands, state, events.
//!
//! c4 registers the IPC shell only — the read commands the Dashboard and
//! Settings need, plus the root/maintenance actions. The rest of the §8.1
//! catalog lands with the features that own it (c8 media, c11/c12 devices, c15
//! lists, c18 vault).

pub mod commands;
pub mod core;
pub mod error;
pub mod state;

pub use error::AppError;
pub use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Boot is §5.5 and it is deliberately non-fatal: a read-only SSD must show a
    // red `[Z1d]` and an Error card, not a silent exit (see `state::AppState`).
    let state = AppState::boot();
    if let Some(error) = state.snapshot().boot_error() {
        eprintln!("[wolfsmedia] boot degraded: {} ({})", error, error.code());
    }

    tauri::Builder::default()
        // PLAN §11.3 (c8, human 2026-10-01): the one sanctioned native plugin.
        // It backs ONLY the "Escolher pasta..." button on Backup > Disco local;
        // everything else in the app stays in-app per §19.1.
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::media::stats_get,
            commands::media::vault_status,
            commands::media::scan_start,
            commands::media::scan_cancel,
            commands::media::geo_lookup,
            commands::media::thumbs_rebuild_all,
            commands::settings::resolve_app_root,
            commands::settings::settings_recalc_root,
            commands::settings::settings_reveal_root,
            commands::settings::settings_vacuum,
            commands::settings::app_versions,
        ])
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}
