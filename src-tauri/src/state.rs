//! Application state (PLAN §7): the one object every command receives.
//!
//! Boot order is §5.5 and it is a straight line: resolve the root → create the
//! tree → open the database.
//!
//! **The boot is degraded, never fatal.** A read-only SSD or a locked file must
//! not close the app silently: the tree and the pool each record their own
//! failure, `[Z1d]` turns red, and the commands that need the pool return
//! `E_ROOT` / `E_DB` so the UI shows its Error branch (§10.6.4) with a retry.
//! That is what §8.2's "Settings alert" / "toast + reopen pool" reactions are
//! for. Only the resolution itself is infallible — it always yields a path.
//!
//! The state sits behind an `RwLock` for one reason: Settings → Origem
//! `[Recalcular Raiz]` (§5.6) re-runs the §5.2 resolution and, when the root
//! actually moved, the tree and the pool must move with it. `PathResolver` and
//! the r2d2 handle are both cheap to clone, so [`AppState::snapshot`] hands out
//! an owned view instead of making every command lock twice.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use crate::core::db::{self, SqlitePool};
use crate::core::geocode::Geocoder;
use crate::core::layout::{self, LayoutReport};
use crate::core::models::RootInfoDto;
use crate::core::paths::{PathResolver, RootSource};
use crate::core::scanner::CancelFlag;
use crate::error::AppError;

/// §9.2 event channels. Background work reaches the UI **only** through these;
/// commands return final values (c6/c7/c9/c18 produce them).
pub mod channels {
    pub const INGEST_PROGRESS: &str = "wolfs://progress/ingest";
    pub const SCAN_PROGRESS: &str = "wolfs://progress/scan";
    pub const THUMB_PROGRESS: &str = "wolfs://thumb/progress";
    pub const THUMB_DONE: &str = "wolfs://thumb/done";
    pub const VAULT: &str = "wolfs://vault";
    pub const TOAST: &str = "wolfs://toast";
    pub const DB_CHANGED: &str = "wolfs://db-changed";
}

/// An owned, cloneable view of the live state. Commands take a snapshot and then
/// work without holding any lock.
#[derive(Clone)]
pub struct Snapshot {
    pub resolver: PathResolver,
    pool: Option<SqlitePool>,
    /// §7.5's index, bound to this root. It is part of the snapshot, not of
    /// `AppState`, so §5.6 `[Recalcular Raiz]` hands out a geocoder that already
    /// points at the new `Database/geonames.bin` — a geocoder outliving a moved
    /// root would keep answering from a file the app no longer has.
    pub geocoder: Arc<Geocoder>,
    pub ffmpeg_ok: bool,
    pub first_run: bool,
    /// Tree creation failed → the root is not writable (`E_ROOT`).
    root_error: Option<String>,
    /// The database could not be opened (`E_DB`).
    db_error: Option<String>,
}

impl Snapshot {
    /// The pool, or the failure that explains its absence. A missing pool is
    /// never a `None` the caller can ignore.
    pub fn require_pool(&self) -> Result<SqlitePool, AppError> {
        if let Some(message) = &self.db_error {
            return Err(AppError::Db(db::DbError::Pool(message.clone())));
        }
        self.pool
            .clone()
            .ok_or_else(|| AppError::Unavailable("banco de dados indisponível".into()))
    }

    /// First boot failure to report, tree first: a missing tree explains more
    /// than whatever the database said afterwards.
    pub fn boot_error(&self) -> Option<AppError> {
        if let Some(message) = &self.root_error {
            return Some(AppError::Root(
                crate::core::paths::PathError::RootNotADirectory(message.clone()),
            ));
        }
        if let Some(message) = &self.db_error {
            return Some(AppError::Db(db::DbError::Pool(message.clone())));
        }
        None
    }
}

pub struct AppState {
    inner: RwLock<Snapshot>,
    /// §9.3. Outside the `RwLock` on purpose: a scan holds its flag for minutes,
    /// and a snapshot is cloned on every single command, so a job living in there
    /// would be copied constantly and could not be flipped from another command.
    /// `Arc` because the job thread outlives the command that started it and has
    /// to give the slot back when it ends.
    jobs: Arc<JobRegistry>,
    /// c11: the live Wi-Fi listener, if `[C-3i Iniciar servidor]` started one.
    /// Also outside the `RwLock` — the server owns its own threads and is
    /// deliberately *not* tied to the root's snapshot, so a recalculated root
    /// stops it explicitly instead of letting it write into a volume the app no
    /// longer uses.
    wifi: Arc<Mutex<Option<Arc<crate::core::wifi::WifiServer>>>>,
}

impl AppState {
    /// §5.5 boot. Returns the state even when the tree or the pool failed, so
    /// the window can explain the problem instead of the process dying.
    pub fn boot() -> Self {
        let resolver = PathResolver::resolve_app_root();
        let layout = layout::ensure(&resolver);
        let pool = match &layout {
            Ok(_) => db::open(&resolver),
            Err(_) => Err(db::DbError::Pool("árvore ausente".into())),
        };

        Self {
            inner: RwLock::new(Snapshot::assemble(resolver, layout, pool)),
            jobs: Arc::new(JobRegistry::default()),
            wifi: Arc::new(Mutex::new(None)),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.inner.read().expect("AppState lock poisoned").clone()
    }

    pub fn jobs(&self) -> Arc<JobRegistry> {
        Arc::clone(&self.jobs)
    }

    /// c11: the running listener, if any. `ingest_wifi_start` is idempotent
    /// against it, and `ingest_wifi_stop` is the only thing that clears it.
    pub fn wifi_server(&self) -> Option<Arc<crate::core::wifi::WifiServer>> {
        self.wifi.lock().expect("AppState wifi poisoned").clone()
    }

    pub fn set_wifi_server(&self, server: Option<Arc<crate::core::wifi::WifiServer>>) {
        *self.wifi.lock().expect("AppState wifi poisoned") = server;
    }

    /// Stops and forgets the listener. Called by `[C-3k Stop]` and by
    /// `[Recalcular Raiz]`: an upload landing in the old volume after the root
    /// moved would put bytes where the app can no longer find them.
    pub fn stop_wifi_server(&self) {
        if let Some(server) = self.wifi_server() {
            let _ = server.stop();
            self.set_wifi_server(None);
        }
    }

    /// What Settings → Origem and `[Z1d]` show (§5.6, §11.0).
    pub fn root_info(&self) -> RootInfoDto {
        let snapshot = self.snapshot();
        RootInfoDto {
            root: snapshot.resolver.root().display().to_string(),
            source: snapshot.resolver.source(),
            rooted: snapshot.resolver.is_rooted(),
            first_run: snapshot.first_run,
            ffmpeg_ok: snapshot.ffmpeg_ok,
        }
    }

    /// §5.6 `[Recalcular Raiz]`: re-run §5.2, re-validate ffmpeg, and rebuild
    /// the tree + pool when the root moved. Returns `changed` so the UI can say
    /// what happened.
    pub fn recalc_root(&self) -> Result<(RootInfoDto, bool), AppError> {
        let previous = self.snapshot();
        let resolver = PathResolver::resolve_app_root();
        let changed = resolver.root() != previous.resolver.root();

        let layout = layout::ensure(&resolver);
        // Only reopen when the root moved; otherwise keep the live handle.
        let pool = match (&layout, changed) {
            (_, true) => db::open(&resolver),
            (Ok(_), false) => Ok(previous.require_pool()?),
            (Err(_), false) => Err(db::DbError::Pool("árvore ausente".into())),
        };
        let snapshot = Snapshot::assemble(resolver, layout, pool);
        let info = RootInfoDto {
            root: snapshot.resolver.root().display().to_string(),
            source: snapshot.resolver.source(),
            rooted: snapshot.resolver.is_rooted(),
            first_run: snapshot.first_run,
            ffmpeg_ok: snapshot.ffmpeg_ok,
        };

        *self.inner.write().expect("AppState lock poisoned") = snapshot;
        if changed {
            // c11: the listener writes into the root it was built with, and the
            // tree it writes to no longer exists.
            self.stop_wifi_server();
        }
        Ok((info, changed))
    }

    /// §11.0 `[Z1d]` DB-health dot: is the pool there *and* answering.
    pub fn db_healthy(&self) -> bool {
        match self.snapshot().require_pool() {
            Ok(pool) => pool.get().is_ok(),
            Err(_) => false,
        }
    }
}

impl Snapshot {
    fn assemble(
        resolver: PathResolver,
        layout: Result<LayoutReport, crate::core::layout::LayoutError>,
        pool: Result<SqlitePool, db::DbError>,
    ) -> Self {
        let (ffmpeg_ok, first_run, root_error) = match &layout {
            Ok(report) => (report.ffmpeg_present, report.is_first_run(), None),
            Err(error) => (false, false, Some(error.to_string())),
        };
        let (pool, db_error) = match pool {
            Ok(pool) => (Some(pool), None),
            Err(error) => (None, Some(error.to_string())),
        };

        Snapshot {
            resolver: resolver.clone(),
            pool,
            geocoder: Arc::new(Geocoder::new(resolver.geonames_path())),
            ffmpeg_ok,
            first_run,
            root_error,
            db_error,
        }
    }
}

/// What a job does, so two scans cannot fight over one tree and a cancel lands on
/// the right flag. One slot per kind: §9.2's channels carry no job id, so the UI
/// can only ever watch one job of each kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JobKind {
    Scan,
    Thumbs,
    IngestUsb,
    IngestWifi,
}

/// §9.3: the cancel flag lives here, keyed by kind, and is flipped by a *different*
/// command than the one that started the job.
#[derive(Default)]
pub struct JobRegistry {
    running: Mutex<HashMap<JobKind, Job>>,
}

/// A claimed slot. The `CancelFlag` is an `Arc`, so the job thread keeps it alive
/// after the registry entry is gone and a late cancel cannot touch freed memory.
pub struct Job {
    id: String,
    pub flag: CancelFlag,
}

impl Job {
    pub fn id(&self) -> &str {
        &self.id
    }
}

impl JobRegistry {
    /// Takes the slot for `kind`, or `E_CONFLICT` when it is busy. Claiming
    /// before spawning is what makes "one scan at a time" true rather than
    /// best-effort: two clicks cannot both pass.
    pub fn claim(&self, kind: JobKind) -> Result<Job, AppError> {
        let mut running = self.running.lock().expect("JobRegistry poisoned");
        if running.contains_key(&kind) {
            return Err(AppError::Conflict(format!(
                "já existe uma operação {:?} em andamento",
                kind
            )));
        }
        let job = Job {
            id: uuid::Uuid::new_v4().to_string(),
            flag: CancelFlag::new(),
        };
        running.insert(
            kind,
            Job {
                id: job.id.clone(),
                flag: job.flag.clone(),
            },
        );
        Ok(job)
    }

    /// Asks the running job of `kind` to stop. Idempotent and silent when there
    /// is nothing to stop (§8.2: `E_CANCEL` is never an error the user sees).
    pub fn cancel(&self, kind: JobKind) -> bool {
        let running = self.running.lock().expect("JobRegistry poisoned");
        match running.get(&kind) {
            Some(job) => {
                job.flag.cancel();
                true
            }
            None => false,
        }
    }

    pub fn is_running(&self, kind: JobKind) -> bool {
        self.running
            .lock()
            .expect("JobRegistry poisoned")
            .contains_key(&kind)
    }

    /// Frees the slot. Called when the job ends, cancelled or failed, so the next
    /// click is never refused because of a job that is already gone.
    pub fn release(&self, kind: JobKind, id: &str) {
        let mut running = self.running.lock().expect("JobRegistry poisoned");
        if running.get(&kind).is_some_and(|job| job.id == id) {
            running.remove(&kind);
        }
    }
}

/// `RootSource` is part of the public boot info, so callers do not need to
/// import `core::paths` just to render a warning.
pub fn source_label(source: RootSource) -> &'static str {
    match source {
        RootSource::WolfsRootEnv => "WOLFS_ROOT",
        RootSource::MarkerWalk => "marker",
        // Not "marker": no marker was found, the root came from the layout.
        RootSource::PackagedAppDir => "App/ parent",
        RootSource::UnrootedFallback => "unrooted",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_at(root: &std::path::Path) -> AppState {
        let resolver = PathResolver::new(root.to_path_buf(), RootSource::MarkerWalk);
        let layout = layout::ensure(&resolver);
        let pool = db::open(&resolver);
        AppState {
            inner: RwLock::new(Snapshot::assemble(resolver, layout, pool)),
            jobs: Arc::new(JobRegistry::default()),
            wifi: Arc::new(Mutex::new(None)),
        }
    }

    #[test]
    fn a_healthy_boot_answers_queries() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());

        let snapshot = state.snapshot();
        assert!(snapshot.first_run);
        assert!(snapshot.boot_error().is_none());
        assert!(state.db_healthy());
        assert!(state.root_info().rooted);
    }

    #[test]
    fn an_unwritable_root_degrades_instead_of_dying() {
        let dir = tempfile::tempdir().unwrap();
        // A file where the tree should be: layout::ensure cannot create it.
        let root = dir.path().join("blocked");
        std::fs::write(&root, b"nao sou uma pasta").unwrap();
        let state = state_at(&root);

        let snapshot = state.snapshot();
        assert!(
            snapshot.root_error.is_some(),
            "the tree failure is recorded"
        );
        assert!(snapshot.pool.is_none());
        assert!(snapshot.boot_error().is_some());
        assert!(!state.db_healthy(), "[Z1d] must be able to show red");
        assert!(matches!(
            snapshot.require_pool(),
            Err(AppError::Db(_)) | Err(AppError::Unavailable(_))
        ));
    }

    #[test]
    fn snapshot_is_owned_and_repeatable() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());

        let first = state.snapshot();
        let second = state.snapshot();

        assert_eq!(first.resolver.root(), second.resolver.root());
        assert!(
            !first.ffmpeg_ok,
            "a temp tree has no App/bin/ffmpeg.exe, so the badge must be false"
        );
        assert!(state.db_healthy());
    }

    /// §9.2's channels carry no job id, so the registry is what makes "one scan
    /// at a time" true. A second click must be refused, not queued.
    #[test]
    fn only_one_job_of_a_kind_runs_at_a_time() {
        let registry = JobRegistry::default();

        let first = registry.claim(JobKind::Scan).unwrap();
        assert!(registry.is_running(JobKind::Scan));
        assert!(registry.claim(JobKind::Scan).is_err(), "E_CONFLICT");
        assert!(
            registry.claim(JobKind::Thumbs).is_ok(),
            "a different kind is a different slot"
        );

        // The flag is shared: the cancel command flips the one the job holds.
        let seen_by_the_job = first.flag.clone();
        assert!(registry.cancel(JobKind::Scan));
        assert!(seen_by_the_job.is_cancelled());

        registry.release(JobKind::Scan, first.id());
        assert!(!registry.is_running(JobKind::Scan));
        assert!(
            registry.claim(JobKind::Scan).is_ok(),
            "the slot is free again"
        );
    }

    /// §8.2: `E_CANCEL` is never an error the user sees, and the cancel button can
    /// be pressed twice, or with nothing running at all.
    #[test]
    fn cancelling_nothing_is_not_an_error() {
        let registry = JobRegistry::default();
        assert!(!registry.cancel(JobKind::Scan), "no job, nothing flipped");
    }

    /// A job that ended on its own must not be able to free a slot that a later
    /// job already took: the id is what makes `release` safe.
    #[test]
    fn a_stale_job_cannot_free_someone_elses_slot() {
        let registry = JobRegistry::default();
        let first = registry.claim(JobKind::Scan).unwrap();
        registry.release(JobKind::Scan, first.id());
        let second = registry.claim(JobKind::Scan).unwrap();

        registry.release(JobKind::Scan, first.id());

        assert!(
            registry.is_running(JobKind::Scan),
            "the new job keeps its slot"
        );
        assert_ne!(first.id(), second.id());
    }

    /// §5.6: the geocoder is bound to the root it was built for, so a root that
    /// moved cannot keep answering from the old volume's `Database/geonames.bin`.
    ///
    /// Asserted through what the user would see — the city, or its absence — and
    /// without touching the process environment, which §5.8 reads once and which
    /// no test may mutate in parallel.
    #[test]
    fn the_geocoder_is_bound_to_its_own_root() {
        use crate::core::geocode::{City, CityWriter};

        fn index_with(city: &str) -> Vec<u8> {
            let mut writer = CityWriter::new(0);
            writer.push(City {
                lat: 42.32,
                lon: -83.17,
                population: 100_000,
                name: city.into(),
                state: "MI".into(),
                country: "US".into(),
            });
            writer.finish().unwrap()
        }

        let dir = tempfile::tempdir().unwrap();
        let with_index = dir.path().join("com-indice");
        let without_index = dir.path().join("sem-indice");

        let first = state_at(&with_index);
        std::fs::write(
            first.snapshot().resolver.geonames_path(),
            index_with("Dearborn"),
        )
        .unwrap();
        let second = state_at(&without_index);

        let found = first
            .snapshot()
            .geocoder
            .geocode(42.32, -83.17)
            .expect("a cidade está no índice deste root");
        assert_eq!(found.city, "Dearborn");
        assert!(
            second.snapshot().geocoder.geocode(42.32, -83.17).is_none(),
            "o outro root não tem índice, e o geocoder não pode emprestar o de cima"
        );
    }
}
