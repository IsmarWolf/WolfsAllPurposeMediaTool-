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
use crate::core::models::{
    CityCountDto, FavoriteChangedDto, FilterSpec, GeoDto, MediaScope, ScanSummary, StatsDto,
};
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
pub(crate) fn mop_up_thumbs(
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

/// §8.1 `city_counts` — the L2 `Localização` tag-cloud (§10.6.1): city + count,
/// most populated first, scoped like every read so hidden rows only appear in
/// Vault Mode. The scope is a parameter (as in `media_query`), never implicit.
#[tauri::command]
pub fn city_counts(
    state: State<'_, AppState>,
    scope: MediaScope,
) -> Result<Vec<CityCountDto>, AppError> {
    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
    Ok(db::city_counts(&pool, scope)?)
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
///
/// It is an id fetch, not a scoped query (§6.6): the id can only come from a
/// scoped list the user already saw. c18 closes the edge anyway — a hidden item
/// resolves to `None` unless Vault Mode is unlocked — so a stale or hostile id
/// can never surface a vault file in the standard shell (C6).
#[tauri::command]
pub fn media_detail(
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<crate::core::models::MediaDto>, AppError> {
    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
    match db::media_detail(&pool, &id)? {
        Some(item) if item.is_hidden && !state.vault_unlocked() => Ok(None),
        other => Ok(other),
    }
}

/// C6 at the id edge (c18). `media_reveal`, `media_scope_video` and `media_remove`
/// take an id, not a `MediaScope`, so nothing in their SQL stops them naming a
/// hidden row; that would let the standard shell reveal, play or delete a vault
/// file. A hidden row is only reachable while Vault Mode is unlocked. `E_AUTH`
/// (not a silent no-op) so a buggy caller learns why.
fn require_visible_unless_unlocked(
    state: &State<'_, AppState>,
    pool: &SqlitePool,
    ids: &[String],
) -> Result<(), AppError> {
    if state.vault_unlocked() || !db::media_any_hidden(pool, ids)? {
        Ok(())
    } else {
        Err(AppError::from(crate::core::vault::VaultError::Locked))
    }
}

/// §8.1 `media_scope_video` — the lightbox's one-file scope grant (c13 fix,
/// human-approved 2026-10-05).
///
/// The asset protocol is scoped to `Thumbnails/` only (§6.7, c10 decision),
/// which can never serve a video's frames. When the lightbox opens a video this
/// one file under `Media/` is granted — a narrow amendment of C10's "never
/// `Media/`": no directory grant, and `.vault` stays out (C6). The path is
/// derived from the resolver, never accepted from the frontend; a missing row
/// or an image simply grants nothing. Additive and irreversible per boot (§14's
/// reset clears it), which a file just shown on screen does not merit worrying
/// about.
#[tauri::command]
pub fn media_scope_video(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), AppError> {
    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
    require_visible_unless_unlocked(&state, &pool, std::slice::from_ref(&id))?;
    let Some(media) = db::media_detail(&pool, &id)? else {
        return Ok(());
    };
    if media.file_type == "video" {
        let abs = snapshot.resolver.rel_to_abs(&media.relative_path)?;
        crate::core::thumbs::scope_video(&app, &abs);
    }
    Ok(())
}

/// §8.1 `media_reveal` — Explorer on the media's folder. The path is derived
/// from the resolver, never accepted from the frontend.
#[tauri::command]
pub fn media_reveal(state: State<'_, AppState>, id: String) -> Result<(), AppError> {
    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
    require_visible_unless_unlocked(&state, &pool, std::slice::from_ref(&id))?;
    let media = db::media_detail(&pool, &id)?
        .ok_or_else(|| AppError::Conflict(format!("media não encontrado: {id}")))?;
    let abs = snapshot.resolver.rel_to_abs(&media.relative_path)?;
    let parent = abs
        .parent()
        .ok_or_else(|| AppError::Conflict("sem pasta pai".to_string()))?;
    super::settings::reveal(parent)
}

/// §8.1 `media_remove` — delete media by id: **the file first, the row second**
/// (§14, human override 2026-10-20 — the c10 "rows only" rule is retired: an
/// "[Apagar]" that leaves the bytes on disk is what the rescan then happily
/// re-imports). The frontend always confirms first (C7); this is the backend
/// half. c18 refuses a hidden row unless Vault Mode is unlocked, so a stale
/// selection cannot orphan a vault file.
///
/// Returns how many rows were actually removed — a file that refuses to die
/// (locked by another program) keeps its row, and the count tells the UI the
/// difference (`media.deletePartial`). Emits `wolfs://db-changed` on success so
/// every open grid and the Dashboard refetch (§10.5), like `media_set_favorite`.
#[tauri::command]
pub fn media_remove(
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> Result<usize, AppError> {
    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
    require_visible_unless_unlocked(&state, &pool, &ids)?;
    let removed = remove_media_files(&snapshot.resolver, &pool, &ids)?;
    if removed > 0 {
        let _ = app.emit(channels::DB_CHANGED, ());
    }
    Ok(removed)
}

/// The file half of §8.1 `media_remove` (§14, c22), split from the command so
/// the fs/DB interplay is unit-testable without an `AppHandle`.
///
/// Invariant: **the row goes only when its file is gone** (or was never there).
/// A locked file leaves its row behind — a gallery tile without its bytes is
/// bad, a rescan resurrecting a file the human deleted is worse (§14). The
/// thumbnails (both slots, §6.7) are best-effort: their failure never keeps the
/// media row alive, it only costs a stale thumb file.
fn remove_media_files(
    resolver: &PathResolver,
    pool: &SqlitePool,
    ids: &[String],
) -> Result<usize, AppError> {
    let rows = db::media_paths_by_id(pool, ids)?;
    let mut doomed: Vec<String> = Vec::with_capacity(rows.len());
    for row in rows {
        let removed = match resolver.rel_to_abs(&row.relative_path) {
            Ok(abs) if abs.exists() => std::fs::remove_file(&abs).is_ok(),
            // Absent already: a dead row a rescan would drop anyway — deleting
            // it is honest, and nothing on disk can resurrect it.
            Ok(_) => true,
            // A path that does not resolve to this volume: keep the row rather
            // than delete data we cannot even name.
            Err(_) => false,
        };
        if !removed {
            continue;
        }
        if let Ok(thumb) = resolver.rel_to_abs(&row.thumb_path) {
            let _ = std::fs::remove_file(thumb);
        }
        // The 200px slot of the same file — `None` for a vault thumb (no /400/
        // in `.vault/thumbs/...`, §12.5.2), which never had a 200 twin.
        if let Some(small) = thumbs::thumb_200_of(&row.thumb_path)
            && let Ok(small_abs) = resolver.rel_to_abs(&small)
        {
            let _ = std::fs::remove_file(small_abs);
        }
        doomed.push(row.id);
    }
    if doomed.is_empty() {
        return Ok(0);
    }
    Ok(db::media_remove(pool, &doomed)?)
}

/// §8.1 `media_set_favorite` (Phase 6, §24.2) — set the ♡ flag on a batch.
///
/// Returns how many rows **actually flipped**, and emits `wolfs://db-changed` so
/// the Favoritos collection and any open grid refetch. Like every id-based media
/// command (c18), it refuses a hidden row unless Vault Mode is unlocked: the flag
/// is a write into a row the standard shell cannot see (C6 at the id edge).
#[tauri::command]
pub fn media_set_favorite(
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<String>,
    on: bool,
) -> Result<FavoriteChangedDto, AppError> {
    let snapshot = state.snapshot();
    let pool = snapshot.require_pool()?;
    require_visible_unless_unlocked(&state, &pool, &ids)?;
    let changed = db::media_set_favorite(&pool, &ids, on)?;
    if changed > 0 {
        let _ = app.emit(channels::DB_CHANGED, ());
    }
    Ok(FavoriteChangedDto {
        changed: changed as i64,
    })
}

/// `App/bin/ffmpeg.exe`, or `None` when it is not there. §3.3's finding on this
/// box: absent, so videos fall back to their mtime and say so — a `None` here is
/// a normal state, not an error.
fn ffmpeg_path(snapshot: &crate::state::Snapshot) -> Option<PathBuf> {
    let candidate = snapshot.resolver.ffmpeg_exe();
    candidate.is_file().then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::paths::RootSource;
    use std::path::Path;

    /// The real boot order of §5.5: the tree first, then the database.
    fn booted(root: &Path) -> PathResolver {
        let resolver = PathResolver::new(root.to_path_buf(), RootSource::MarkerWalk);
        crate::core::layout::ensure(&resolver).unwrap();
        resolver
    }

    fn open_pool(root: &Path) -> SqlitePool {
        db::open(&booted(root)).unwrap()
    }

    /// One visible image row — every column the inserts elsewhere also spell out,
    /// so the fixture never leans on a column default silently changing.
    fn insert(pool: &SqlitePool, id: &str, rel: &str, thumb: &str) {
        pool.get()
            .unwrap()
            .execute(
                "INSERT INTO media (id, relative_path, thumb_path, file_hash, file_size, file_type,
                                    captured_at, added_at, has_metadata, is_hidden, device_name)
                 VALUES (?1, ?2, ?3, ?4, 1000, 'image', '2024-03-15 10:00:00',
                         '2026-06-01T09:00:00', 1, 0, 'PC')",
                rusqlite::params![id, rel, thumb, format!("hash-{id}")],
            )
            .unwrap();
    }

    fn write(root: &Path, rel: &str) {
        let abs = root.join(rel);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(abs, b"bytes").unwrap();
    }

    fn row_count(pool: &SqlitePool, id: &str) -> i64 {
        pool.get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM media WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .unwrap()
    }

    /// §14/c22 — the row, the file and both thumbnail slots go together, file
    /// first. This is the invariant the whole change exists for: before it,
    /// `[Apagar]` only unlinked the row and the next scan re-imported the very
    /// same bytes.
    #[test]
    fn deleting_a_row_removes_its_file_and_both_thumbnail_slots() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = booted(dir.path());
        let pool = open_pool(dir.path());

        let (rel, thumb_400) = ("Media/PC/2024/03/IMG_1.jpg", "Thumbnails/400/gone.webp");
        let thumb_200 = thumb_400.replacen("/400/", "/200/", 1);
        insert(&pool, "a1", rel, thumb_400);
        write(dir.path(), rel);
        write(dir.path(), thumb_400);
        write(dir.path(), &thumb_200);

        let removed = remove_media_files(&resolver, &pool, &["a1".into()]).unwrap();
        assert_eq!(removed, 1);
        assert!(
            !dir.path().join(rel).exists(),
            "the media file itself is gone"
        );
        assert!(!dir.path().join(thumb_400).exists());
        assert!(
            !dir.path().join(thumb_200).exists(),
            "both slots, one file (§6.7)"
        );
        assert_eq!(row_count(&pool, "a1"), 0, "row last, after the bytes");
    }

    /// A row whose file was already lost (an interrupted move, a human with an
    /// external editor) is a dead row: it must drop, or `[Apagar]` would report
    /// a failure for a file that is not there. Nothing on disk resurrects it.
    #[test]
    fn a_row_whose_file_is_already_absent_still_drops() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = booted(dir.path());
        let pool = open_pool(dir.path());
        insert(
            &pool,
            "a2",
            "Media/PC/2024/03/never_existed.jpg",
            "Thumbnails/400/a2.webp",
        );

        let removed = remove_media_files(&resolver, &pool, &["a2".into()]).unwrap();
        assert_eq!(removed, 1);
        assert_eq!(row_count(&pool, "a2"), 0);
    }

    /// A file that refuses to go keeps its row: the gallery may show a tile
    /// whose bytes are unremovable, but the inverse would let a scan resurrect
    /// a file the human just deleted (§14). The refusal is real, not simulated:
    /// the handle below is opened **without** `FILE_SHARE_DELETE`, which is
    /// exactly the state a non-Rust program (or an AV scanner) leaves a file in
    /// — and `DeleteFile` then fails with a sharing violation, as it should.
    /// (A read-only attribute would *not* do: Rust's own `remove_file` clears
    /// it first, which this test also proved before the share mode replaced it.)
    #[test]
    fn a_file_that_refuses_to_go_keeps_its_row() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = tempfile::tempdir().unwrap();
        let resolver = booted(dir.path());
        let pool = open_pool(dir.path());

        let rel = "Media/PC/2024/03/locked.jpg";
        insert(&pool, "a3", rel, "Thumbnails/400/a3.webp");
        write(dir.path(), rel);
        let abs = dir.path().join(rel);
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1) // FILE_SHARE_READ — no FILE_SHARE_DELETE
            .open(&abs)
            .unwrap();

        let removed = remove_media_files(&resolver, &pool, &["a3".into()]).unwrap();
        assert_eq!(removed, 0, "undeletable file, no row dropped");
        assert!(abs.exists());
        assert_eq!(row_count(&pool, "a3"), 1, "the row still names the file");

        drop(held);
        let removed = remove_media_files(&resolver, &pool, &["a3".into()]).unwrap();
        assert_eq!(removed, 1, "the moment the file can go, so does the row");
        assert_eq!(row_count(&pool, "a3"), 0);
    }

    /// The Cofre's own `[C-6i Apagar def.]` runs through this same command, so
    /// a hidden row deletes from the vault mirror (`.vault/items/...`) — whose
    /// thumb (`.vault/thumbs/...`) never had a 200 twin to chase (§12.5.2).
    #[test]
    fn a_vault_row_deletes_from_the_mirror_without_a_200_slot() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = booted(dir.path());
        let pool = open_pool(dir.path());

        let rel = ".vault/items/2024/03/secret.jpg";
        let thumb = ".vault/thumbs/h1.webp";
        insert(&pool, "v1", rel, thumb);
        write(dir.path(), rel);
        write(dir.path(), thumb);

        let removed = remove_media_files(&resolver, &pool, &["v1".into()]).unwrap();
        assert_eq!(removed, 1);
        assert!(!dir.path().join(rel).exists());
        assert!(!dir.path().join(thumb).exists());
        assert_eq!(row_count(&pool, "v1"), 0);
    }

    /// Ids that match no row are absent, not an error; an empty batch is a no-op.
    #[test]
    fn unknown_ids_are_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = booted(dir.path());
        let pool = open_pool(dir.path());
        assert_eq!(
            remove_media_files(&resolver, &pool, &["ghost".into()]).unwrap(),
            0
        );
        assert_eq!(remove_media_files(&resolver, &pool, &[]).unwrap(), 0);
    }
}
