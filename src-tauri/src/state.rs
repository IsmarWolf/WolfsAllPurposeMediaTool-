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

use std::sync::RwLock;

use crate::core::db::{self, SqlitePool};
use crate::core::layout::{self, LayoutReport};
use crate::core::models::RootInfoDto;
use crate::core::paths::{PathResolver, RootSource};
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
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.inner.read().expect("AppState lock poisoned").clone()
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
            resolver,
            pool,
            ffmpeg_ok,
            first_run,
            root_error,
            db_error,
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
}
