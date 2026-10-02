//! Media commands (PLAN §8.1).
//!
//! c4 shipped the Dashboard reads; c8 adds the scan itself (`scan_start` /
//! `scan_cancel`), the §11.3 Disco-local source (the same scan, `folder` set),
//! and the `geo_lookup` probe §7.5.1 decision 8 handed here. `media_query`,
//! `media_detail`, `media_reveal` and `media_remove` land with the gallery (c10).
//!
//! The Disco-local folder is chosen with the **native** Windows dialog
//! (`tauri-plugin-dialog`, human-confirmed 2026-10-01), so there is no `dir_*`
//! command here: the plugin owns the browsing, and `scan_start` receives the
//! path the human confirmed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};

use crate::core::db::{self, SqlitePool};
use crate::core::models::{FilterSpec, GeoDto, MediaScope, ScanSummary, StatsDto, VaultStatusDto};
use crate::core::paths::PathResolver;
use crate::core::scanner;
use crate::core::thumbs;
use crate::error::AppError;
use crate::state::{AppState, JobKind, JobRegistry, channels};

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

/// §8.1 `scan_start` — §7.3's scanner, as a job.
///
/// It returns the final [`ScanSummary`] and streams [`ScanProgressDto`] on
/// `wolfs://progress/scan` while it runs (§9.2). The promise resolving is how the
/// UI learns the scan is *over*; the events are how it shows it happening.
/// `scan_cancel` flips the flag meanwhile and the same promise then resolves with
/// `cancelled: true` — §9.3's "keep processed rows", never an error.
///
/// A second scan while one runs is `E_CONFLICT` rather than a second walk: two
/// scans would fight over the same `fs::rename` targets and the same rows.
///
/// `folder` picks the tree: `None` (or a `scan_start` with no folder argument) is
/// §7.3's in-place mop-up of `Media/<device>/`; `Some` is §11.3's Disco-local
/// ingest — the scanner copies from there and never touches the source (§14).
#[tauri::command]
pub async fn scan_start(
    app: AppHandle,
    state: State<'_, AppState>,
    device: String,
    folder: Option<String>,
) -> Result<ScanSummary, AppError> {
    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
    let jobs = state.jobs();
    let job = jobs.claim(JobKind::Scan)?;
    let id = job.id().to_string();
    let flag = job.flag.clone();
    let geocoder = Arc::clone(&snapshot.geocoder);
    let ffmpeg = ffmpeg_path(&snapshot);

    tauri::async_runtime::spawn_blocking(move || {
        // §7.3 vs §11.3. The folder is borrowed for the scan and dropped with the
        // closure; only the copy's own bytes ever reach the tree.
        let source = match folder.as_deref() {
            Some(path) => scanner::ScanSource::CopyFrom {
                source: Path::new(path),
            },
            None => scanner::ScanSource::Device,
        };
        let result = scanner::scan(
            &device,
            &snapshot.resolver,
            &pool,
            &geocoder,
            ffmpeg.as_deref(),
            source,
            &flag,
            |progress| {
                // §9.2: the only way background work reaches the UI. A failed
                // emit is not the scan's problem — the summary still arrives on
                // the promise.
                let _ = app.emit(channels::SCAN_PROGRESS, progress);
            },
        );
        // Released whatever happened, so a cancelled or failed scan never leaves
        // the slot busy.
        jobs.release(JobKind::Scan, &id);
        let summary = result?;
        // The counts moved, so every open panel is now stale (§10.5).
        let _ = app.emit(channels::DB_CHANGED, ());
        // §7.6's producer (c9 decision 1): the mop-up runs *after* the walk, on
        // its own thread, so the awaited ScanSummary resolves now and a full
        // 512-slot queue can never hold the scan hostage.
        mop_up_thumbs(
            app.clone(),
            jobs.clone(),
            snapshot.resolver.clone(),
            pool.clone(),
            ffmpeg.clone(),
        );
        Ok(summary)
    })
    .await
    .map_err(|error| AppError::Unavailable(format!("a thread de scan morreu: {error}")))?
}

/// §7.6's post-scan producer: every row, `force = false`, on a fresh thread.
///
/// The `JobKind::Thumbs` claim happens *inside* the thread: a rebuild already
/// generating owns the slot, and its force-queue covers every row anyway, so
/// yielding here loses nothing (§9.3, one Thumbs job — c9 decision 1).
fn mop_up_thumbs(
    app: AppHandle,
    jobs: Arc<JobRegistry>,
    resolver: PathResolver,
    pool: SqlitePool,
    ffmpeg: Option<PathBuf>,
) {
    std::thread::spawn(move || {
        let Ok(slot) = jobs.claim(JobKind::Thumbs) else {
            return;
        };
        let id = slot.id().to_string();
        let rows = match thumbs::all_jobs(&pool) {
            Ok(rows) => rows,
            Err(error) => {
                eprintln!("[wolfsmedia] thumb job list failed: {error}");
                jobs.release(JobKind::Thumbs, &id);
                return;
            }
        };
        let queue = thumbs::Queue::start(resolver, pool, ffmpeg, false, {
            let app = app.clone();
            move |event| {
                let _ = app.emit(channels::THUMB_PROGRESS, event);
            }
        });
        for row in rows {
            queue.enqueue(row);
        }
        let summary = queue.shutdown().join();
        let _ = app.emit(channels::THUMB_DONE, summary);
        jobs.release(JobKind::Thumbs, &id);
    });
}

/// §8.1 `thumbs_rebuild_all` — §7.6's `[[Rebuild all thumbs]]`: re-render every
/// row with `force = true`, as a background job. The claim happens on the
/// calling thread so a second click while one runs is `E_CONFLICT` (§9.3),
/// synchronously, on the promise; progress itself is the `wolfs://thumb/*`
/// event pair, and `()` resolving only means "the job started".
#[tauri::command]
pub fn thumbs_rebuild_all(app: AppHandle, state: State<'_, AppState>) -> Result<(), AppError> {
    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
    let jobs = state.jobs();
    let slot = jobs.claim(JobKind::Thumbs)?;
    let id = slot.id().to_string();
    let resolver = snapshot.resolver.clone();
    let ffmpeg = ffmpeg_path(&snapshot);

    tauri::async_runtime::spawn_blocking(move || {
        let rows = match thumbs::all_jobs(&pool) {
            Ok(rows) => rows,
            Err(error) => {
                eprintln!("[wolfsmedia] thumb job list failed: {error}");
                jobs.release(JobKind::Thumbs, &id);
                return;
            }
        };
        let queue = thumbs::Queue::start(resolver, pool, ffmpeg, true, {
            let app = app.clone();
            move |event| {
                let _ = app.emit(channels::THUMB_PROGRESS, event);
            }
        });
        for row in rows {
            queue.enqueue(row);
        }
        let summary = queue.shutdown().join();
        let _ = app.emit(channels::THUMB_DONE, summary);
        jobs.release(JobKind::Thumbs, &id);
    });
    Ok(())
}

/// §8.1 `scan_cancel` — §9.3. Idempotent: cancelling with nothing running is a
/// no-op, because the button can be pressed twice and the user must not see an
/// error for it.
#[tauri::command]
pub fn scan_cancel(state: State<'_, AppState>) -> Result<(), AppError> {
    state.jobs().cancel(JobKind::Scan);
    Ok(())
}

/// §8.1 `geo_lookup` — §7.5's reverse geocoder, exposed for debugging and tests
/// only (§8.1: "debug/testing only"), never for the gallery: the UI reads
/// `locations`, which the scan already filled.
///
/// Takes raw coordinates rather than a `media_id` on purpose: a probe that
/// resolved ids could not be used to check a point the app has not indexed yet.
#[tauri::command]
pub async fn geo_lookup(
    state: State<'_, AppState>,
    lat: f64,
    lon: f64,
) -> Result<Option<GeoDto>, AppError> {
    let snapshot = state.snapshot();
    let geocoder = Arc::clone(&snapshot.geocoder);
    tauri::async_runtime::spawn_blocking(move || geocoder.geocode(lat, lon))
        .await
        .map_err(|error| {
            AppError::Unavailable(format!("a thread de geocodificação morreu: {error}"))
        })
}

/// §8.1 `media_query` — one page of gallery results. The scope is always
/// explicit (§6.6); the frontend never chooses it implicitly.
#[tauri::command]
pub fn media_query(
    state: State<'_, AppState>,
    spec: FilterSpec,
) -> Result<crate::core::models::MediaQueryDto, AppError> {
    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
    Ok(db::media_query(&pool, &spec)?)
}

/// §8.1 `media_detail` — one media item for the lightbox.
#[tauri::command]
pub fn media_detail(
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<crate::core::models::MediaDto>, AppError> {
    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
    Ok(db::media_detail(&pool, &id)?)
}

/// §8.1 `media_reveal` — Explorer on the media's folder. The path is derived
/// from the resolver, never accepted from the frontend.
#[tauri::command]
pub fn media_reveal(state: State<'_, AppState>, id: String) -> Result<(), AppError> {
    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
    let media = db::media_detail(&pool, &id)?
        .ok_or_else(|| AppError::Conflict(format!("media não encontrado: {id}")))?;
    let abs = snapshot.resolver.rel_to_abs(&media.relative_path)?;
    let parent = abs
        .parent()
        .ok_or_else(|| AppError::Conflict("sem pasta pai".to_string()))?;
    super::settings::reveal(parent)
}

/// §8.1 `media_remove` — delete media rows by id. The frontend always confirms
/// first (C7/§14); this is the backend half.
#[tauri::command]
pub fn media_remove(state: State<'_, AppState>, ids: Vec<String>) -> Result<usize, AppError> {
    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
    db::media_remove(&pool, &ids).map_err(AppError::from)
}

/// `App/bin/ffmpeg.exe`, or `None` when it is not there. §3.3's finding on this
/// box: absent, so videos fall back to their mtime and say so — a `None` here is
/// a normal state, not an error.
fn ffmpeg_path(snapshot: &crate::state::Snapshot) -> Option<PathBuf> {
    let candidate = snapshot.resolver.ffmpeg_exe();
    candidate.is_file().then_some(candidate)
}
