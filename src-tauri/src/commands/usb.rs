//! PLAN §8.1 USB commands: `devices_list`, `ingest_usb_start`, `ingest_usb_stop`.
//!
//! Thin wrappers, §7.11. All the work is the background job started here, and its
//! facts travel on `wolfs://progress/ingest` (§9.2) — the same channel the Wi-Fi
//! pane uses, because the Backup page shows one ingest at a time and both
//! transports produce the same three facts: bytes coming off the device, files
//! being indexed, and what it finally did.
//!
//! ## The order this file exists to enforce
//!
//! §12.1: **pull → scan → delete**. `ingest_usb_start` claims the §9.3 job slot,
//! then one dedicated thread
//!
//! 1. reads every new object into `Database/staging/usb/` and lets `Placement`
//!    rename it into `Media/<label>/` (`usb::ingest_from`, which deletes nothing);
//! 2. runs §7.3's `scanner::scan` over that folder — hashing, dedupe, and the DB
//!    insert that makes the files *ours*;
//! 3. and only for `mode: move`, deletes from the phone the objects it can prove
//!    it indexed ([`delete_proven`]).
//!
//! Phase 3 is unreachable on every failure path: a `Cancelled` pull or scan, an
//! `Err` scan, and a `move` pass that pulled nothing all return before it. That is
//! the difference between this and commit `ea44337`, which reported a fabricated
//! all-zero `ScanSummary` as a success.
//!
//! And phase 3 is not merely "the scan returned `Ok`", because `Ok` is weaker than
//! it looks: `ScanSummary::skipped` counts files the scan could not read, hash or
//! insert, and it walks past each one. A cancelled scan is `Ok` too. Both cases
//! delete nothing, and inside phase 3 each object is checked individually — see
//! [`delete_proven`], which is the load-bearing function of this file.
//!
//! ## Threading
//!
//! §9.2: COM is per-thread, so the whole job body runs on one spawned thread and
//! never migrates. `usb::DeviceSource` takes a device *friendly name* per call
//! rather than a handle, which is what lets that same thread re-open the device
//! after a reconnect mid-pass.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use tauri::{AppHandle, Emitter, State};

use crate::core::db::{self, SqlitePool};
use crate::core::geocode::Geocoder;
use crate::core::models::{DeviceDto, IngestEventDto, IngestUsbStartDto, ScanSummary, UsbModeDto};
use crate::core::paths::{self, PathResolver};
use crate::core::placement::{self, Placement};
use crate::core::scanner::{self, CancelFlag, ScanSource};
use crate::core::usb::{self, ActiveDeviceSource, DeviceSource, PullPass, RemoteObject};
use crate::error::AppError;
use crate::state::{AppState, JobKind, Snapshot, channels};

/// §8.1 `devices_list` — one attached MTP/PTP device, for §12.1's
/// `[C-3e Conectar iPhone] → WPD enumerate → DeviceDto → UI` flow.
///
/// Since c12 the transport is a **default** feature, so this enumerates real
/// devices in every normal build. On a deliberate `--no-default-features` build
/// it returns an empty list rather than an error: "no device attached" and "this
/// build cannot see devices" look identical to a user, and a toast on every
/// visit to the pane would be noise. The *start* command is where the difference
/// matters, and it does refuse.
#[tauri::command]
pub fn devices_list() -> Result<Vec<DeviceDto>, AppError> {
    ActiveDeviceSource.enumerate()
}

/// §8.1 `ingest_usb_start` — claims the §9.3 job slot and returns its `JobId`
/// (§8.1 says `JobId`; commit `ea44337` returned a `ScanSummary` here instead).
///
/// The summary arrives later on the event channel, not as this return value: a
/// 4 GB video takes minutes, so awaiting the final numbers here would look like a
/// hang, and it would make the promise a second click has to race against.
#[tauri::command]
pub fn ingest_usb_start(
    app: AppHandle,
    state: State<'_, AppState>,
    payload: IngestUsbStartDto,
) -> Result<String, AppError> {
    // Claimed before any work, so two clicks cannot both pass (§9.3).
    let job = state.jobs().claim(JobKind::IngestUsb)?;
    let job_id = job.id().to_string();
    let jobs = state.jobs();

    // Every early return below must free the slot, or the next click is refused
    // because of a job that never ran.
    let started = start(&app, payload, &state.snapshot(), &job, Arc::clone(&jobs));
    if let Err(error) = started {
        jobs.release(JobKind::IngestUsb, &job_id);
        return Err(error);
    }

    Ok(job_id)
}

/// Validates the payload and spawns the job thread. Split out of
/// [`ingest_usb_start`] so the slot-release-on-error path exists exactly once.
fn start(
    app: &AppHandle,
    payload: IngestUsbStartDto,
    snapshot: &Snapshot,
    job: &crate::state::Job,
    jobs: Arc<crate::state::JobRegistry>,
) -> Result<(), AppError> {
    let pool = snapshot.require_pool()?;
    let ffmpeg = snapshot.ffmpeg_ok.then(|| snapshot.resolver.ffmpeg_exe());

    // §5.4: a user-supplied `dest_label` is untrusted input and must be rejected
    // *before* the first byte moves, not after. Without one, the device's own
    // name is used, sanitized the same way.
    let raw_label = payload
        .dest_label
        .as_deref()
        .unwrap_or(&payload.device_friendly);
    let label = placement::sanitize_device_label(raw_label)?;

    let friendly = payload.device_friendly.clone();
    let job_id = job.id().to_string();
    let cancel = job.flag.clone();
    let resolver = snapshot.resolver.clone();
    let geocoder = Arc::clone(&snapshot.geocoder);
    let app_for_events = app.clone();
    let label_for_error = label.clone();
    let jobs_for_thread = Arc::clone(&jobs);

    std::thread::spawn(move || {
        // `catch_unwind` is here for the *job slot*, not for the COM calls (§9.3
        // has `wpd.rs` guarding those one by one). A panic escaping this thread
        // used to leave `JobKind::IngestUsb` claimed forever, and the next click
        // would be refused with `E_CONFLICT` against a job that no longer exists.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // c13 fix: an ingest is a write, so the "counts moved" signal must
            // leave the same channel the scan uses (§10.5) or the Mídia screen —
            // which listens only to `wolfs://db-changed` — stays stale after a
            // USB pass. The same event also triggers the §7.6 thumbnail mop-up
            // (c9 decision 1): ingest rows land without a render pass, so their
            // tiles stay grey until a scan rebuilds them. The check runs before
            // the emit moves the event.
            let emit = |event: IngestEventDto| {
                let is_db_change = matches!(event, IngestEventDto::DbChanged);
                let _ = app_for_events.emit(channels::INGEST_PROGRESS, event);
                if is_db_change {
                    let _ = app_for_events.emit(channels::DB_CHANGED, ());
                    super::media::mop_up_thumbs(
                        app_for_events.clone(),
                        Arc::clone(&jobs_for_thread),
                        resolver.clone(),
                        pool.clone(),
                        ffmpeg.clone(),
                    );
                }
            };
            run_job(
                Job {
                    source: &ActiveDeviceSource,
                    friendly: &friendly,
                    label: &label,
                    mode: payload.mode,
                    only_new: payload.only_new,
                    resolver: &resolver,
                    pool: &pool,
                    geocoder: &geocoder,
                    ffmpeg: ffmpeg.as_deref(),
                    cancel: &cancel,
                },
                &emit,
            )
        }));

        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                let _ = app_for_events.emit(
                    channels::INGEST_PROGRESS,
                    IngestEventDto::Failed {
                        device: label_for_error.clone(),
                        message: error.to_string(),
                    },
                );
            }
            Err(_) => {
                let _ = app_for_events.emit(
                    channels::INGEST_PROGRESS,
                    IngestEventDto::Failed {
                        device: label_for_error.clone(),
                        message: "falha inesperada na importação; nada foi apagado do aparelho"
                            .to_string(),
                    },
                );
            }
        }
        jobs_for_thread.release(JobKind::IngestUsb, &job_id);
    });

    Ok(())
}

/// §8.1 `ingest_usb_stop` — §12.1's `[C-3k Parar]`: flips the shared `CancelFlag`,
/// so the pull stops before the next object and, critically, the delete phase is
/// never reached.
///
/// Idempotent and silent when there is nothing to stop (§8.2: `E_CANCEL` is never
/// an error the user sees).
#[tauri::command]
pub fn ingest_usb_stop(state: State<'_, AppState>) -> Result<(), AppError> {
    state.jobs().cancel(JobKind::IngestUsb);
    Ok(())
}

/// Everything the job thread borrows, bundled so the `run_job` signature stays
/// readable and nothing is captured by accident.
struct Job<'a, S: DeviceSource> {
    source: &'a S,
    friendly: &'a str,
    label: &'a str,
    mode: UsbModeDto,
    only_new: bool,
    resolver: &'a PathResolver,
    pool: &'a SqlitePool,
    geocoder: &'a Geocoder,
    ffmpeg: Option<&'a Path>,
    cancel: &'a CancelFlag,
}

/// The three phases, in order. See the module docs for why the order is the whole
/// point of this file.
///
/// `emit` is injected rather than an `&AppHandle` so the safety order is testable
/// without a Tauri runtime: the tests drive this exact function with a fake device
/// and collect the events.
fn run_job<S: DeviceSource>(
    job: Job<'_, S>,
    emit: &impl Fn(IngestEventDto),
) -> Result<(), AppError> {
    let pulled = pull_phase(&job, emit)?;

    if pulled.cancelled {
        // Whatever landed before the cancel is a real file in the library; the next
        // scan mops it up. Deleting anything now is exactly what the user asked us
        // not to do.
        //
        // The terminal event still has to go out. Without it the pane would sit in
        // `running` forever, because `ingest_usb_stop` only flips a flag - the
        // frontend cannot know the job ended until the backend says so. This is the
        // same contract the folder scanner has (§7.3.1: a cancelled walk returns a
        // summary with `cancelled: true`, not an error), so `cancelled` is the one
        // field that says "this did not finish".
        emit_problems(emit, job.label, &pulled.errors);
        emit(IngestEventDto::Summary(cancelled_summary(job.label)));
        return Ok(());
    }

    // Phase 2 — §7.3 over the folder phase 1 just filled. Its `Ok` is necessary but
    // not sufficient for phase 3: `skipped` counts files it walked past.
    //
    // The `is_dir` guard is not an optimisation. `scanner::scan` opens
    // `Media/<label>/`, and that folder only exists once something has been placed
    // there — so with nothing pulled it fails with *"a pasta do dispositivo não
    // existe: C:\…\Media\Apple iPhone"*, which blames a local folder for a decision
    // the device made. When the folder is absent there is genuinely nothing to
    // index and no scan to run, so the pass ends here with an honest empty summary.
    // When it *is* present (an earlier import, or a cancel that landed files), the
    // scan still runs, because it is the mop-up that indexes them.
    let device_dir = job
        .resolver
        .media_device(&paths::sanitize_label(job.label))?;
    if !device_dir.is_dir() {
        let summary = ScanSummary {
            device: job.label.to_string(),
            ..ScanSummary::default()
        };
        emit(IngestEventDto::Summary(summary));
        if !pulled.errors.is_empty() {
            emit_problems(emit, job.label, &pulled.errors);
        }
        return Ok(());
    }

    let summary = scanner::scan(
        job.label,
        job.resolver,
        job.pool,
        job.geocoder,
        job.ffmpeg,
        ScanSource::Device,
        job.cancel,
        |progress| emit(IngestEventDto::Progress(progress.clone())),
    )?;

    emit(IngestEventDto::Summary(summary.clone()));
    if summary.inserted > 0 {
        emit(IngestEventDto::DbChanged);
    }

    // Stopped part-way through the scan: only part of what was pulled is indexed,
    // so nothing at all may leave the phone. This check has to be here, right
    // after the emit, because a cancelled scan is `Ok` - without it, pressing
    // Parar during a long scan would delete the files the scan never reached.
    if summary.cancelled {
        emit_problems(emit, job.label, &pulled.errors);
        return Ok(());
    }

    // Phase 3 — only now, and only for `move`.
    let mut problems = pulled.errors;
    if job.mode == UsbModeDto::Move {
        delete_proven(&job, &pulled.pulled, &mut problems);
    }
    emit_problems(emit, job.label, &problems);

    Ok(())
}

/// §12.1's third phase, per file instead of per pass.
///
/// `scan` returning `Ok` does not mean every pulled byte is indexed: `skipped`
/// counts files it could not read, hash or insert, and it continues past each one.
/// So before any object leaves the phone, each one gets its own proof — a `media`
/// row for *this* device, this name, this size ([`db::exists_in_device`]). What
/// cannot be proven stays on the phone and is reported, because "I could not
/// verify this copy" is the only honest answer and the file is the phone's to lose
/// or keep.
///
/// The consequence worth stating plainly: a file the scanner dropped as a duplicate
/// hash gets no row of its own, so in `move` mode it is kept on the phone. That is
/// the safe direction (§12.1 never trades a photo for tidiness) and it only shows up
/// on re-imports of an already-indexed library.
fn delete_proven<S: DeviceSource>(
    job: &Job<'_, S>,
    pulled: &[usb::Pulled],
    problems: &mut Vec<String>,
) {
    let mut proven = Vec::with_capacity(pulled.len());
    let mut kept = Vec::new();

    for entry in pulled {
        let object = &entry.object;
        let is_proven =
            db::exists_in_device(job.pool, job.label, &object.name, object.size).unwrap_or(false);
        if is_proven {
            proven.push(entry.clone());
        } else {
            kept.push(object.name.clone());
        }
    }

    if !kept.is_empty() {
        let shown: Vec<&str> = kept.iter().take(3).map(String::as_str).collect();
        problems.push(format!(
            "{} arquivo(s) ficaram no aparelho porque não foi possível confirmar \
             a cópia no SSD ({}). Nenhum deles foi apagado.",
            kept.len(),
            shown.join(", ")
        ));
    }

    usb::delete_pulled(job.source, job.friendly, &proven, problems);
}

/// A cancelled job still reports a summary; `cancelled: true` is what makes it
/// honest. Every counter is zero because nothing was indexed — reporting zeros
/// *with* the flag is a fact, reporting them as a finished scan was exactly the
/// fabrication commit `ea44337` shipped.
fn cancelled_summary(label: &str) -> ScanSummary {
    ScanSummary {
        device: label.to_string(),
        cancelled: true,
        ..ScanSummary::default()
    }
}

fn emit_problems(emit: &impl Fn(IngestEventDto), device: &str, problems: &[String]) {
    for message in problems {
        emit(IngestEventDto::Failed {
            device: device.to_string(),
            message: message.clone(),
        });
    }
}

/// Phase 1 alone, so the `Pull` progress events and the pull's own outcome stay
/// separable from the scan that follows.
fn pull_phase<S: DeviceSource>(
    job: &Job<'_, S>,
    emit: &impl Fn(IngestEventDto),
) -> Result<usb::UsbOutcome, AppError> {
    let done = AtomicU32::new(0);
    let pool = job.pool;
    let mut on_pulled = |object: &RemoteObject, total: u32| {
        let current = done.fetch_add(1, Ordering::Relaxed) + 1;
        emit(IngestEventDto::Pull {
            current,
            total,
            last_name: object.name.clone(),
        });
    };

    let placement = Placement::new(job.resolver, job.ffmpeg);
    // The listing is the one part of an import the human cannot see. On a phone
    // whose album layout we do not recognise it is *two* verified walks over every
    // object — measured at ~2 min for 3 288 files, paid before the first byte
    // moves — and until this event the pane and the shell's progress bar would sit
    // at zero with no explanation, which reads as a hang rather than as work.
    //
    // `total: 0` is the frontend's existing indeterminate marker (`dash.scanCollecting`),
    // so no new event variant is needed for "we have no idea how many yet".
    emit(IngestEventDto::Pull {
        current: 0,
        total: 0,
        last_name: String::new(),
    });
    usb::ingest_from(
        job.source,
        placement,
        PullPass {
            friendly: job.friendly,
            label: job.label,
            only_new: job.only_new,
            // §8.1's `only_new`, closing over the pool so `core::usb` never
            // learns about SQLite. A failed lookup answers "not indexed", which
            // re-reads a file rather than skipping one — the safe direction for a
            // predicate whose whole purpose is to avoid work.
            already_indexed: &|object: &RemoteObject| {
                db::exists_by_name_and_size(pool, &object.name, object.size).unwrap_or(false)
            },
            staging: &staging_dir(job.resolver)?,
            cancel: job.cancel,
            on_pulled: &mut on_pulled,
        },
    )
}

/// `Database/staging/usb`, created on demand. §7.8's staging rule: bytes land
/// here first and are renamed into `Media/` only once a file is complete.
fn staging_dir(resolver: &PathResolver) -> Result<PathBuf, AppError> {
    let staging = resolver.db_dir().join("staging").join("usb");
    std::fs::create_dir_all(&staging).map_err(|error| AppError::Ingest(error.to_string()))?;
    Ok(staging)
}

/// The order of §12.1, pinned. These run with no phone attached: `run_job` takes
/// its event sink as a closure precisely so the destructive phase can be asserted
/// on a laptop, against a fake device and a real database.
#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;
    use std::fs;
    use std::sync::Mutex;

    use crate::core::exif::tests::{Fixture as ExifFixture, jpeg, tiff};
    use crate::core::paths::RootSource;

    /// The c6 fixture builders, so the scanner sees a real dated photo and indexes
    /// it the way it indexes anything else — the proof in phase 3 depends on the
    /// scan genuinely inserting a row.
    fn dated_photo() -> Vec<u8> {
        jpeg(&tiff(ExifFixture::Date))
    }

    struct FakeDevice {
        files: BTreeMap<String, Vec<u8>>,
        deleted: Mutex<Vec<String>>,
        /// Folders the fake could not enumerate (`MediaListing::unreadable`).
        unreadable_folders: Vec<String>,
    }

    impl FakeDevice {
        fn new(files: &[(&str, Vec<u8>)]) -> Self {
            Self {
                files: files
                    .iter()
                    .map(|(name, bytes)| (name.to_string(), bytes.clone()))
                    .collect(),
                deleted: Mutex::new(Vec::new()),
                unreadable_folders: Vec::new(),
            }
        }

        fn blind(mut self, folders: &[&str]) -> Self {
            self.unreadable_folders = folders.iter().map(|f| f.to_string()).collect();
            self
        }

        fn deleted(&self) -> Vec<String> {
            self.deleted.lock().expect("the delete log").clone()
        }
    }

    impl DeviceSource for FakeDevice {
        fn enumerate(&self) -> Result<Vec<DeviceDto>, AppError> {
            Ok(Vec::new())
        }

        fn list_media(
            &self,
            _friendly: &str,
            _on_enum: Option<&mut dyn FnMut(&str)>,
            _cancel: Option<&CancelFlag>,
        ) -> Result<usb::MediaListing, AppError> {
            Ok(usb::MediaListing {
                objects: self
                    .files
                    .iter()
                    .map(|(name, bytes)| RemoteObject {
                        id: format!("id-{name}"),
                        name: name.clone(),
                        size: bytes.len() as u64,
                    })
                    .collect(),
                unreadable: self.unreadable_folders.clone(),
            })
        }

        fn read_to_abs(
            &self,
            _friendly: &str,
            object: &RemoteObject,
            staged: &Path,
        ) -> Result<u64, AppError> {
            let bytes = self
                .files
                .get(&object.name)
                .ok_or_else(|| AppError::Device("gone".into()))?;
            fs::write(staged, bytes).map_err(|error| AppError::Ingest(error.to_string()))?;
            Ok(bytes.len() as u64)
        }

        fn delete(&self, _friendly: &str, object: &RemoteObject) -> Result<(), AppError> {
            self.deleted
                .lock()
                .expect("the delete log")
                .push(object.name.clone());
            Ok(())
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        resolver: PathResolver,
        pool: SqlitePool,
        geocoder: Geocoder,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("a temp volume");
        let resolver = PathResolver::new(dir.path().to_path_buf(), RootSource::MarkerWalk);
        crate::core::layout::ensure(&resolver).expect("the §5.9 tree");
        let pool = db::open(&resolver).expect("the pool");
        let geocoder = Geocoder::new(resolver.geonames_path());
        Fixture {
            _dir: dir,
            resolver,
            pool,
            geocoder,
        }
    }

    fn job<'a>(
        source: &'a FakeDevice,
        mode: UsbModeDto,
        fixture: &'a Fixture,
        cancel: &'a CancelFlag,
    ) -> Job<'a, FakeDevice> {
        Job {
            source,
            friendly: "iPhone",
            label: "iPhone",
            mode,
            only_new: false,
            resolver: &fixture.resolver,
            pool: &fixture.pool,
            geocoder: &fixture.geocoder,
            ffmpeg: None,
            cancel,
        }
    }

    fn run(source: &FakeDevice, mode: UsbModeDto, fixture: &Fixture) -> Vec<IngestEventDto> {
        let cancel = CancelFlag::new();
        run_cancelled(source, mode, fixture, &cancel)
    }

    /// The same run, with the caller's `CancelFlag` so a test can flip it first.
    fn run_cancelled(
        source: &FakeDevice,
        mode: UsbModeDto,
        fixture: &Fixture,
        cancel: &CancelFlag,
    ) -> Vec<IngestEventDto> {
        let events: Mutex<Vec<IngestEventDto>> = Mutex::new(Vec::new());
        let sink = &events;
        run_job(job(source, mode, fixture, cancel), &|event| {
            sink.lock().expect("the event log").push(event);
        })
        .expect("the job");
        events.into_inner().expect("the event log")
    }

    fn summaries(events: &[IngestEventDto]) -> Vec<&ScanSummary> {
        events
            .iter()
            .filter_map(|event| match event {
                IngestEventDto::Summary(summary) => Some(summary),
                _ => None,
            })
            .collect()
    }

    fn messages(events: &[IngestEventDto]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                IngestEventDto::Failed { message, .. } => Some(message.clone()),
                _ => None,
            })
            .collect()
    }

    /// The whole order, end to end: bytes off the fake phone, hashed and indexed,
    /// and only then deleted from it.
    #[test]
    fn move_deletes_from_the_device_only_after_the_scan_indexed_the_file() {
        let fixture = fixture();
        let source = FakeDevice::new(&[("IMG_0001.jpg", dated_photo())]);

        let events = run(&source, UsbModeDto::Move, &fixture);

        assert_eq!(source.deleted(), vec!["IMG_0001.jpg".to_string()]);
        let summaries = summaries(&events);
        assert_eq!(summaries.len(), 1, "one terminal summary");
        assert!(!summaries[0].cancelled);
        assert_eq!(summaries[0].inserted, 1);
        assert!(messages(&events).is_empty(), "{:?}", messages(&events));
    }

    /// §12.1's other half: `copy` must never touch the phone, even when the scan
    /// succeeded and every byte is indexed.
    #[test]
    fn copy_never_deletes_from_the_device() {
        let fixture = fixture();
        let source = FakeDevice::new(&[("IMG_0001.jpg", dated_photo())]);

        let events = run(&source, UsbModeDto::Copy, &fixture);

        assert!(source.deleted().is_empty(), "{:?}", source.deleted());
        assert_eq!(summaries(&events)[0].inserted, 1);
    }

    /// Parar during the pull: nothing is deleted, and — the part that used to hang
    /// the pane — a terminal event still arrives, flagged `cancelled`.
    #[test]
    fn a_cancelled_pull_reports_a_cancelled_summary_and_deletes_nothing() {
        let fixture = fixture();
        let source = FakeDevice::new(&[("IMG_0001.jpg", dated_photo())]);
        let cancel = CancelFlag::new();
        cancel.cancel();

        let events = run_cancelled(&source, UsbModeDto::Move, &fixture, &cancel);

        assert!(source.deleted().is_empty(), "{:?}", source.deleted());
        let summaries = summaries(&events);
        assert_eq!(summaries.len(), 1, "a cancelled run still terminates");
        assert!(
            summaries[0].cancelled,
            "the flag is what makes the zeros honest"
        );
        assert!(!events.contains(&IngestEventDto::DbChanged));
    }

    /// The case that is not a cancel at all and not an error either: `scan`
    /// returned `Ok`, but these bytes never became a row. The phone keeps them.
    #[test]
    fn a_pulled_file_with_no_indexed_row_is_kept_on_the_device() {
        let fixture = fixture();
        let source = FakeDevice::new(&[("IMG_0001.jpg", dated_photo())]);
        let cancel = CancelFlag::new();

        // One object, as phase 1 would have produced it, with **no** matching row:
        // the shape `skipped` files leave behind.
        let orphan = usb::Pulled {
            object: RemoteObject {
                id: "id-IMG_0001.jpg".into(),
                name: "IMG_0001.jpg".into(),
                size: 10,
            },
            relative: "Media/iPhone/2024/03/IMG_0001.jpg".into(),
            bytes: 10,
        };
        let mut problems = Vec::new();
        delete_proven(
            &job(&source, UsbModeDto::Move, &fixture, &cancel),
            &[orphan],
            &mut problems,
        );

        assert!(source.deleted().is_empty(), "{:?}", source.deleted());
        assert_eq!(problems.len(), 1, "and it is reported: {problems:?}");
        assert!(
            problems[0].contains("IMG_0001.jpg"),
            "the user is told which file stayed: {problems:?}"
        );
    }

    /// The other half of the same test: a row that really does match is deleted.
    /// Without this, `delete_proven` could pass by never deleting anything.
    #[test]
    fn a_proven_file_is_deleted_and_only_the_unproven_one_is_kept() {
        let fixture = fixture();
        let photo = dated_photo();
        let size = photo.len() as u64;
        let source = FakeDevice::new(&[]);
        let cancel = CancelFlag::new();

        fixture
            .pool
            .get()
            .expect("a connection")
            .execute(
                "INSERT INTO media
                   (id, relative_path, thumb_path, file_hash, file_size, file_type,
                    captured_at, has_metadata, is_hidden, device_name)
                 VALUES ('m1', 'Media/iPhone/2024/03/IMG_0001.jpg', 'Thumbnails/400/x.webp',
                         'h1', ?1, 'image', '2024-03-15 10:00:00', 1, 0, 'iPhone')",
                rusqlite::params![size as i64],
            )
            .expect("the row");

        let proven = usb::Pulled {
            object: RemoteObject {
                id: "id-IMG_0001.jpg".into(),
                name: "IMG_0001.jpg".into(),
                size,
            },
            relative: "Media/iPhone/2024/03/IMG_0001.jpg".into(),
            bytes: size,
        };
        // Same name and size, but it was never indexed for *this* device.
        let other_device = usb::Pulled {
            object: RemoteObject {
                id: "id-OTHER.jpg".into(),
                name: "OTHER.jpg".into(),
                size,
            },
            relative: "Media/iPhone/2024/03/OTHER.jpg".into(),
            bytes: size,
        };

        let mut problems = Vec::new();
        delete_proven(
            &job(&source, UsbModeDto::Move, &fixture, &cancel),
            &[proven, other_device],
            &mut problems,
        );

        assert_eq!(source.deleted(), vec!["IMG_0001.jpg".to_string()]);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("OTHER.jpg"), "{problems:?}");
    }

    /// The bug the human hit on 2026-10-04 with a real iPhone: the device offered
    /// nothing, so nothing was pulled, so `Media/iPhone/` was never created — and
    /// the scanner then failed with *"a pasta do dispositivo não existe: C:\…"*.
    ///
    /// That message blames a local folder for something the phone did, and it is
    /// the one message guaranteed to be wrong: a folder of that name is absent
    /// *because* the import pulled nothing. With no folder there is nothing to
    /// index, so the pass ends with an honest zero summary instead of an error.
    #[test]
    fn a_device_with_nothing_to_offer_does_not_fail_with_a_local_path() {
        let fixture = fixture();
        let source = FakeDevice::new(&[]);

        let events = run(&source, UsbModeDto::Copy, &fixture);

        let summaries = summaries(&events);
        assert_eq!(summaries.len(), 1, "the pass still reports an outcome");
        assert_eq!(summaries[0].inserted, 0);
        assert!(!summaries[0].cancelled);
        assert!(source.deleted().is_empty());
        // The device folder is not created by a pass that pulled nothing.
        assert!(
            !fixture
                .resolver
                .media_device("iPhone")
                .expect("the device dir")
                .exists()
        );
    }

    /// A folder WPD would not enumerate is news, not silence — and it must not
    /// stop the files that *were* readable from being backed up.
    #[test]
    fn an_unreadable_folder_is_reported_and_the_readable_ones_still_import() {
        let fixture = fixture();
        let source = FakeDevice::new(&[("IMG_0001.jpg", dated_photo())])
            .blind(&["Internal Storage/DCIM/100APPLE: MTP indisponível (0x8007001F)"]);

        let events = run(&source, UsbModeDto::Copy, &fixture);

        let problems = messages(&events);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("100APPLE"), "{problems:?}");
        // The readable file is still indexed: one bad album is not a failed backup.
        assert_eq!(summaries(&events)[0].inserted, 1);
    }

    /// An import whose *listing* takes two minutes has to say so. Measured on real
    /// hardware: ~258 s of `EnumObjects` walks before the first byte moves, so a
    /// pass that reported nothing until the first file landed looks like a hang.
    /// The event goes out before the walk, and `total: 0` is the frontend's
    /// "indeterminate" marker, so the pane leaves "starting" immediately.
    #[test]
    fn the_listing_announces_itself_before_the_first_file_is_pulled() {
        let fixture = fixture();
        let source = FakeDevice::new(&[]);

        let events = run(&source, UsbModeDto::Copy, &fixture);

        let listing = events
            .iter()
            .find(|event| matches!(event, IngestEventDto::Pull { total: 0, .. }))
            .expect("an indeterminate pull event before anything was pulled");
        // It is the *first* thing the pass says: any order that reports a file
        // before announcing the walk would be a lie about where the time went.
        assert!(matches!(
            events.first(),
            Some(IngestEventDto::Pull { total: 0, .. })
        ));
        assert!(matches!(listing, IngestEventDto::Pull { current: 0, .. }));
        assert!(source.deleted().is_empty());
    }
}
