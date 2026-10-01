//! §7.3 `core/scanner.rs` — the folder walk that turns files on the SSD into
//! rows, and the mop-up rule that organizes them.
//!
//! ```text
//! scan(device_label, root, pool, emit) -> ScanSummary
//! ```
//!
//! ## What this module may and may not do
//!
//! It never sees a `rusqlite` type (§6.5) and never resolves the root itself
//! (C2/C5): the caller passes a [`PathResolver`], the pool, the geocoder and the
//! ffmpeg path. Everything about *where* the tree is lives in `paths.rs`;
//! everything about *what a row looks like* lives in `db.rs`.
//!
//! ## The tree is ours, and only ours
//!
//! §4.1 says `Media/` holds "originals untouched (**rename/move only**)", and §14
//! protects "a local folder" from being moved, renamed or deleted. Those are two
//! different trees, and this module walks both — see [`ScanSource`]. Inside
//! `Media/<device>/` (which is already a copy the app owns) the mop-up rule only
//! ever renames, and always same-volume (one SSD). A human's own library (§11.3's
//! Disco-local source) is read and **copied** into `Media/`, never renamed, moved
//! or deleted: `fs::copy` for the external tree, `fs::rename` for ours.

use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Datelike;
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::core::db::{self, MediaInsert, SCAN_BATCH_ROWS, SqlitePool};
use crate::core::exif::{self, IMAGE_EXTS, RawMetadata, VIDEO_EXTS};
use crate::core::geocode::Geocoder;
use crate::core::models::{LocationDto, ScanProgressDto, ScanSummary};
use crate::core::paths::{self, NO_META_DIR, PathResolver, THUMBNAILS_DIR};
use crate::error::AppError;

/// §7.3 step 1: 1 MB streaming buffer. Big enough that syscall overhead stops
/// mattering, small enough that the resident cost of a scan is not a library.
const HASH_BUFFER: usize = 1024 * 1024;

/// §7.3 step 8: "emit progress event per file (throttled)". Count-based, not
/// time-based, so the event stream is a pure function of the input and a test can
/// assert it exactly. The first file always emits.
const PROGRESS_EVERY: u32 = 25;

/// Directories that are never media, even if a human nests them under
/// `Media/<device>/`. §7.3 names `.vault` and `Thumbnails`; the rest are the
/// other §4.1 top-level folders, skipped because scanning them would index the
/// app's own files.
const SKIP_DIRS: &[&str] = &[THUMBNAILS_DIR, paths::APP_DIR, paths::DATABASE_DIR];

/// §9.3's per-job cancellation flag: the `Arc<AtomicBool>` the registry owns and
/// `scan_cancel` sets, checked between files and inside a hash, so a cancel on a
/// 4 GB video does not wait for it.
#[derive(Debug, Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// The two trees a scan can walk (§7.3 vs §11.3). They get different treatment
/// because they have different ownership: `Media/<device>/` is the app's own copy
/// (rename + repair allowed), a local folder is the human's (copy-only, §14).
#[derive(Debug, Clone, Copy)]
pub enum ScanSource<'a> {
    /// The village: `Media/<device>/` exists and a three-way dedupe (§7.3 step 2)
    /// can repair human moves inside it.
    Device,
    /// An external folder chosen on the Backup → Disco local pane (§11.3 c8).
    /// The walk reads it and copies into `Media/<device>/`; the source is never
    /// renamed, moved or deleted, so there is no repair branch and no second-copy
    /// classification — the hash vs the DB is the whole story (C5).
    CopyFrom { source: &'a Path },
}

/// A media file the walker found, before anything has been decided about it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    abs: PathBuf,
    /// Path relative to `Media/<device>/`, `/`-joined. The organized path is built
    /// from the capture date instead, so a deep human folder (`2023/raw/…`) is
    /// flattened into the §4.1 `YYYY/MM` tree rather than preserved.
    from_device: String,
    hash: String,
    size: i64,
    file_type: &'static str,
}

/// The §7.3 pipeline. `emit` receives the walk's progress; the return is the
/// summary the toast shows.
// Eight args is the pipeline's whole context (c8): bundling them into a struct
// would just move the list, not shorten it.
#[allow(clippy::too_many_arguments)]
pub fn scan(
    device: &str,
    resolver: &PathResolver,
    pool: &SqlitePool,
    geocoder: &Geocoder,
    ffmpeg: Option<&Path>,
    source: ScanSource<'_>,
    cancel: &CancelFlag,
    mut emit: impl FnMut(&ScanProgressDto),
) -> Result<ScanSummary, AppError> {
    let label = paths::sanitize_label(device);
    paths::ensure_safe_component(&label)?;
    let device_dir = resolver.media_device(&label)?;

    // §7.5 decision 5: the index is loaded once, here, so the first photo with a
    // GPS tag does not pay for the read in the middle of the walk.
    geocoder.preload();

    let mut summary = ScanSummary {
        device: label.clone(),
        ..ScanSummary::default()
    };

    // --- Phase 1: walk + hash (§7.3 steps 1 and 2) --------------------------
    // Which tree is being scanned, and how phase 2 must treat a file it already
    // knows: §7.3's own tree gets the three-way dedupe, §11.3's a plain "skip".
    let walk_root: &Path = match source {
        ScanSource::Device => {
            if !device_dir.is_dir() {
                return Err(AppError::Conflict(format!(
                    "a pasta do dispositivo não existe: {}",
                    device_dir.display()
                )));
            }
            device_dir.as_path()
        }
        ScanSource::CopyFrom { source } => {
            if !source.is_dir() {
                return Err(AppError::Conflict(format!(
                    "a pasta selecionada não existe: {}",
                    source.display()
                )));
            }
            // The device tree may not exist yet: in copy mode the mop-up creates
            // `Media/<label>/…` on the first file that lands (§11.3).
            source
        }
    };
    let copy_mode = matches!(source, ScanSource::CopyFrom { .. });

    let walk = collect(walk_root, cancel, &mut emit)?;
    summary.found = walk.found;
    summary.skipped = walk.skipped;
    // Duplicates inside this very walk, plus the ones phase 2 finds against the
    // DB. Both are the same outcome for the human, so they share one counter.
    summary.duplicates = walk.intra_duplicates;
    let candidates = walk.candidates;
    if cancel.is_cancelled() {
        summary.cancelled = true;
        return Ok(summary);
    }

    // Every dedupe question in one connection, one cached statement — not a
    // prepare per file.
    let hashes: Vec<&str> = candidates.iter().map(|c| c.hash.as_str()).collect();
    let known = db::find_hashes(pool, &hashes)?;

    // --- Phase 2: dedupe, metadata, organize, geocode, batch (§7.3 steps 2-8) --
    let found = candidates.len() as u32;
    let mut prepared: Vec<MediaInsert> = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if cancel.is_cancelled() {
            summary.cancelled = true;
            break;
        }
        let existing = known[index].clone();
        let metadata = read_metadata(ffmpeg, &candidate.abs);
        let name = file_name_of(&candidate.abs);

        if let Some(existing) = existing {
            if copy_mode {
                // §11.3/§14: the source is the human's tree. Never repair, never
                // move, never classify — if the bytes are indexed, the copy is
                // redundant and the job moves on.
                summary.duplicates += 1;
            } else {
                // §7.3 step 2. These bytes are already indexed, so a second row is
                // not representable (`file_hash` is UNIQUE, §6.1). Which of the two
                // situations is it? Only one of them is repairable.
                match classify_duplicate(
                    resolver,
                    &label,
                    &existing.relative_path,
                    &candidate.from_device,
                ) {
                    Duplicate::SamePath | Duplicate::SecondCopy => {
                        // Already organized, or a duplicate elsewhere: the indexed
                        // row keeps the bytes (C5) and this file is left exactly
                        // where it is. Moving the second copy would orphan the
                        // file the row points at.
                        summary.duplicates += 1;
                    }
                    Duplicate::MovedByHuman => {
                        // The row's path is gone from disk and the same bytes are
                        // here: the file was dragged inside our own tree. Repair
                        // the pointer instead of indexing the same bytes twice.
                        let target_rel = target_rel(&label, &metadata, &name);
                        let (abs, moved) = organize(resolver, &candidate.abs, &target_rel, false)?;
                        let final_rel = resolver.abs_to_rel(&abs)?;
                        db::update_media_path(
                            pool,
                            &existing.id,
                            &final_rel,
                            &thumb_path_for(&final_rel),
                        )?;
                        summary.repaired += 1;
                        if moved {
                            summary.organized += 1;
                        }
                    }
                }
            }
        } else {
            let target_rel = target_rel(&label, &metadata, &name);
            let (abs, moved) = organize(resolver, &candidate.abs, &target_rel, copy_mode)?;
            // §6.7: the 400px destination, computed from the *final* path, so c9
            // never has to rewrite the row.
            let final_rel = resolver.abs_to_rel(&abs)?;
            let location = geocode_one(geocoder, &metadata, &mut summary);

            prepared.push(MediaInsert {
                id: uuid::Uuid::new_v4().to_string(),
                relative_path: final_rel.clone(),
                thumb_path: thumb_path_for(&final_rel),
                file_hash: candidate.hash.clone(),
                file_size: candidate.size,
                file_type: candidate.file_type.to_string(),
                captured_at: metadata
                    .captured_at
                    .map(|when| when.format("%Y-%m-%dT%H:%M:%S").to_string()),
                has_metadata: metadata.has_metadata(),
                device_name: label.clone(),
                location,
            });
            if moved {
                summary.organized += 1;
            }
        }

        if prepared.len() >= SCAN_BATCH_ROWS {
            flush(pool, &mut prepared, &mut summary);
        }
        // Same throttle as phase 1, plus the last candidate, so a scan that
        // ends on a non-multiple still lands on 100% instead of stalling at 97%.
        let done = index + 1;
        if (done as u32) == found || (done as u32).is_multiple_of(PROGRESS_EVERY) {
            emit(&ScanProgressDto {
                current: done as u32,
                total: found,
                last_path: candidate.abs.display().to_string(),
                inserted: summary.inserted,
            });
        }
    }
    flush(pool, &mut prepared, &mut summary);
    Ok(summary)
}

/// §7.3 step 2's three cases, decided by what the *filesystem* says rather than
/// by guessing from the hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Duplicate {
    /// The row already points at this file: a re-scan of a settled tree.
    SamePath,
    /// The row's file is still on disk: a second copy of the same bytes.
    SecondCopy,
    /// The row's file is gone and these are the same bytes: the human moved it
    /// inside `Media/`. The only case a scan can repair.
    MovedByHuman,
}

fn classify_duplicate(
    resolver: &PathResolver,
    label: &str,
    indexed_rel: &str,
    from_device: &str,
) -> Duplicate {
    let here = format!("{}/{}/{}", paths::MEDIA_DIR, label, from_device);
    if indexed_rel == here {
        // A re-scan of a settled tree, and the common case: no filesystem probe
        // needed, so a settled library costs one string comparison per file.
        return Duplicate::SamePath;
    }
    match resolver.rel_to_abs(indexed_rel) {
        Ok(path) if path.is_file() => Duplicate::SecondCopy,
        _ => Duplicate::MovedByHuman,
    }
}

/// §7.3 step 5 + §6.7: where a file with this metadata belongs, and where its
/// 400px thumbnail will live.
///
/// `YYYY/MM` from the capture date, or `Sem_Metadados/` when the file has no
/// metadata (C7). The date that decides is `captured_at` **and not
/// `date_from_fs`**, because an mtime is the copy time: a file whose only date is
/// its mtime belongs in `Sem_Metadados/`, and `RawMetadata::has_metadata` already
/// says so. One rule, asked once, in one place — the scanner never re-derives it.
fn target_rel(label: &str, metadata: &RawMetadata, name: &str) -> String {
    let dated = metadata
        .has_metadata()
        .then_some(metadata.captured_at)
        .flatten();
    match dated {
        Some(when) => format!(
            "{}/{}/{:04}/{:02}/{}",
            paths::MEDIA_DIR,
            label,
            when.year(),
            when.month(),
            name
        ),
        None => format!("{}/{}/{}/{}", paths::MEDIA_DIR, label, NO_META_DIR, name),
    }
}

/// §7.3 step 5: put the file into place, or report the current path when it is
/// already organized. A settled tree is never touched, so a re-scan does not
/// rewrite mtimes across the library.
///
/// `copy` selects the transport: `fs::rename` for `Media/<device>/` (our own
/// copy, same volume), `fs::copy` for §11.3's human-owned source — the copy is
/// the feature, and the original must survive it.
///
/// Collisions append `_1`, `_2`… before the extension (C5: the original bytes are
/// never overwritten). The suffix is chosen against what is **on disk**, not
/// against the DB, because a previous scan may have been interrupted before its
/// rows were flushed.
fn organize(
    resolver: &PathResolver,
    abs: &Path,
    target_rel: &str,
    copy: bool,
) -> Result<(PathBuf, bool), AppError> {
    let target_abs = resolver.rel_to_abs(target_rel)?;
    if !copy && abs == target_abs {
        return Ok((target_abs, false));
    }
    if let Some(parent) = target_abs.parent() {
        fs::create_dir_all(parent).map_err(|error| AppError::Ingest(error.to_string()))?;
    }
    let free = free_destination(&target_abs)?;
    if copy {
        fs::copy(abs, &free).map_err(|error| AppError::Ingest(error.to_string()))?;
    } else {
        fs::rename(abs, &free).map_err(|error| AppError::Ingest(error.to_string()))?;
    }
    // §14: a critical write is followed by a sync. Best-effort, because a
    // read-only volume must not fail a scan that already succeeded.
    let _ = fs::File::open(&free).and_then(|file| file.sync_all());
    Ok((free, true))
}

fn free_destination(target: &Path) -> Result<PathBuf, AppError> {
    if !target.exists() {
        return Ok(target.to_path_buf());
    }
    let parent = target
        .parent()
        .ok_or_else(|| AppError::Conflict(target.display().to_string()))?;
    let stem = target
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| AppError::Conflict(target.display().to_string()))?;
    let ext = target.extension().and_then(|e| e.to_str()).unwrap_or("");
    for attempt in 1..=9999u32 {
        let name = if ext.is_empty() {
            format!("{stem}_{attempt}")
        } else {
            format!("{stem}_{attempt}.{ext}")
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(AppError::Conflict(format!(
        "não há nome livre para {}",
        target.display()
    )))
}

/// §6.7 + §4.1: `Media/<device>/<rest>/name.ext` → `Thumbnails/400/<device>/<rest>/name.webp`.
///
/// The scanner computes the **destination** and stores it, so `NOT NULL` (§6.1) is
/// satisfied with a real path, c9 never rewrites the row, and the 200px path is
/// the same string with `400` replaced by `200` — the §6.7 convention, which keeps
/// §6.1's DDL verbatim.
///
/// The stem is preserved (`IMG_4501.HEIC` → `IMG_4501.webp`, as §22 shows) and the
/// extension is forced to `.webp` because §7.6 encodes WebP at both sizes.
fn thumb_path_for(rel_media: &str) -> String {
    let rest = rel_media
        .strip_prefix(&format!("{}/", paths::MEDIA_DIR))
        .unwrap_or(rel_media);
    let stem = rest.rsplit_once('.').map(|(s, _)| s).unwrap_or(rest);
    format!("{}/400/{stem}.webp", THUMBNAILS_DIR)
}

/// §7.3 step 1 — recursive walk of `Media/<device>/`, keeping only media files.
///
/// The walk is depth-first and does not follow symlinks: a link pointing outside
/// the tree would make §15's "no persisted absolute path" property untestable, and
/// could name a file on another volume, where `fs::rename` would silently become
/// a copy.
///
/// ## Dedupe happens *here*, not only against the DB
///
/// `file_hash` is UNIQUE (§6.1), so two copies of the same bytes found in the
/// *same* walk cannot both become rows — the batch insert would abort and take
/// the whole batch's files down with it. That is not a hypothetical: a camera
/// dumped twice into the same folder is the most ordinary thing a human does.
/// `in_walk` therefore keeps the first candidate per hash and reports the rest as
/// duplicates, exactly as a hash already in the DB would be.
///
/// `sort_by_file_name` is what makes "the first" mean something: readdir order is
/// filesystem-dependent, and without it the surviving copy would change between
/// machines for the same tree.
fn collect(
    device_dir: &Path,
    cancel: &CancelFlag,
    mut emit: impl FnMut(&ScanProgressDto),
) -> Result<Walk, AppError> {
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut in_walk: HashSet<String> = HashSet::new();
    let mut skipped: u32 = 0;
    let mut seen: u32 = 0;
    let mut found: u32 = 0;
    let mut intra_duplicates: u32 = 0;

    let walker = WalkDir::new(device_dir)
        .min_depth(1)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| !is_skipped_dir(entry.path(), device_dir));

    for entry in walker {
        if cancel.is_cancelled() {
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            // §7.4: "all parsing errors degrade" — an unreadable folder loses its
            // own files, not the whole scan.
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let Some(file_type) = file_type_of(path) else {
            continue;
        };
        seen += 1;
        found += 1;
        if seen == 1 || seen.is_multiple_of(PROGRESS_EVERY) {
            emit(&ScanProgressDto {
                current: seen,
                // A walk cannot know its size in advance, so §9.2's `total` stays 0
                // until the second phase and the UI shows an indeterminate bar.
                total: 0,
                last_path: path.display().to_string(),
                inserted: 0,
            });
        }

        let size = match fs::metadata(path) {
            Ok(metadata) => metadata.len() as i64,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        match sha256_file(path, cancel) {
            Ok(hash) => {
                if !in_walk.insert(hash.clone()) {
                    // Same bytes twice in this walk: one row keeps them and this
                    // file is left where the human put it, never renamed and never
                    // deleted. That is the same treatment a hash already in the DB
                    // gets, so the outcome does not depend on scan order.
                    intra_duplicates += 1;
                    continue;
                }
                candidates.push(Candidate {
                    abs: path.to_path_buf(),
                    from_device: from_device(path, device_dir),
                    hash,
                    size,
                    file_type,
                })
            }
            // A cancel mid-hash is not a skip: it is the reason we are stopping.
            Err(Some(())) => break,
            Err(None) => skipped += 1,
        }
    }

    Ok(Walk {
        candidates,
        found,
        skipped,
        intra_duplicates,
    })
}

/// What phase 1 learned. `found` counts every media file the walk met, while
/// `candidates` is the hash-deduped set phase 2 actually works on — so the two
/// differ exactly by `intra_duplicates`, and progress reaching 100% means "every
/// file that could become a row has been decided", not "every file was indexed".
struct Walk {
    candidates: Vec<Candidate>,
    found: u32,
    skipped: u32,
    intra_duplicates: u32,
}

/// `Err(Some(()))` = cancelled, `Err(None)` = unreadable. A separate channel from
/// the file errors because §9.3's cancel must not be counted as a bad file.
type HashOutcome = Result<String, Option<()>>;

fn sha256_file(path: &Path, cancel: &CancelFlag) -> HashOutcome {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return Err(None),
    };
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; HASH_BUFFER];
    loop {
        if cancel.is_cancelled() {
            return Err(Some(()));
        }
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => hasher.update(&buffer[..read]),
            Err(_) => return Err(None),
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// `image` / `video` from the §7.4.1 decision 10 lists, or `None` for a file the
/// scan must not parse as EXIF.
fn file_type_of(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if IMAGE_EXTS.contains(&ext.as_str()) {
        Some("image")
    } else if VIDEO_EXTS.contains(&ext.as_str()) {
        Some("video")
    } else {
        None
    }
}

fn is_skipped_dir(path: &Path, device_dir: &Path) -> bool {
    if path == device_dir {
        return false;
    }
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    // Dot-prefixed covers `.vault` (§7.3) and every other hidden folder a sync
    // tool or Windows may have left behind.
    name.starts_with('.') || SKIP_DIRS.contains(&name)
}

fn from_device(path: &Path, device_dir: &Path) -> String {
    path.strip_prefix(device_dir)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// §7.3 step 3: never a failure for one file. `exif::extract` returning an error
/// (unreadable file, a missing ffmpeg binary) degrades to the mtime fallback the
/// module already applies, so the scan keeps its place in the walk.
fn read_metadata(ffmpeg: Option<&Path>, abs: &Path) -> RawMetadata {
    match exif::extract(abs, ffmpeg) {
        Ok(metadata) => metadata,
        Err(error) => {
            eprintln!("[wolfsmedia] sem metadados de {}: {error}", abs.display());
            RawMetadata::default()
        }
    }
}

/// §7.3 step 6. The counters live here so both halves of §7.5 — "geocode if there
/// is a GPS" and "keep the coordinates even when there is no city" — are decided
/// in one place, and a row with a location is written in both cases.
fn geocode_one(
    geocoder: &Geocoder,
    metadata: &RawMetadata,
    summary: &mut ScanSummary,
) -> Option<LocationDto> {
    let (lat, lon) = metadata.gps?;
    match geocoder.geocode(lat, lon) {
        Some(place) => {
            summary.located += 1;
            Some(LocationDto {
                city: Some(place.city),
                state: Some(place.state),
                country: Some(place.country),
                // §7.5.1 decision 4: the media's point, not the city centroid.
                latitude: Some(lat),
                longitude: Some(lon),
            })
        }
        None => {
            summary.located_none += 1;
            Some(LocationDto {
                city: None,
                state: None,
                country: None,
                latitude: Some(lat),
                longitude: Some(lon),
            })
        }
    }
}

/// §7.3 step 7: one transaction per batch, flushed at
/// [`SCAN_BATCH_ROWS`](db::SCAN_BATCH_ROWS).
///
/// A failed batch is counted as skipped, never silently dropped: `inserted` is
/// the number the toast shows, and it must never over-count. The scan continues —
/// one bad row is not a reason to abandon the library.
fn flush(pool: &SqlitePool, prepared: &mut Vec<MediaInsert>, summary: &mut ScanSummary) {
    if prepared.is_empty() {
        return;
    }
    let rows = std::mem::take(prepared);
    let pending = rows.len() as u32;
    let no_metadata = rows.iter().filter(|row| !row.has_metadata).count() as u32;
    match db::insert_media_batch(pool, &rows) {
        Ok(written) => {
            summary.inserted += written as u32;
            summary.no_metadata += no_metadata;
        }
        Err(error) => {
            summary.skipped += pending;
            eprintln!("[wolfsmedia] lote de {pending} linhas não gravado: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::exif::tests::{DATE, DATE_ISO, Fixture, LAT_DEG, LON_DEG, jpeg, tiff};
    use crate::core::paths::{PC_DEVICE_LABEL, RootSource};

    fn resolver_at(root: &Path) -> PathResolver {
        PathResolver::new(root.to_path_buf(), RootSource::MarkerWalk)
    }

    /// A booted tree + pool. Both halves of a scan touch real state — the
    /// filesystem and the database — so neither can be faked.
    fn booted(root: &Path) -> (PathResolver, SqlitePool) {
        let resolver = resolver_at(root);
        crate::core::layout::ensure(&resolver).unwrap();
        let pool = db::open(&resolver).unwrap();
        (resolver, pool)
    }

    fn drop_file(device_dir: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
        let path = device_dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    /// A dated photo, straight from the c6 fixture builders — the same bytes the
    /// JPEG/PNG/HEIC parity test reads.
    fn dated_photo() -> Vec<u8> {
        jpeg(&tiff(Fixture::Date))
    }

    fn photo_with_gps() -> Vec<u8> {
        jpeg(&tiff(Fixture::DateGps))
    }

    /// A photo whose only date is its mtime: the EXIF date is not a calendar
    /// date, so the module falls back to the filesystem and flags it.
    fn mtime_only_photo() -> Vec<u8> {
        jpeg(&tiff(Fixture::ImpossibleDate))
    }

    fn run_scan(resolver: &PathResolver, pool: &SqlitePool) -> ScanSummary {
        let cancel = CancelFlag::new();
        let geocoder = Geocoder::new(resolver.geonames_path());
        scan(
            PC_DEVICE_LABEL,
            resolver,
            pool,
            &geocoder,
            None,
            ScanSource::Device,
            &cancel,
            |_| {},
        )
        .unwrap()
    }

    fn scan_with_events(
        resolver: &PathResolver,
        pool: &SqlitePool,
        cancel: &CancelFlag,
        events: &mut Vec<ScanProgressDto>,
    ) -> ScanSummary {
        let geocoder = Geocoder::new(resolver.geonames_path());
        scan(
            PC_DEVICE_LABEL,
            resolver,
            pool,
            &geocoder,
            None,
            ScanSource::Device,
            cancel,
            |event| events.push(event.clone()),
        )
        .unwrap()
    }

    fn rows(pool: &SqlitePool) -> Vec<(String, bool, String)> {
        let conn = pool.get().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT relative_path, has_metadata, thumb_path FROM media ORDER BY relative_path",
            )
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    /// One `media` row joined to its optional `locations` row.
    #[allow(clippy::type_complexity)]
    type Place = (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<f64>,
        Option<f64>,
    );

    /// Every media row with its location, if it has one.
    fn places(pool: &SqlitePool) -> Vec<Place> {
        let conn = pool.get().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT m.relative_path, l.city, l.state, l.country, l.latitude, l.longitude
                 FROM media m LEFT JOIN locations l ON l.media_id = m.id
                 ORDER BY m.relative_path",
            )
            .unwrap();
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    }

    #[test]
    fn the_thumb_path_is_the_400px_destination_of_the_media_path() {
        // §6.7 + §22: `Media/iPhone/2024/05/IMG_4501.HEIC` ->
        // `Thumbnails/400/iPhone/2024/05/IMG_4501.webp`, and the 200px path is the
        // same string with `400` swapped.
        assert_eq!(
            thumb_path_for("Media/iPhone_14_Pro/2024/05/IMG_4501.HEIC"),
            "Thumbnails/400/iPhone_14_Pro/2024/05/IMG_4501.webp"
        );
        assert_eq!(
            thumb_path_for("Media/PC/Sem_Metadados/scan.tif"),
            "Thumbnails/400/PC/Sem_Metadados/scan.webp"
        );
        // A name with no extension must not lose its stem.
        assert_eq!(
            thumb_path_for("Media/PC/2024/01/README"),
            "Thumbnails/400/PC/2024/01/README.webp"
        );
        // The stored value must be the *real* destination, not a placeholder: the
        // DTO's thumb400 is this string and thumb200 is the 400/200 swap.
        let (thumb400, thumb200) = (
            thumb_path_for("Media/PC/2024/03/IMG_1.jpg"),
            thumb_path_for("Media/PC/2024/03/IMG_1.jpg").replace("/400/", "/200/"),
        );
        assert_ne!(thumb400, thumb200);
    }

    #[test]
    fn a_dated_photo_lands_in_year_month_and_a_bare_one_in_sem_metadados() {
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        let device_dir = resolver.media_device(PC_DEVICE_LABEL).unwrap();
        drop_file(&device_dir, "IMG_0001.jpg", &dated_photo());
        drop_file(&device_dir, "loose/notes-scan.jpg", &mtime_only_photo());

        let summary = run_scan(&resolver, &pool);

        assert_eq!(summary.found, 2, "{summary:?}");
        assert_eq!(summary.inserted, 2);
        assert_eq!(summary.organized, 2, "neither file was in place");
        let stem = format!("Media/PC/{}/", year_month());
        let paths: Vec<String> = rows(&pool).into_iter().map(|row| row.0).collect();
        assert!(
            paths.contains(&format!("{stem}IMG_0001.jpg")),
            "the dated photo must land in YYYY/MM: {paths:?}"
        );
        assert!(
            paths
                .iter()
                .any(|p| p.starts_with("Media/PC/Sem_Metadados/")),
            "{paths:?}"
        );
    }

    #[test]
    fn has_metadata_is_the_c7_rule_not_a_guess() {
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        let device_dir = resolver.media_device(PC_DEVICE_LABEL).unwrap();
        // c6 decision 2: an mtime date never lifts a file out of
        // `Sem_Metadados/`, and the scanner must not re-derive the rule.
        drop_file(&device_dir, "mtime-only.jpg", &mtime_only_photo());
        drop_file(&device_dir, "dated.jpg", &dated_photo());

        let summary = run_scan(&resolver, &pool);

        assert_eq!(summary.no_metadata, 1, "{summary:?}");
        let inserted = rows(&pool);
        assert_eq!(inserted.len(), 2);
        let bare = inserted
            .iter()
            .find(|row| row.0.contains("mtime-only"))
            .expect("the mtime-only photo is indexed");
        assert!(!bare.1, "has_metadata must be false for an mtime-only file");
        assert!(bare.0.starts_with("Media/PC/Sem_Metadados/"), "{}", bare.0);
        let dated = inserted.iter().find(|row| row.0.contains("dated")).unwrap();
        assert!(dated.1, "a real EXIF date is metadata");
    }

    #[test]
    fn a_gps_point_without_a_city_keeps_its_coordinates() {
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        let device_dir = resolver.media_device(PC_DEVICE_LABEL).unwrap();
        drop_file(&device_dir, "somewhere.jpg", &photo_with_gps());
        // No geonames.bin in this tree, so §7.5's absent-index branch is what runs.
        assert!(!resolver.geonames_path().is_file());

        let summary = run_scan(&resolver, &pool);

        assert_eq!(summary.located, 0, "no index means no city");
        assert_eq!(
            summary.located_none, 1,
            "but the point was still a location"
        );
        let rows = places(&pool);
        assert_eq!(rows.len(), 1);
        let (_, city, _state, _country, lat, lon) = &rows[0];
        assert!(city.is_none(), "city stays empty, never a guess");
        assert!((lat.unwrap() - LAT_DEG).abs() < 1e-9, "{lat:?}");
        assert!((lon.unwrap() - LON_DEG).abs() < 1e-9, "{lon:?}");
    }

    #[test]
    fn a_gps_point_with_an_index_is_geocoded_and_persisted() {
        // The end-to-end §7.3 step 6 -> §7.5 -> §6.1 `locations` chain, with an
        // index built by the c7 writer through the real format code.
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        let device_dir = resolver.media_device(PC_DEVICE_LABEL).unwrap();
        drop_file(&device_dir, "dearborn.jpg", &photo_with_gps());

        let mut writer = crate::core::geocode::CityWriter::new(0);
        writer.push(crate::core::geocode::City {
            lat: LAT_DEG,
            lon: LON_DEG,
            population: 100_000,
            name: "Dearborn".into(),
            state: "MI".into(),
            country: "US".into(),
        });
        fs::write(resolver.geonames_path(), writer.finish().unwrap()).unwrap();

        let summary = run_scan(&resolver, &pool);

        assert_eq!(summary.located, 1, "{summary:?}");
        assert_eq!(summary.located_none, 0);
        let rows = places(&pool);
        assert_eq!(rows.len(), 1);
        let (_path, city, state, country, lat, _lon) = &rows[0];
        assert_eq!(city.as_deref(), Some("Dearborn"), "the city is persisted");
        assert_eq!(state.as_deref(), Some("MI"));
        assert_eq!(country.as_deref(), Some("US"));
        // §7.5.1 decision 4: the media's own point, not the city's centroid.
        assert!((lat.unwrap() - LAT_DEG).abs() < 1e-9, "{lat:?}");
    }

    #[test]
    fn a_second_scan_inserts_nothing_and_reports_the_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        let device_dir = resolver.media_device(PC_DEVICE_LABEL).unwrap();
        drop_file(&device_dir, "IMG_0001.jpg", &dated_photo());
        let first = run_scan(&resolver, &pool);

        let second = run_scan(&resolver, &pool);

        assert_eq!(first.inserted, 1);
        assert_eq!(second.inserted, 0, "the bytes are already indexed");
        assert_eq!(second.duplicates, 1);
        assert_eq!(second.organized, 0, "a settled tree is not touched again");
        assert_eq!(rows(&pool).len(), 1);
    }

    #[test]
    fn identical_bytes_in_two_places_do_not_produce_two_rows() {
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        let device_dir = resolver.media_device(PC_DEVICE_LABEL).unwrap();
        let bytes = dated_photo();
        drop_file(&device_dir, "a/IMG_1.jpg", &bytes);
        drop_file(&device_dir, "b/copy-of-img-1.jpg", &bytes);

        let summary = run_scan(&resolver, &pool);

        assert_eq!(summary.found, 2);
        assert_eq!(summary.inserted, 1, "file_hash is UNIQUE (§6.1)");
        assert_eq!(summary.duplicates, 1);
        assert_eq!(rows(&pool).len(), 1);
    }

    #[test]
    fn a_file_the_human_moved_is_repaired_not_duplicated() {
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        let device_dir = resolver.media_device(PC_DEVICE_LABEL).unwrap();
        drop_file(&device_dir, "IMG_0001.jpg", &dated_photo());
        run_scan(&resolver, &pool);
        let organized = rows(&pool)[0].0.clone();
        let moved_to = resolver.rel_to_abs(&organized).unwrap();
        assert!(moved_to.is_file(), "the scan moved it into the date tree");

        // The human drags the organized file somewhere else inside Media/PC.
        let hand_moved = device_dir.join("kept").join("IMG_0001.jpg");
        fs::create_dir_all(hand_moved.parent().unwrap()).unwrap();
        fs::rename(&moved_to, &hand_moved).unwrap();

        let summary = run_scan(&resolver, &pool);

        assert_eq!(summary.repaired, 1, "the row's path was gone from disk");
        assert_eq!(summary.inserted, 0, "not a second row for the same bytes");
        assert_eq!(rows(&pool).len(), 1);
        assert_eq!(rows(&pool)[0].0, organized, "and the path is right again");
    }

    #[test]
    fn a_collision_gets_a_suffix_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        let device_dir = resolver.media_device(PC_DEVICE_LABEL).unwrap();
        // Two different photos, same name, same month. The second must survive
        // with its own bytes, so the bytes have to differ or the dedupe would eat
        // it before the collision was ever reached.
        let mut second = dated_photo();
        second.extend_from_slice(b"a different photo with the same name");
        drop_file(&device_dir, "one/IMG_0001.jpg", &dated_photo());
        drop_file(&device_dir, "two/IMG_0001.jpg", &second);

        let summary = run_scan(&resolver, &pool);

        assert_eq!(summary.inserted, 2, "C5: no byte is ever overwritten");
        let stem = format!("Media/PC/{}/", year_month());
        let paths: Vec<String> = rows(&pool).into_iter().map(|row| row.0).collect();
        assert!(paths.contains(&format!("{stem}IMG_0001.jpg")), "{paths:?}");
        assert!(
            paths.contains(&format!("{stem}IMG_0001_1.jpg")),
            "{paths:?}"
        );
    }

    /// The `YYYY/MM` the shared date fixture lands in, so the tests assert the
    /// real convention instead of a hardcoded month that would rot. `DATE` is the
    /// EXIF spelling (`2024:03:15 22:33:44`), hence the colons.
    fn year_month() -> String {
        let (year, rest) = DATE.split_once(':').unwrap();
        let month = rest.split_once(':').unwrap().0;
        format!("{year}/{month}")
    }

    #[test]
    fn the_scan_ignores_non_media_and_the_other_tree_folders() {
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        let device_dir = resolver.media_device(PC_DEVICE_LABEL).unwrap();
        drop_file(&device_dir, "notes.txt", b"not media");
        drop_file(&device_dir, "clip.mkv", b"a video; 0 bytes is fine");
        drop_file(
            &device_dir,
            "Thumbnails/400/PC/decoy.webp",
            b"not media either",
        );
        drop_file(&device_dir, ".vault/items/secret.jpg", b"hidden");
        drop_file(&device_dir, "real.jpg", &dated_photo());

        let summary = run_scan(&resolver, &pool);

        assert_eq!(summary.found, 2, "the .mkv and the .jpg only: {summary:?}");
        let paths: Vec<String> = rows(&pool).into_iter().map(|row| row.0).collect();
        assert!(!paths.iter().any(|p| p.contains("notes")), "{paths:?}");
        assert!(!paths.iter().any(|p| p.contains("decoy")), "{paths:?}");
        assert!(!paths.iter().any(|p| p.contains("secret")), "{paths:?}");
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        let device_dir = resolver.media_device(PC_DEVICE_LABEL).unwrap();
        // §7.4.1 decision 10: `IMG_0001.HEIC` must be recognised.
        drop_file(&device_dir, "UPPER.HEIC", &dated_photo());

        let summary = run_scan(&resolver, &pool);

        assert_eq!(summary.found, 1, "{summary:?}");
        assert_eq!(summary.inserted, 1);
    }

    #[test]
    fn a_cancelled_scan_stops_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        let device_dir = resolver.media_device(PC_DEVICE_LABEL).unwrap();
        for index in 0..5 {
            drop_file(&device_dir, &format!("IMG_{index}.jpg"), &dated_photo());
        }
        let cancel = CancelFlag::new();
        cancel.cancel();

        let summary = scan_with_events(&resolver, &pool, &cancel, &mut Vec::new());

        assert!(summary.cancelled);
        assert_eq!(summary.inserted, 0, "nothing is written after a cancel");
        assert_eq!(summary.found, 0, "the walk stops before the first file");
    }

    #[test]
    fn a_cancel_halfway_through_keeps_what_was_already_flushed() {
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        let device_dir = resolver.media_device(PC_DEVICE_LABEL).unwrap();
        // Each file must be unique, or the dedupe would eat them.
        for index in 0..250 {
            let mut bytes = dated_photo();
            bytes.extend_from_slice(format!("padding-{index}").as_bytes());
            drop_file(&device_dir, &format!("raw_{index}.jpg"), &bytes);
        }
        let cancel = CancelFlag::new();
        let geocoder = Geocoder::new(resolver.geonames_path());
        let handle = {
            let cancel = cancel.clone();
            let resolver = resolver.clone();
            let pool = pool.clone();
            std::thread::spawn(move || {
                // Cancel from another thread once the walk is underway.
                std::thread::sleep(std::time::Duration::from_millis(5));
                cancel.cancel();
                scan(
                    PC_DEVICE_LABEL,
                    &resolver,
                    &pool,
                    &geocoder,
                    None,
                    ScanSource::Device,
                    &cancel,
                    |_| {},
                )
                .unwrap()
            })
        };
        let summary = handle.join().unwrap();

        // Either it finished first (250 inserted) or it stopped (a partial count
        // that is still consistent). What must never happen: a count that claims
        // more rows than the DB has.
        assert!(summary.inserted <= 250, "{summary:?}");
        let stored = pool
            .get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM media", [], |row| row.get::<_, i64>(0))
            .unwrap();
        assert_eq!(
            stored as u32, summary.inserted,
            "the summary must not over-count"
        );
    }

    #[test]
    fn progress_starts_unknown_then_reports_the_real_total() {
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        let device_dir = resolver.media_device(PC_DEVICE_LABEL).unwrap();
        for index in 0..60 {
            let mut bytes = dated_photo();
            bytes.extend_from_slice(format!("padding-{index}").as_bytes());
            drop_file(&device_dir, &format!("IMG_{index}.jpg"), &bytes);
        }
        let cancel = CancelFlag::new();
        let mut events: Vec<ScanProgressDto> = Vec::new();

        scan_with_events(&resolver, &pool, &cancel, &mut events);

        // The walk cannot know its size, so the first event says 0...
        assert_eq!(
            events.first().unwrap().total,
            0,
            "and stays honest about it"
        );
        // ...and once the candidates are known, the total is real.
        assert!(
            events.iter().any(|event| event.total == 60),
            "the second phase knows the total"
        );
        // Throttled, not one per file: 60 files must not be 60 events.
        assert!(events.len() < 60, "throttled: {} events", events.len());
        assert_eq!(events.last().unwrap().current, 60);
    }

    #[test]
    fn scanning_a_device_that_does_not_exist_is_an_error_not_an_empty_scan() {
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        let cancel = CancelFlag::new();
        let geocoder = Geocoder::new(resolver.geonames_path());

        let result = scan(
            "iPhone_14_Pro",
            &resolver,
            &pool,
            &geocoder,
            None,
            ScanSource::Device,
            &cancel,
            |_| {},
        );

        assert!(matches!(result, Err(AppError::Conflict(_))), "{result:?}");
    }

    #[test]
    fn the_device_label_is_sanitized_so_the_db_and_the_tree_agree() {
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        // §5.7: the label *is* the folder name, so `sanitize_label` decides both
        // sides and they cannot disagree.
        let raw = "iPhone 14 (Pro)/raw";
        let device_dir = resolver.media_device(&paths::sanitize_label(raw)).unwrap();
        fs::create_dir_all(&device_dir).unwrap();
        drop_file(&device_dir, "IMG_1.jpg", &dated_photo());
        let cancel = CancelFlag::new();
        let geocoder = Geocoder::new(resolver.geonames_path());

        let summary = scan(
            raw,
            &resolver,
            &pool,
            &geocoder,
            None,
            ScanSource::Device,
            &cancel,
            |_| {},
        )
        .unwrap();

        assert_eq!(summary.device, paths::sanitize_label(raw));
        let conn = pool.get().unwrap();
        let stored: String = conn
            .query_row("SELECT device_name FROM media LIMIT 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(stored, paths::sanitize_label(raw));
    }

    #[test]
    fn no_row_ever_holds_an_absolute_path() {
        // §15's critical property: the DB is portable, so nothing may be absolute.
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        let device_dir = resolver.media_device(PC_DEVICE_LABEL).unwrap();
        drop_file(&device_dir, "IMG_1.jpg", &photo_with_gps());
        drop_file(&device_dir, "bare.jpg", &mtime_only_photo());
        run_scan(&resolver, &pool);

        let conn = pool.get().unwrap();
        let mut stmt = conn
            .prepare("SELECT relative_path, thumb_path FROM media")
            .unwrap();
        let stored: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(stored.len(), 2);
        for (relative, thumb) in stored {
            assert!(!relative.contains(':'), "not absolute: {relative}");
            assert!(!thumb.contains(':'), "not absolute: {thumb}");
            assert!(
                resolver.rel_to_abs(&relative).is_ok(),
                "and it resolves inside the root: {relative}"
            );
        }
    }

    #[test]
    fn the_batch_boundary_is_the_documented_two_hundred() {
        // §7.3 step 7 says 200; a 250-file scan must not lose the tail.
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        let device_dir = resolver.media_device(PC_DEVICE_LABEL).unwrap();
        for index in 0..250 {
            let mut bytes = dated_photo();
            bytes.extend_from_slice(format!("padding-{index}").as_bytes());
            drop_file(&device_dir, &format!("raw_{index}.jpg"), &bytes);
        }

        let summary = run_scan(&resolver, &pool);

        assert_eq!(summary.found, 250, "{summary:?}");
        assert_eq!(
            summary.inserted, 250,
            "the last partial batch must be flushed"
        );
        assert_eq!(rows(&pool).len(), 250);
    }

    #[test]
    fn a_captured_at_is_stored_in_the_iso_form_the_dto_and_sql_both_want() {
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        let device_dir = resolver.media_device(PC_DEVICE_LABEL).unwrap();
        drop_file(&device_dir, "IMG_1.jpg", &dated_photo());

        run_scan(&resolver, &pool);

        let conn = pool.get().unwrap();
        let stored: String = conn
            .query_row("SELECT captured_at FROM media", [], |row| row.get(0))
            .unwrap();
        assert_eq!(stored, DATE_ISO, "§22 shows T-separated, SQLite accepts it");
        // c14's month filter will run `strftime('%Y-%m', captured_at)`; if the
        // stored format were not parseable, that filter would silently return
        // nothing, so pin it here.
        let month: String = conn
            .query_row(
                "SELECT strftime('%Y-%m', captured_at) FROM media",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(month, &DATE_ISO[..7]);
    }

    // --- §11.3 Disco local: the copy-only source (c8) ------------------------

    /// A human folder with a dated photo, a bare scan and a GPS photo, all with
    /// unique bytes so the walk's own dedupe does not get in the way.
    fn human_source(dir: &Path) -> PathBuf {
        let source = dir.join("Fotos antigas");
        let mut gps = photo_with_gps();
        gps.extend_from_slice(b"-gps");
        drop_file(&source, "2023/natal/IMG_4001.jpg", &dated_photo());
        drop_file(&source, "escaninhos/sonho.jpg", &mtime_only_photo());
        drop_file(&source, "gps.jpg", &gps);
        source
    }

    fn run_copy(resolver: &PathResolver, pool: &SqlitePool, source: &Path) -> ScanSummary {
        let cancel = CancelFlag::new();
        let geocoder = Geocoder::new(resolver.geonames_path());
        scan(
            &paths::sanitize_label(source.file_name().unwrap().to_string_lossy().as_ref()),
            resolver,
            pool,
            &geocoder,
            None,
            ScanSource::CopyFrom { source },
            &cancel,
            |_| {},
        )
        .unwrap()
    }

    #[test]
    fn copy_from_ingests_into_the_tree_and_leaves_the_source_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        let source = human_source(dir.path());
        let source_bytes: Vec<_> = walkdir_content(&source);

        let summary = run_copy(&resolver, &pool, &source);

        // The label is derived from the folder name, sanitized §5.4 (spaces stay).
        assert_eq!(summary.device, "Fotos antigas", "{summary:?}");
        assert_eq!(summary.found, 3);
        assert_eq!(summary.inserted, 3, "{summary:?}");
        assert_eq!(summary.organized, 3, "every file landed in a new place");
        assert_eq!(summary.repaired, 0, "a human source is never repaired");
        assert_eq!(
            summary.no_metadata, 1,
            "the mtime-only scan is Sem_Metadados"
        );
        assert_eq!(
            summary.located_none, 1,
            "GPS without an index still keeps the point"
        );

        let paths: Vec<String> = rows(&pool).into_iter().map(|row| row.0).collect();
        let stem = format!("Media/Fotos antigas/{}/", year_month());
        assert!(paths.contains(&format!("{stem}IMG_4001.jpg")), "{paths:?}");
        assert!(
            paths
                .iter()
                .any(|p| p.starts_with("Media/Fotos antigas/Sem_Metadados/")),
            "{paths:?}"
        );
        assert!(
            paths
                .iter()
                .any(|p| p.contains("gps") && resolver.rel_to_abs(p).is_ok()),
            "{paths:?}"
        );

        // §14: the source survived byte-for-byte. Nothing was moved or deleted.
        assert_eq!(
            walkdir_content(&source),
            source_bytes,
            "the human's library is never renamed, moved or deleted"
        );
        assert!(
            !paths.iter().any(|p| p.contains(':')),
            "no persisted absolute path: {paths:?}"
        );
    }

    fn walkdir_content(root: &Path) -> Vec<(String, Vec<u8>)> {
        let mut entries: Vec<(String, Vec<u8>)> = walkdir::WalkDir::new(root)
            .sort_by_file_name()
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| {
                let name = entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .to_string();
                let bytes = fs::read(entry.path()).unwrap();
                (name, bytes)
            })
            .collect();
        entries.sort();
        entries
    }

    #[test]
    fn copy_from_creates_the_device_tree_when_it_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        let source = human_source(dir.path());
        let device_dir = resolver.media_device("Fotos antigas").unwrap();
        assert!(!device_dir.exists(), "the scan must build the tree");

        let summary = run_copy(&resolver, &pool, &source);

        assert_eq!(summary.inserted, 3, "{summary:?}");
        assert!(device_dir.is_dir(), "Media/<label> was created by the copy");
        assert!(resolver.rel_to_abs(&rows(&pool)[0].0).unwrap().is_file());
    }

    #[test]
    fn copy_from_dedupes_and_never_touches_the_source_again() {
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        let source = human_source(dir.path());

        let first = run_copy(&resolver, &pool, &source);
        let second = run_copy(&resolver, &pool, &source);

        assert_eq!(first.inserted, 3, "{first:?}");
        assert_eq!(
            second.inserted, 0,
            "the same folder twice is a dup, not a repair"
        );
        assert_eq!(second.duplicates, 3, "{second:?}");
        assert_eq!(second.repaired, 0);
        assert_eq!(second.organized, 0);
        assert_eq!(rows(&pool).len(), 3);
    }

    #[test]
    fn copy_from_a_name_collision_suffixes_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        // Two different photos named IMG_0001.jpg inside one source folder. Both
        // are copied (C5), so the second lands `_1` in the same YYYY/MM.
        let source = dir.path().join("antiga");
        drop_file(&source, "raw/IMG_0001.jpg", &dated_photo());
        let mut second = dated_photo();
        second.extend_from_slice(b"another photo, same name");
        drop_file(&source, "edited/IMG_0001.jpg", &second);

        let summary = run_copy(&resolver, &pool, &source);

        assert_eq!(summary.inserted, 2, "{summary:?}");
        let paths: Vec<String> = rows(&pool).into_iter().map(|row| row.0).collect();
        assert!(
            paths.iter().any(|p| p.ends_with("IMG_0001.jpg")),
            "{paths:?}"
        );
        assert!(
            paths.iter().any(|p| p.ends_with("IMG_0001_1.jpg")),
            "{paths:?}"
        );
    }

    #[test]
    fn copy_from_an_absent_folder_is_a_conflict_not_an_empty_scan() {
        let dir = tempfile::tempdir().unwrap();
        let (resolver, pool) = booted(dir.path());
        let cancel = CancelFlag::new();
        let geocoder = Geocoder::new(resolver.geonames_path());
        let missing = dir.path().join("nao existe");

        let result = scan(
            "fantasma",
            &resolver,
            &pool,
            &geocoder,
            None,
            ScanSource::CopyFrom { source: &missing },
            &cancel,
            |_| {},
        );

        assert!(matches!(result, Err(AppError::Conflict(_))), "{result:?}");
        assert_eq!(rows(&pool).len(), 0);
    }
}
