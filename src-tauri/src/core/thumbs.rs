//! §7.6 `core/thumbs.rs` — the WebP rendition pipeline behind a bounded queue.
//!
//! Every gallery row owes the tree two files: `Thumbnails/400/...` (the hover
//! upgrade) and `Thumbnails/200/...` (the LQIP tile). The paths are not this
//! module's business — §6.7 stored the 400 one on the row at scan time, and the
//! 200 one is the §4.1 `400` → `200` swap ([`thumb_200_of`]). What lives here
//! is the *rendering*: decode → resize → encode WebP (q=80) → publish, for one
//! [`ThumbJob`] at a time, through a queue whose workers are what make the
//! grid light up tile by tile (`wolfs://thumb/*`, §7.6/§9.2).
//!
//! Three decisions this module carries, spelled out because §7.6 gives the
//! shape and the gate needs the reasoning:
//!
//! 1. **The producer runs after the scan, not inside it.** §7.6 says "the
//!    producer is the scanner/ingest finishing rows" — mechanically,
//!    [`crate::commands::media::scan_start`] spawns a mop-up thread once
//!    `scanner::scan` returns `Ok`. A producer inside the walk would hold the
//!    awaited `ScanSummary` promise hostage to the 512-slot queue filling up,
//!    so the scanner's signature (and c8's tests) stay untouched. Both
//!    producers claim the one `JobKind::Thumbs` slot (§9.3): a mop-up that
//!    finds a rebuild already running yields, because the rebuild's
//!    force-queue covers every row anyway.
//!
//! 2. **EXIF orientation is baked into the tile** ([`orient`]). A phone
//!    portrait whose rotation exists only in the metadata would render sideways
//!    in the grid, and the UI cannot fix it (it shows the tile as stored).
//!    [`crate::core::exif::orientation`] — the c9 helper of §7.4.1 — reads the
//!    value, and only the *rendition* rotates; §7.6's "tiles never contain
//!    EXIF-modified originals" is about not touching the source file, which
//!    this module never does.
//!
//! 3. **`thumb_meta` remembers failures only.** The two files are the source of
//!    truth: a pair that already exists renders as a no-op (no DB write), a
//!    render that succeeds clears any stale error row
//!    ([`crate::core::db::thumb_clear_failed`]), and only an error is written.
//!    That is what makes "run the mop-up after every scan" cheap and
//!    self-healing: a row that failed while ffmpeg was missing gets its retry
//!    for free.
//!
//! ffmpeg (HEIC/AVIF and every video) is optional for §7.4.1's reason: no
//! `App/bin/ffmpeg.exe` on this box (§3.3) is a normal state, and the row keeps
//! its failure until the binary exists. Nothing here *executes* ffmpeg in tests
//! for decision 12's reason — the binary is not in the dev box. What is tested
//! is the exact argv (§7.6 verbatim), the missing/broken binary paths, and the
//! whole queue end to end through the raster route.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use image::DynamicImage;
use thiserror::Error;

use crate::core::db::{self, DbError, SqlitePool, ThumbJobRow};
use crate::core::exif::VIDEO_EXTS;
use crate::core::models::{ThumbProgressDto, ThumbSummaryDto};
use crate::core::paths::{PathError, PathResolver};
use crate::error::SpawnError;

/// §7.6: the producer blocks here instead of growing an unbounded backlog —
/// backpressure lands on the *producer* (a thread that can wait), never on the
/// scanner's awaited promise (c9 decision 1).
const QUEUE_CAPACITY: usize = 512;

/// §7.6: `N = min(4, cpus)`. WebP encoding and JPEG decode are single-threaded
/// per file, so past four workers the SSD — not the CPU — is the bottleneck.
const WORKERS_MAX: usize = 4;

/// §7.6: q=80 for both slots.
const WEBP_QUALITY: f32 = 80.0;

/// §7.6: "a 60 s per-file timeout". For a video both ffmpeg passes share one
/// deadline, so a pathological file costs 60 s, not 120.
const FFMPEG_TIMEOUT_SECS: u64 = 60;

/// How often a running ffmpeg is polled. Files are small; 25 ms is invisible
/// next to the render itself and keeps the timeout honest.
const FFMPEG_POLL: Duration = Duration::from_millis(25);

/// §7.6's raster route: the formats the `image` crate decodes natively —
/// exactly `exif::IMAGE_EXTS` minus the containers ffmpeg owns. The partition
/// is asserted by a test, so adding an extension to §7.4.1 without a thumbnail
/// route fails the build's test run instead of losing tiles.
const RASTER_EXTS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "webp"];

/// §7.6: still containers that go to ffmpeg (no codec for them here), while
/// their 200 slot still comes from the 400 via the raster route.
const STILL_EXTS: &[&str] = &["heic", "heif", "avif"];

/// One row a producer enqueues: where the media lives and where its 400px
/// tile must land (§6.7). The 200px path is derived, never stored (§4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumbJob {
    pub media_id: String,
    pub relative_path: String,
    pub thumb_path: String,
}

impl From<ThumbJobRow> for ThumbJob {
    fn from(row: ThumbJobRow) -> Self {
        Self {
            media_id: row.id,
            relative_path: row.relative_path,
            thumb_path: row.thumb_path,
        }
    }
}

/// Why one media has no tile. Every arm becomes the `error` column of
/// `thumb_meta` (via `Display`) and — for the two ffmpeg-host arms — the
/// `E_FFMPEG` the §8.2 table already knows, once a command surfaces it.
#[derive(Debug, Error)]
pub enum ThumbError {
    /// The row points outside the SSD root (§5.8): the root moved under a
    /// stale row, same story `E_ROOT` tells everywhere else.
    #[error(transparent)]
    Root(#[from] PathError),
    /// The row is live but the file is gone — a manual deletion outside the
    /// app. Never a retry: `media_remove` is what removes rows.
    #[error("source media is gone: {0}")]
    SourceMissing(String),
    /// Not a media extension at all (§7.3's walker would not have created
    /// this row; defensive).
    #[error("no thumbnail renderer for extension '{0}'")]
    Unsupported(String),
    #[error("could not read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not decode {path}: {message}")]
    Decode { path: String, message: String },
    #[error("could not write {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// No `App/bin/ffmpeg.exe` (§3.3, §16.2): a normal state, retried by the
    /// next mop-up once the binary exists.
    #[error("ffmpeg is missing; thumbnail deferred: {0}")]
    FfmpegMissing(String),
    /// ffmpeg would not start (bad binary, permissions) — `SpawnError` is the
    /// same one §7.4 uses for dates.
    #[error(transparent)]
    Spawn(#[from] SpawnError),
    #[error("ffmpeg exited with {status} on {on}")]
    FfmpegFailed { status: i32, on: String },
    #[error("ffmpeg exceeded {seconds}s on {on}")]
    FfmpegTimeout { on: String, seconds: u64 },
    #[error("ffmpeg wrote no file: {output}")]
    FfmpegNoOutput { output: String },
    /// The stored `thumb_path` has no `/400/` segment to swap (§6.7): an
    /// upstream bug must fail loudly, never overwrite its own 400 slot with
    /// the 200.
    #[error("thumb_path breaks §6.7 (no /400/ segment): {0}")]
    BadThumbPath(String),
}

/// The whole library as producers read it: rows in path order, so a rebuild
/// and a mop-up walk the tree the same way.
pub fn all_jobs(pool: &SqlitePool) -> Result<Vec<ThumbJob>, DbError> {
    Ok(db::thumb_jobs(pool)?
        .into_iter()
        .map(ThumbJob::from)
        .collect())
}

/// §7.6's bounded MPSC queue: one [`mpsc::SyncSender`] shared by the producers,
/// `N` workers on the receiving side. Dropping every sender (via
/// [`Queue::shutdown`]) is how the workers learn the batch is complete.
pub struct Queue {
    tx: mpsc::SyncSender<ThumbJob>,
    counters: Arc<Counters>,
    workers: Vec<JoinHandle<()>>,
}

/// Batch verdict of one queue, the `wolfs://thumb/done` payload.
pub struct Drain {
    counters: Arc<Counters>,
    workers: Vec<JoinHandle<()>>,
}

impl Queue {
    /// Starts `min(4, cpus)` workers. `force` is per-queue, not per-job: a
    /// rebuild re-renders everything, a mop-up skips every pair that exists.
    /// `emit` is called once per job with its verdict — the §9.2 per-media
    /// event, so the gallery refreshes exactly the tile that changed.
    pub fn start(
        resolver: PathResolver,
        pool: SqlitePool,
        ffmpeg: Option<PathBuf>,
        force: bool,
        emit: impl Fn(&ThumbProgressDto) + Send + Sync + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::sync_channel(QUEUE_CAPACITY);
        let rx = Arc::new(Mutex::new(rx));
        let counters = Arc::new(Counters::default());
        let emit: Emit = Arc::new(emit);
        let workers = (0..worker_count())
            .map(|_| {
                let worker = Worker {
                    rx: Arc::clone(&rx),
                    resolver: resolver.clone(),
                    pool: pool.clone(),
                    ffmpeg: ffmpeg.clone(),
                    force,
                    counters: Arc::clone(&counters),
                    emit: Arc::clone(&emit),
                };
                thread::spawn(move || worker.run())
            })
            .collect();
        Self {
            tx,
            counters,
            workers,
        }
    }

    /// Hands one row to the workers. `false` means the batch is already
    /// shutting down — a race only a producer that ignores shutdown can hit,
    /// and the row is covered by the next run anyway.
    pub fn enqueue(&self, job: ThumbJob) -> bool {
        self.tx.send(job).is_ok()
    }

    /// Closes the channel and returns the handle that waits for the drain.
    /// Nothing renders after this returns: the summary is one [`Drain::join`]
    /// away.
    pub fn shutdown(self) -> Drain {
        let Self {
            tx,
            counters,
            workers,
        } = self;
        drop(tx);
        Drain { counters, workers }
    }
}

impl Drain {
    /// Waits for every worker and reports the §9.2 `done` payload: rows
    /// handed in (skips included) and rows that ended in `thumb_meta`.
    pub fn join(self) -> ThumbSummaryDto {
        for worker in self.workers {
            // A panicking worker already logged; the drain must not turn its
            // death into a second panic on the producer thread.
            let _ = worker.join();
        }
        ThumbSummaryDto {
            processed: self.counters.processed.load(Ordering::Relaxed) as u32,
            failed: self.counters.failed.load(Ordering::Relaxed) as u32,
        }
    }
}

/// The per-media event sink, type-erased so `Queue::start` stays generic.
type Emit = Arc<dyn Fn(&ThumbProgressDto) + Send + Sync + 'static>;

/// Shared by every worker of one queue; read only after the drain.
#[derive(Default)]
struct Counters {
    processed: AtomicU64,
    failed: AtomicU64,
}

fn worker_count() -> usize {
    thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .min(WORKERS_MAX)
}

/// One worker's whole state. Held in a struct because the loop *is* the
/// program: recv → render → record → emit, forever, until the senders go away.
struct Worker {
    rx: Arc<Mutex<mpsc::Receiver<ThumbJob>>>,
    resolver: PathResolver,
    pool: SqlitePool,
    ffmpeg: Option<PathBuf>,
    force: bool,
    counters: Arc<Counters>,
    emit: Emit,
}

impl Worker {
    fn run(self) {
        loop {
            // The lock exists only to make `recv` usable from N threads; it is
            // never held while rendering.
            let job = {
                let receiver = self.rx.lock().expect("thumb queue poisoned");
                match receiver.recv() {
                    Ok(job) => job,
                    // Every producer dropped its sender: the batch is over.
                    Err(_) => return,
                }
            };
            let result = render(&job, &self.resolver, self.ffmpeg.as_deref(), self.force);
            let ok = match result {
                Ok(()) => true,
                Err(error) => {
                    // A pool hiccup must not kill the worker: the media's
                    // verdict still reaches the UI either way.
                    if let Err(db_error) =
                        db::thumb_mark_failed(&self.pool, &job.media_id, &error.to_string())
                    {
                        eprintln!(
                            "[wolfsmedia] thumb_meta write failed for {}: {db_error}",
                            job.media_id
                        );
                    }
                    false
                }
            };
            if ok && let Err(db_error) = db::thumb_clear_failed(&self.pool, &job.media_id) {
                // Success with a stale error row: files exist, so the log is
                // wrong — worth saying out loud, not worth failing the tile.
                eprintln!(
                    "[wolfsmedia] thumb_meta cleanup failed for {}: {db_error}",
                    job.media_id
                );
            }
            self.counters.processed.fetch_add(1, Ordering::Relaxed);
            if !ok {
                self.counters.failed.fetch_add(1, Ordering::Relaxed);
            }
            (self.emit)(&ThumbProgressDto {
                media_id: job.media_id,
                ok,
            });
        }
    }
}

/// Renders one row into its two slots (or records why it could not).
///
/// Order matters: the convention check first (a broken row must not be able to
/// write to its own 400 slot), then the skip — *the files* decide, even when
/// the source is gone, because a pair that exists is a tile the gallery can
/// already show — and only then the route by extension.
fn render(
    job: &ThumbJob,
    resolver: &PathResolver,
    ffmpeg: Option<&Path>,
    force: bool,
) -> Result<(), ThumbError> {
    let thumb_200_rel = thumb_200_of(&job.thumb_path)
        .ok_or_else(|| ThumbError::BadThumbPath(job.thumb_path.clone()))?;
    let media = resolver.rel_to_abs(&job.relative_path)?;
    let thumb_400 = resolver.rel_to_abs(&job.thumb_path)?;
    let thumb_200 = resolver.rel_to_abs(&thumb_200_rel)?;

    if !force && thumb_400.is_file() && thumb_200.is_file() {
        return Ok(());
    }
    if !media.is_file() {
        return Err(ThumbError::SourceMissing(job.relative_path.clone()));
    }

    let ext = extension(&media);
    if RASTER_EXTS.contains(&ext.as_str()) {
        raster_pair(&media, &thumb_400, &thumb_200)
    } else if STILL_EXTS.contains(&ext.as_str()) {
        let ffmpeg = ffmpeg.ok_or_else(|| ThumbError::FfmpegMissing(job.relative_path.clone()))?;
        still_via_ffmpeg(ffmpeg, &media, &thumb_400, &thumb_200)
    } else if VIDEO_EXTS.contains(&ext.as_str()) {
        let ffmpeg = ffmpeg.ok_or_else(|| ThumbError::FfmpegMissing(job.relative_path.clone()))?;
        video_via_ffmpeg(ffmpeg, &media, &thumb_400, &thumb_200)
    } else {
        Err(ThumbError::Unsupported(ext))
    }
}

/// §7.6 raster route: one decode, orientation applied (decision 2), the 400
/// first and the 200 derived from that result in memory — "no double decode"
/// is what the second line of §7.6 asks for.
fn raster_pair(media: &Path, thumb_400: &Path, thumb_200: &Path) -> Result<(), ThumbError> {
    let image = orient(decode(media)?, crate::core::exif::orientation(media));
    let big = image.thumbnail(400, 400);
    write_webp(thumb_400, &big)?;
    write_webp(thumb_200, &big.thumbnail(200, 200))
}

/// The ffmpeg still route (§7.6): one frame at 400, then the 200 from that
/// 400 through the raster code — ffmpeg never runs twice for a photo.
fn still_via_ffmpeg(
    ffmpeg: &Path,
    media: &Path,
    thumb_400: &Path,
    thumb_200: &Path,
) -> Result<(), ThumbError> {
    let deadline = Instant::now() + Duration::from_secs(FFMPEG_TIMEOUT_SECS);
    let tmp = tmp_path(thumb_400);
    run_ffmpeg(ffmpeg, &still_400_args(media, &tmp), &tmp, media, deadline)?;

    // ffmpeg writes what the decoder saw, EXIF rotation included (decision 2);
    // a frame with orientation 1 is published as-is, byte for byte.
    let orientation = crate::core::exif::orientation(media);
    if orientation == 1 {
        rename_replace(&tmp, thumb_400)?;
    } else {
        let frame = orient(decode(&tmp)?, orientation);
        // The raw frame is spent: drop it before write_webp claims the same
        // `.tmp` name for the re-encoded, upright one.
        let _ = fs::remove_file(&tmp);
        write_webp(thumb_400, &frame)?;
    }

    let big = decode(thumb_400)?;
    write_webp(thumb_200, &big.thumbnail(200, 200))
}

/// §7.6's MOV/MP4 route: the tiny slot first, then the 400 — two passes, one
/// shared 60 s deadline, and two *separate* publishes: an interrupt between
/// them leaves a half-pair, which the existence check treats as "still owed",
/// so the next mop-up simply redoes both.
fn video_via_ffmpeg(
    ffmpeg: &Path,
    media: &Path,
    thumb_400: &Path,
    thumb_200: &Path,
) -> Result<(), ThumbError> {
    let deadline = Instant::now() + Duration::from_secs(FFMPEG_TIMEOUT_SECS);
    let tmp_200 = tmp_path(thumb_200);
    let tmp_400 = tmp_path(thumb_400);
    run_ffmpeg(
        ffmpeg,
        &video_200_args(media, &tmp_200),
        &tmp_200,
        media,
        deadline,
    )?;
    run_ffmpeg(
        ffmpeg,
        &video_400_args(media, &tmp_400),
        &tmp_400,
        media,
        deadline,
    )?;
    rename_replace(&tmp_400, thumb_400)?;
    rename_replace(&tmp_200, thumb_200)
}

/// §7.6 verbatim — MOV/MP4, slot 200. `-s 320x180` is PLAN's exact flag: it
/// stretches a portrait video (gate note, not a silent change).
fn video_200_args(media: &Path, out: &Path) -> Vec<String> {
    vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-ss".into(),
        "00:00:01".into(),
        "-i".into(),
        media.display().to_string(),
        "-vframes".into(),
        "1".into(),
        "-s".into(),
        "320x180".into(),
        "-f".into(),
        "webp".into(),
        out.display().to_string(),
    ]
}

/// §7.6's second MOV/MP4 pass: same second of seek, scaled to fit the 400 slot
/// instead of forced into the 320×180 box.
fn video_400_args(media: &Path, out: &Path) -> Vec<String> {
    vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-ss".into(),
        "00:00:01".into(),
        "-i".into(),
        media.display().to_string(),
        "-vframes".into(),
        "1".into(),
        "-vf".into(),
        "scale=400:400:force_original_aspect_ratio=decrease".into(),
        "-f".into(),
        "webp".into(),
        out.display().to_string(),
    ]
}

/// §7.6 verbatim — HEIC/AVIF still: no `-ss` (there is no timeline to seek)
/// and a frame count instead of a video count.
fn still_400_args(media: &Path, out: &Path) -> Vec<String> {
    vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-i".into(),
        media.display().to_string(),
        "-vf".into(),
        "scale=400:400:force_original_aspect_ratio=decrease".into(),
        "-frames:v".into(),
        "1".into(),
        "-f".into(),
        "webp".into(),
        out.display().to_string(),
    ]
}

/// Runs one ffmpeg pass to a `.tmp` file, with §7.6's per-file deadline.
///
/// stdin/stdout/stderr are null on purpose: nothing may ever answer ffmpeg's
/// overwrite prompt (the stale `.tmp` is removed first for that reason), and a
/// batch of hundreds of files must not pipe megabytes of log through the app.
fn run_ffmpeg(
    exe: &Path,
    args: &[String],
    output: &Path,
    media: &Path,
    deadline: Instant,
) -> Result<(), ThumbError> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|source| ThumbError::Write {
            path: parent.display().to_string(),
            source,
        })?;
    }
    let _ = fs::remove_file(output);

    let mut child = Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| {
            ThumbError::Spawn(SpawnError {
                program: exe.display().to_string(),
                source,
            })
        })?;

    loop {
        match child.try_wait() {
            Ok(None) if Instant::now() < deadline => thread::sleep(FFMPEG_POLL),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_file(output);
                return Err(ThumbError::FfmpegTimeout {
                    on: media.display().to_string(),
                    seconds: FFMPEG_TIMEOUT_SECS,
                });
            }
            Ok(Some(status)) => {
                if !status.success() {
                    let _ = fs::remove_file(output);
                    return Err(ThumbError::FfmpegFailed {
                        status: status.code().unwrap_or(-1),
                        on: media.display().to_string(),
                    });
                }
                if !output.is_file() {
                    return Err(ThumbError::FfmpegNoOutput {
                        output: output.display().to_string(),
                    });
                }
                return Ok(());
            }
            Err(source) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ThumbError::Spawn(SpawnError {
                    program: exe.display().to_string(),
                    source,
                }));
            }
        }
    }
}

/// Decodes a raster file with content sniffing, not just the extension: a
/// `.jpg` that is really a PNG must still render (and a garbage `.jpg` must
/// fail as a `Decode`, which is how the corrupt-file path is tested).
fn decode(path: &Path) -> Result<DynamicImage, ThumbError> {
    let shown = path.display().to_string();
    let reader = image::ImageReader::open(path).map_err(|source| ThumbError::Read {
        path: shown.clone(),
        source,
    })?;
    let reader = reader
        .with_guessed_format()
        .map_err(|source| ThumbError::Read {
            path: shown.clone(),
            source,
        })?;
    reader.decode().map_err(|error| ThumbError::Decode {
        path: shown,
        message: error.to_string(),
    })
}

/// Decision 2: EXIF orientation applied to the rendition. Values are the §7.4.1
/// semantics (`1` = upright, `6` = "rotate 90 CW", …) and `image`'s
/// `rotate90` is clockwise too, so 6 maps straight onto it; the mirrored
/// pairs (5/7) flip *after* their turn, which is what makes them transposes
/// rather than rotations. Anything outside 1..=8 is `1` already — this match
/// just falls through to the untouched image.
fn orient(image: DynamicImage, orientation: u16) -> DynamicImage {
    match orientation {
        2 => image.fliph(),
        3 => image.rotate180(),
        4 => image.flipv(),
        5 => image.rotate90().fliph(),
        6 => image.rotate90(),
        7 => image.rotate270().fliph(),
        8 => image.rotate270(),
        _ => image,
    }
}

/// Encodes one slot as WebP q=80 and publishes it: bytes go to `<dest>.tmp`
/// first (§7.6), then the rename — a reader of `Thumbnails/` sees either the
/// old tile or the new one, never half of either.
fn write_webp(dest: &Path, image: &DynamicImage) -> Result<(), ThumbError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|source| ThumbError::Write {
            path: parent.display().to_string(),
            source,
        })?;
    }
    let rgba = image.to_rgba8();
    let encoder = webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height());
    let bytes = encoder.encode(WEBP_QUALITY);
    let tmp = tmp_path(dest);
    fs::write(&tmp, &*bytes).map_err(|source| ThumbError::Write {
        path: tmp.display().to_string(),
        source,
    })?;
    rename_replace(&tmp, dest)
}

/// `dest` + `.tmp`: distinct from any real slot name, so interrupted runs
/// leave debris the next run overwrites instead of a tile a viewer could open.
fn tmp_path(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_owned();
    name.push(".tmp");
    PathBuf::from(name)
}

/// The §7.6 publish. Windows refuses to rename *over* an existing file, so a
/// rebuild removes the old tile first — the reader still never sees a partial,
/// because the swap itself remains a single rename.
fn rename_replace(from: &Path, to: &Path) -> Result<(), ThumbError> {
    if let Err(first) = fs::rename(from, to) {
        if !to.exists() {
            return Err(ThumbError::Write {
                path: to.display().to_string(),
                source: first,
            });
        }
        fs::remove_file(to).map_err(|source| ThumbError::Write {
            path: to.display().to_string(),
            source,
        })?;
        fs::rename(from, to).map_err(|source| ThumbError::Write {
            path: to.display().to_string(),
            source,
        })?;
    }
    Ok(())
}

/// §4.1/§6.7: `Thumbnails/400/...` → `Thumbnails/200/...`. `None` when the row
/// does not follow the convention — a bug upstream that must fail loudly.
fn thumb_200_of(thumb_400: &str) -> Option<String> {
    let swapped = thumb_400.replacen("/400/", "/200/", 1);
    (swapped != thumb_400).then_some(swapped)
}

/// The extension, lowercased — `IMG_0001.JPG` is `jpg` whichever device wrote
/// it. Same helper `exif::extension` has; kept local so this module never
/// reaches into another's private items.
fn extension(path: &Path) -> String {
    path.extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::io::Cursor;

    use image::{Rgb, RgbImage};
    use tempfile::TempDir;

    use crate::core::exif::IMAGE_EXTS;
    use crate::core::geocode::Geocoder;
    use crate::core::paths::{PC_DEVICE_LABEL, RootSource};
    use crate::core::scanner::{self, CancelFlag, ScanSource};

    fn resolver_at(root: &Path) -> PathResolver {
        PathResolver::new(root.to_path_buf(), RootSource::MarkerWalk)
    }

    /// A booted tree + pool: both halves of every test touch real state —
    /// the filesystem and the database — so neither can be faked. The same
    /// helper shape `scanner::tests` uses.
    fn booted(root: &Path) -> (PathResolver, SqlitePool) {
        let resolver = resolver_at(root);
        crate::core::layout::ensure(&resolver).unwrap();
        let pool = db::open(&resolver).unwrap();
        (resolver, pool)
    }

    /// A real, decodable JPEG: left half red, right half black. Built with the
    /// same crate the renderer uses (builders, not blobs — §7.4.1), because
    /// unlike c6's EXIF fixtures these files must actually decode.
    fn decodable_jpeg(width: u32, height: u32) -> Vec<u8> {
        let mut img = RgbImage::new(width, height);
        for x in 0..width {
            for y in 0..height {
                let px = if x < width / 2 {
                    Rgb([255, 0, 0])
                } else {
                    Rgb([0, 0, 0])
                };
                img.put_pixel(x, y, px);
            }
        }
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(img)
            .write_to(&mut bytes, image::ImageFormat::Jpeg)
            .unwrap();
        bytes.into_inner()
    }

    /// The same picture as PNG (lossless) for the plain-route queue test.
    fn decodable_png(width: u32, height: u32) -> Vec<u8> {
        let mut img = RgbImage::new(width, height);
        for x in 0..width {
            for y in 0..height {
                img.put_pixel(x, y, Rgb([0, 128, 255]));
            }
        }
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(img)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    /// A minimal big-endian TIFF whose only IFD entry is Orientation — the
    /// c6 fixture trick, one tag at a time.
    fn orientation_tiff(value: u16) -> Vec<u8> {
        let mut tiff = b"MM\x00\x2a".to_vec();
        tiff.extend_from_slice(&8u32.to_be_bytes()); // IFD0 starts at 8
        tiff.extend_from_slice(&1u16.to_be_bytes()); // exactly one entry
        tiff.extend_from_slice(&0x0112u16.to_be_bytes()); // Orientation
        tiff.extend_from_slice(&3u16.to_be_bytes()); // SHORT
        tiff.extend_from_slice(&1u32.to_be_bytes()); // count
        tiff.extend_from_slice(&value.to_be_bytes()); // value, left-aligned…
        tiff.extend_from_slice(&0u16.to_be_bytes()); // …in a 4-byte slot
        tiff.extend_from_slice(&0u32.to_be_bytes()); // no next IFD
        tiff
    }

    /// Splices an EXIF APP1 block right after SOI — a JPEG whose metadata
    /// *is* its orientation, without committing a binary fixture.
    fn jpeg_with_orientation(base: &[u8], orientation: u16) -> Vec<u8> {
        let tiff = orientation_tiff(orientation);
        let mut app1 = vec![0xFF, 0xE1];
        let payload = 6 + tiff.len(); // b"Exif\0\0" + TIFF block
        app1.extend_from_slice(&((payload + 2) as u16).to_be_bytes());
        app1.extend_from_slice(b"Exif\0\0");
        app1.extend_from_slice(&tiff);
        let mut out = vec![0xFF, 0xD8]; // SOI
        out.extend_from_slice(&app1);
        out.extend_from_slice(&base[2..]); // the original, minus its SOI
        out
    }

    fn drop_file(device_dir: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
        let path = device_dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    /// §6.7 as the scanner computes it, independently derived here so the
    /// test rows follow the convention without importing a private fn.
    fn thumb_rel(relative: &str) -> String {
        let stem = relative
            .strip_prefix("Media/")
            .unwrap_or(relative)
            .rsplit_once('.')
            .map(|(stem, _)| stem)
            .unwrap_or(relative);
        format!("Thumbnails/400/{stem}.webp")
    }

    fn insert_media(pool: &SqlitePool, id: &str, relative_path: &str) {
        pool.get()
            .unwrap()
            .execute(
                "INSERT INTO media (id, relative_path, thumb_path) VALUES (?1, ?2, ?3)",
                rusqlite::params![id, relative_path, thumb_rel(relative_path)],
            )
            .unwrap();
    }

    fn job(id: &str, relative_path: &str) -> ThumbJob {
        ThumbJob {
            media_id: id.to_string(),
            relative_path: relative_path.to_string(),
            thumb_path: thumb_rel(relative_path),
        }
    }

    /// `.tmp` debris under `Thumbnails/` — must be empty after any successful
    /// render (§7.6: the only files a reader may meet are real slots).
    fn tmp_debris(root: &Path) -> Vec<PathBuf> {
        walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.into_path())
            .filter(|path| path.extension() == Some(OsStr::new("tmp")))
            .collect()
    }

    #[test]
    fn a_photo_renders_both_slots_and_leaves_no_temp_files() {
        let dir = TempDir::new().unwrap();
        let (resolver, _pool) = booted(dir.path());

        let relative = "Media/PC/2024/03/IMG_1.jpg";
        let media = resolver.rel_to_abs(relative).unwrap();
        fs::create_dir_all(media.parent().unwrap()).unwrap();
        fs::write(&media, decodable_jpeg(800, 600)).unwrap();

        let job = job("m1", relative);
        render(&job, &resolver, None, false).unwrap();

        let thumb_400 = resolver.rel_to_abs(&job.thumb_path).unwrap();
        let thumb_200 = resolver
            .rel_to_abs(&job.thumb_path.replace("/400/", "/200/"))
            .unwrap();
        assert!(thumb_400.is_file(), "the 400 slot exists");
        assert!(thumb_200.is_file(), "the 200 slot exists");

        let big = image::open(&thumb_400).unwrap();
        assert_eq!((big.width(), big.height()), (400, 300), "fits the box");
        let small = image::open(&thumb_200).unwrap();
        assert_eq!((small.width(), small.height()), (200, 150), "from the 400");

        assert!(
            tmp_debris(&resolver.thumb_root()).is_empty(),
            "§7.6: nothing but real slots is ever left behind"
        );

        // The pair exists: both force=false (skip) and force=true (re-render)
        // succeed — the second one byte-identical through the same pipeline.
        render(&job, &resolver, None, false).unwrap();
        render(&job, &resolver, None, true).unwrap();
        assert!(tmp_debris(&resolver.thumb_root()).is_empty());
    }

    #[test]
    fn a_bad_row_fails_loudly_before_touching_anything() {
        let dir = TempDir::new().unwrap();
        let (resolver, _pool) = booted(dir.path());

        // The row's thumb_path has no /400/ to swap: refuse instead of
        // writing the 200 over the 400 (or the reverse).
        let broken = ThumbJob {
            media_id: "b1".into(),
            relative_path: "Media/PC/2024/03/IMG_1.jpg".into(),
            thumb_path: "Thumbnails/400X/PC/IMG_1.webp".into(),
        };
        assert!(matches!(
            render(&broken, &resolver, None, false),
            Err(ThumbError::BadThumbPath(_))
        ));

        // A row whose file was deleted outside the app: SourceMissing, but
        // only when there is no pair to skip to.
        let gone = job("b2", "Media/PC/2024/03/gone.jpg");
        assert!(matches!(
            render(&gone, &resolver, None, false),
            Err(ThumbError::SourceMissing(_))
        ));

        // An extension no renderer owns: Unsupported, never a silent no-op.
        let notes = "Media/PC/2024/03/notes.txt";
        let notes_abs = resolver.rel_to_abs(notes).unwrap();
        fs::create_dir_all(notes_abs.parent().unwrap()).unwrap();
        fs::write(&notes_abs, b"not media at all").unwrap();
        let weird = job("b3", notes);
        assert!(matches!(
            render(&weird, &resolver, None, false),
            Err(ThumbError::Unsupported(ext)) if ext == "txt"
        ));
    }

    #[test]
    fn skip_is_decided_by_the_files_even_when_the_source_is_gone() {
        let dir = TempDir::new().unwrap();
        let (resolver, _pool) = booted(dir.path());

        let relative = "Media/PC/2024/03/IMG_2.jpg";
        let media = resolver.rel_to_abs(relative).unwrap();
        fs::create_dir_all(media.parent().unwrap()).unwrap();
        fs::write(&media, decodable_jpeg(640, 480)).unwrap();

        let job = job("m2", relative);
        render(&job, &resolver, None, false).unwrap();
        let thumb_400 = resolver.rel_to_abs(&job.thumb_path).unwrap();
        let before = fs::read(&thumb_400).unwrap();

        // The source disappears (manual deletion outside the app): the pair
        // still exists, so the mop-up has nothing to say — and nothing to
        // rewrite.
        let moved = media.with_extension("jpg.moved");
        fs::rename(&media, &moved).unwrap();
        render(&job, &resolver, None, false).unwrap();
        assert_eq!(fs::read(&thumb_400).unwrap(), before, "skip wrote nothing");

        // But a forced render has no source to work from: say so.
        assert!(matches!(
            render(&job, &resolver, None, true),
            Err(ThumbError::SourceMissing(_))
        ));

        fs::rename(&moved, &media).unwrap();
    }

    #[test]
    fn an_orientation_six_portrait_is_baked_upright() {
        let dir = TempDir::new().unwrap();
        let (resolver, _pool) = booted(dir.path());

        // Stored landscape 800×400 (left half red), EXIF says "rotate 90 CW":
        // displayed it is a 400×800 portrait with red on top.
        let relative = "Media/PC/2024/07/portrait.jpg";
        let media = resolver.rel_to_abs(relative).unwrap();
        fs::create_dir_all(media.parent().unwrap()).unwrap();
        fs::write(&media, jpeg_with_orientation(&decodable_jpeg(800, 400), 6)).unwrap();
        assert_eq!(
            crate::core::exif::orientation(&media),
            6,
            "the spliced APP1 block is what the reader sees"
        );

        let job = job("m3", relative);
        render(&job, &resolver, None, false).unwrap();

        let thumb_400 = resolver.rel_to_abs(&job.thumb_path).unwrap();
        let tile = image::open(&thumb_400).unwrap();
        assert_eq!(
            (tile.width(), tile.height()),
            (200, 400),
            "a landscape source would have yielded 400×200 — orientation moved it"
        );

        let rgb = tile.to_rgb8();
        let top = rgb.get_pixel(100, 50).0;
        assert!(
            top[0] > 150 && top[1] < 110 && top[2] < 110,
            "the stored left half is the displayed top: {top:?}"
        );
        let bottom = rgb.get_pixel(100, 350).0;
        assert!(
            bottom[0] < 110 && bottom[1] < 110 && bottom[2] < 110,
            "the stored right half is the displayed bottom: {bottom:?}"
        );
    }

    /// The mapping itself, straight on pixels: `rotate90` is clockwise (the
    /// `image` crate docs say so, and this test pins it), and 5/7 are
    /// transposes rather than rotations.
    #[test]
    fn the_orientation_map_turns_exactly_the_way_exif_describes() {
        let mut src = RgbImage::new(2, 2);
        src.put_pixel(0, 0, Rgb([255, 0, 0]));
        src.put_pixel(1, 0, Rgb([0, 255, 0]));
        src.put_pixel(0, 1, Rgb([0, 0, 255]));
        src.put_pixel(1, 1, Rgb([255, 255, 0]));
        let src = DynamicImage::ImageRgb8(src);
        let px = |img: &DynamicImage, x: u32, y: u32| img.to_rgb8().get_pixel(x, y).0;

        // 6 = rotate 90 CW: TL→TR, TR→BR, BR→BL, BL→TL — so the destination
        // corners receive, respectively, BL, TL, BR, TR.
        let six = orient(src.clone(), 6);
        assert_eq!((six.width(), six.height()), (2, 2));
        assert_eq!(px(&six, 0, 0), [0, 0, 255]); // came from BL
        assert_eq!(px(&six, 1, 0), [255, 0, 0]); // came from TL
        assert_eq!(px(&six, 0, 1), [255, 255, 0]); // came from BR
        assert_eq!(px(&six, 1, 1), [0, 255, 0]); // came from TR

        // 8 = rotate 270 CW (90 CCW): TL→BL.
        let eight = orient(src.clone(), 8);
        assert_eq!(px(&eight, 0, 1), [255, 0, 0]);

        // 5 and 7 are the mirrored pairs — transposed pixels, not rotations.
        let five = orient(src.clone(), 5);
        assert_eq!(px(&five, 0, 0), [255, 0, 0]);
        assert_eq!(px(&five, 1, 1), [255, 255, 0]);
        let seven = orient(src.clone(), 7);
        assert_eq!(px(&seven, 0, 0), [255, 255, 0]);
        assert_eq!(px(&seven, 1, 1), [255, 0, 0]);

        // 3 = 180°: every corner swaps with its opposite.
        let three = orient(src.clone(), 3);
        assert_eq!(px(&three, 1, 1), [255, 0, 0]);

        // Unreadable or absent orientation (what `exif::orientation` returns
        // for anything it cannot parse) is `1`: the pixels stay put.
        for value in [0, 1, 9] {
            let same = orient(src.clone(), value);
            assert_eq!(px(&same, 0, 0), [255, 0, 0], "value {value} is a no-op");
        }
    }

    #[test]
    fn a_video_without_its_host_reports_the_missing_host_not_a_bad_file() {
        let dir = TempDir::new().unwrap();
        let (resolver, _pool) = booted(dir.path());

        let relative = "Media/PC/2024/03/clip.mov";
        let media = resolver.rel_to_abs(relative).unwrap();
        fs::create_dir_all(media.parent().unwrap()).unwrap();
        fs::write(&media, b"never decoded without a host").unwrap();
        let job = job("v1", relative);

        // No ffmpeg on the box (§3.3): a normal, retryable state.
        assert!(matches!(
            render(&job, &resolver, None, false),
            Err(ThumbError::FfmpegMissing(_))
        ));

        // A host that will not start is a different story — SpawnError, the
        // same shape §7.4 reports for dates.
        assert!(matches!(
            render(
                &job,
                &resolver,
                Some(Path::new("Z:\\nope\\ffmpeg.exe")),
                false
            ),
            Err(ThumbError::Spawn(_))
        ));

        // HEIC rides the same fence as video: stills are not free either.
        let heic_rel = "Media/PC/2024/03/IMG_9.heic";
        let heic = resolver.rel_to_abs(heic_rel).unwrap();
        fs::write(&heic, b"container without a host").unwrap();
        assert!(matches!(
            render(&job_heic(heic_rel), &resolver, None, false),
            Err(ThumbError::FfmpegMissing(_))
        ));
    }

    fn job_heic(relative_path: &str) -> ThumbJob {
        job("h1", relative_path)
    }

    #[test]
    fn the_ffmpeg_invocations_are_the_ones_7_6_wrote() {
        let media = Path::new("E:\\SSD\\Media\\PC\\2024\\03\\clip.mov");
        let out = Path::new("E:\\SSD\\Thumbnails\\200\\PC\\2024\\03\\clip.webp");

        assert_eq!(
            video_200_args(media, out),
            [
                "-hide_banner",
                "-loglevel",
                "error",
                "-ss",
                "00:00:01",
                "-i",
                "E:\\SSD\\Media\\PC\\2024\\03\\clip.mov",
                "-vframes",
                "1",
                "-s",
                "320x180",
                "-f",
                "webp",
                "E:\\SSD\\Thumbnails\\200\\PC\\2024\\03\\clip.webp",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
            "§7.6's MOV/MP4 slot-200 invocation, verbatim"
        );

        let out_400 = Path::new("E:\\SSD\\Thumbnails\\400\\PC\\2024\\03\\clip.webp");
        let args_400 = video_400_args(media, out_400);
        assert_eq!(
            args_400,
            [
                "-hide_banner",
                "-loglevel",
                "error",
                "-ss",
                "00:00:01",
                "-i",
                "E:\\SSD\\Media\\PC\\2024\\03\\clip.mov",
                "-vframes",
                "1",
                "-vf",
                "scale=400:400:force_original_aspect_ratio=decrease",
                "-f",
                "webp",
                "E:\\SSD\\Thumbnails\\400\\PC\\2024\\03\\clip.webp",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
            "the second MOV/MP4 pass: same seek, fitted to the 400 slot"
        );

        let still = Path::new("E:\\SSD\\Media\\PC\\IMG_1.HEIC");
        let still_out = Path::new("E:\\SSD\\Thumbnails\\400\\PC\\IMG_1.webp");
        assert_eq!(
            still_400_args(still, still_out),
            [
                "-hide_banner",
                "-loglevel",
                "error",
                "-i",
                "E:\\SSD\\Media\\PC\\IMG_1.HEIC",
                "-vf",
                "scale=400:400:force_original_aspect_ratio=decrease",
                "-frames:v",
                "1",
                "-f",
                "webp",
                "E:\\SSD\\Thumbnails\\400\\PC\\IMG_1.webp",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
            "§7.6's HEIC invocation, verbatim"
        );
    }

    #[test]
    fn the_route_lists_partition_the_741_image_list() {
        for ext in IMAGE_EXTS {
            assert!(
                RASTER_EXTS.contains(ext) || STILL_EXTS.contains(ext),
                "{ext} has no thumbnail route"
            );
        }
        for ext in VIDEO_EXTS {
            assert!(
                !RASTER_EXTS.contains(ext) && !STILL_EXTS.contains(ext),
                "{ext} must ride ffmpeg, not the image crate"
            );
        }
    }

    #[test]
    fn the_200_path_is_the_41_swap_and_never_a_guess() {
        assert_eq!(
            thumb_200_of("Thumbnails/400/PC/2024/03/IMG_1.webp").as_deref(),
            Some("Thumbnails/200/PC/2024/03/IMG_1.webp")
        );
        assert!(thumb_200_of("Thumbnails/300/PC/a.webp").is_none());
        assert!(thumb_200_of("Thumbnails/400").is_none());
        assert!(thumb_200_of("Media/PC/a.webp").is_none());
    }

    #[test]
    fn the_queue_renders_reports_and_records_only_real_failures() {
        let dir = TempDir::new().unwrap();
        let (resolver, pool) = booted(dir.path());
        let device = resolver.media_device(PC_DEVICE_LABEL).unwrap();

        // m1: real JPEG · m2: real PNG · m3: corrupt JPEG · m4: video, hostless.
        drop_file(&device, "2024/03/IMG_1.jpg", &decodable_jpeg(800, 600));
        drop_file(&device, "2024/03/IMG_2.png", &decodable_png(640, 480));
        drop_file(&device, "2024/03/broken.jpg", b"not an image at all");
        drop_file(&device, "2024/03/clip.mov", b"never decoded without a host");
        for id in ["m1", "m2", "m3", "m4"] {
            let rel = match id {
                "m1" => "Media/PC/2024/03/IMG_1.jpg",
                "m2" => "Media/PC/2024/03/IMG_2.png",
                "m3" => "Media/PC/2024/03/broken.jpg",
                _ => "Media/PC/2024/03/clip.mov",
            };
            insert_media(&pool, id, rel);
        }
        // A stale error from yesterday's run: m1 renders fine today, so the
        // log must go away with the success (decision 3).
        db::thumb_mark_failed(&pool, "m1", "stale").unwrap();

        let events = Arc::new(Mutex::new(Vec::new()));
        let queue = Queue::start(resolver.clone(), pool.clone(), None, false, {
            let events = Arc::clone(&events);
            move |event| events.lock().unwrap().push(event.clone())
        });
        for id in ["m1", "m2", "m3", "m4"] {
            let relative = match id {
                "m1" => "Media/PC/2024/03/IMG_1.jpg",
                "m2" => "Media/PC/2024/03/IMG_2.png",
                "m3" => "Media/PC/2024/03/broken.jpg",
                _ => "Media/PC/2024/03/clip.mov",
            };
            assert!(queue.enqueue(job(id, relative)));
        }
        let summary = queue.shutdown().join();
        assert_eq!(
            summary,
            ThumbSummaryDto {
                processed: 4,
                failed: 2
            },
            "both good files land, corrupt + hostless fail, nothing is lost"
        );

        // One event per row, with the verdict the tile needs.
        let mut seen = events.lock().unwrap().clone();
        seen.sort_by(|a, b| a.media_id.cmp(&b.media_id));
        assert_eq!(
            seen.iter().map(|e| e.media_id.as_str()).collect::<Vec<_>>(),
            ["m1", "m2", "m3", "m4"]
        );
        assert!(seen[0].ok && seen[1].ok);
        assert!(!seen[2].ok && !seen[3].ok);

        // thumb_meta holds exactly the failures — and m1's stale row is gone.
        let conn = pool.get().unwrap();
        let mut stmt = conn
            .prepare("SELECT media_id FROM thumb_meta ORDER BY media_id")
            .unwrap();
        let failed = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(failed, ["m3", "m4"], "{failed:?}");

        // The good tiles exist at both sizes; the failures left no debris.
        for relative in ["Media/PC/2024/03/IMG_1.jpg", "Media/PC/2024/03/IMG_2.png"] {
            let row = job("x", relative);
            assert!(resolver.rel_to_abs(&row.thumb_path).unwrap().is_file());
            assert!(
                resolver
                    .rel_to_abs(&row.thumb_path.replace("/400/", "/200/"))
                    .unwrap()
                    .is_file()
            );
        }
        assert!(
            tmp_debris(&resolver.thumb_root()).is_empty(),
            "failed renders clean their .tmp behind them"
        );

        // An empty batch drains to a zero verdict instead of hanging.
        let empty = Queue::start(resolver.clone(), pool.clone(), None, false, |_| {})
            .shutdown()
            .join();
        assert_eq!(empty, ThumbSummaryDto::default());
    }

    /// §7.6's producer, end to end: a walk lands rows, `all_jobs` reads them,
    /// the queue renders what it can — the same sequence `scan_start` and
    /// `thumbs_rebuild_all` run, minus the threads and events they add.
    #[test]
    fn the_post_scan_mop_up_makes_every_row_viewable() {
        let dir = TempDir::new().unwrap();
        let (resolver, pool) = booted(dir.path());
        let device = resolver.media_device(PC_DEVICE_LABEL).unwrap();

        drop_file(&device, "2024/05/IMG_7001.jpg", &decodable_jpeg(1024, 768));
        drop_file(&device, "2024/05/trip.mov", b"never decoded without a host");

        let cancel = CancelFlag::new();
        let geocoder = Geocoder::new(resolver.geonames_path());
        let summary = scanner::scan(
            PC_DEVICE_LABEL,
            &resolver,
            &pool,
            &geocoder,
            None,
            ScanSource::Device,
            &cancel,
            |_| {},
        )
        .unwrap();
        assert_eq!(summary.inserted, 2);

        let jobs = all_jobs(&pool).unwrap();
        assert_eq!(jobs.len(), 2, "both producers read the same ordered list");

        let queue = Queue::start(resolver.clone(), pool.clone(), None, false, |_| {});
        for job in &jobs {
            assert!(queue.enqueue(job.clone()));
        }
        let drained = queue.shutdown().join();
        assert_eq!(
            drained,
            ThumbSummaryDto {
                processed: 2,
                failed: 1
            }
        );

        let photo = jobs
            .iter()
            .find(|j| j.relative_path.ends_with(".jpg"))
            .unwrap();
        let thumb_400 = resolver.rel_to_abs(&photo.thumb_path).unwrap();
        let thumb_200 = resolver
            .rel_to_abs(&photo.thumb_path.replace("/400/", "/200/"))
            .unwrap();
        assert!(thumb_400.is_file() && thumb_200.is_file());
        let tile = image::open(&thumb_400).unwrap();
        assert!(tile.width() <= 400 && tile.height() <= 400);

        // The video owes an explanation, not a tile.
        let conn = pool.get().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM thumb_meta m JOIN media r ON r.id = m.media_id
                 WHERE r.relative_path LIKE '%trip.mov'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
}
