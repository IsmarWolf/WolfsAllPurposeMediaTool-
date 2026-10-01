//! SQLite layer (PLAN §6). Owns **all** SQL (§6.5) — higher modules never see
//! `rusqlite` types.
//!
//! The five tables of §6.1 are created verbatim (with `IF NOT EXISTS`, so
//! `apply_schema` is safe on every boot), followed by the §6.3 indexes and the
//! §14 `process_log` diagnostics table. `PRAGMA user_version` gates migrations
//! (§6.4); v0 → v1 is "current".

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;
use std::time::Duration;
use thiserror::Error;

use crate::core::models::{DeviceCountDto, LocationDto, MediaScope, StatsDto, VaultStatusDto};
use crate::core::paths::PathResolver;

pub type SqlitePool = Pool<SqliteConnectionManager>;

/// §6.4: "Migration v0 -> v1 is current".
pub const SCHEMA_VERSION: i64 = 1;

/// §6.2: max 3 connections — WAL allows concurrent readers + one writer, and the
/// background scanner (c6) takes at most one.
pub const POOL_MAX_SIZE: u32 = 3;

/// §6.2: a hot spare stays warm so maintenance commands don't wait for a new
/// connection to be opened.
pub const POOL_MIN_IDLE: u32 = 1;

/// §6.2.
const BUSY_TIMEOUT_MS: i64 = 5_000;

/// How long `pool.get()` waits for a free connection. With 3 slots and one warm
/// spare, anything slower than this means the pool is starved (a leaked
/// connection or an unresponsive writer) — fail fast and let the caller report.
const POOL_CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

/// §6.2 — applied to **every** connection the pool opens, so a recycled
/// connection can never lose `foreign_keys` (it is per-connection in SQLite,
/// unlike `journal_mode`, which is persisted in the file header).
fn connection_pragmas() -> String {
    format!(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = {BUSY_TIMEOUT_MS};"
    )
}

/// §6.1, verbatim, with `IF NOT EXISTS` added so the DDL is re-runnable (§6.4).
const VERBATIM_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS media (
    id              TEXT PRIMARY KEY,
    relative_path   TEXT NOT NULL UNIQUE,
    thumb_path      TEXT NOT NULL,
    file_hash       TEXT UNIQUE,
    file_size       INTEGER,
    file_type       TEXT,                        -- "image" | "video"
    captured_at     DATETIME,                    -- NULL when unknown
    has_metadata    BOOLEAN DEFAULT 1,
    is_hidden       BOOLEAN DEFAULT 0,
    device_name     TEXT
);

CREATE TABLE IF NOT EXISTS locations (
    media_id TEXT PRIMARY KEY REFERENCES media(id) ON DELETE CASCADE,
    city TEXT, state TEXT, country TEXT,
    latitude REAL, longitude REAL
);

CREATE TABLE IF NOT EXISTS lists (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    filter_criteria TEXT,                         -- JSON (NULL => manual list)
    created_at  DATETIME
);

CREATE TABLE IF NOT EXISTS list_items (
    list_id TEXT REFERENCES lists(id) ON DELETE CASCADE,
    media_id TEXT REFERENCES media(id) ON DELETE CASCADE,
    PRIMARY KEY (list_id, media_id)
);

CREATE TABLE IF NOT EXISTS vault_config (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    password_hash TEXT NOT NULL,
    recovery_key_hash TEXT NOT NULL
);
"#;

/// §6.3. Additions to the verbatim schema are allowed; the 5 tables are fixed.
const INDEXES: &str = r#"
CREATE INDEX IF NOT EXISTS idx_media_captured    ON media(captured_at);
CREATE INDEX IF NOT EXISTS idx_media_device      ON media(device_name);
CREATE INDEX IF NOT EXISTS idx_media_hasmeta     ON media(has_metadata);
CREATE INDEX IF NOT EXISTS idx_media_hidden      ON media(is_hidden);          -- supports scope pruning
CREATE INDEX IF NOT EXISTS idx_media_type        ON media(file_type);
CREATE INDEX IF NOT EXISTS idx_locations_city    ON locations(city);
CREATE INDEX IF NOT EXISTS idx_list_items_media  ON list_items(media_id);      -- reverse lookup cost guard
"#;

/// §14 emergency bundle / diagnostics. Additive: it is not one of the five
/// domain tables of §6.1 and no media query depends on it.
const PROCESS_LOG_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS process_log (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    level      TEXT NOT NULL,                    -- "info" | "warn" | "error"
    code       TEXT,                             -- e.g. "E_THUMB", "E_VAULT"
    message    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_process_log_created ON process_log(created_at);
"#;

/// §7.6's side table: why a media row has no tile. Additive like §14's
/// `process_log` — not one of the five domain tables of §6.1, and no media
/// query depends on it.
///
/// Only **failures** live here. A row whose two WebP files exist needs no
/// marker (the files are the source of truth, §7.6), so `thumb_meta` answers
/// "why is there no tile" and is cleared the moment a render succeeds.
const THUMB_META_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS thumb_meta (
    media_id    TEXT PRIMARY KEY REFERENCES media(id) ON DELETE CASCADE,
    thumb_state TEXT NOT NULL,                    -- 'Error' (see §7.6 decisions)
    error       TEXT,
    updated_at  DATETIME NOT NULL DEFAULT (datetime('now'))
);
"#;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("could not reach the connection pool: {0}")]
    Pool(String),
    #[error("unexpected schema version {found} (this build knows up to {expected})")]
    SchemaVersion { found: i64, expected: i64 },
}

/// Opens (creating if needed) `Database/app.db` and applies the schema.
///
/// §5.5: lock the DB, then create the schema if `app.db` is missing.
pub fn open(resolver: &PathResolver) -> Result<SqlitePool, DbError> {
    let manager = SqliteConnectionManager::file(resolver.db_path())
        .with_init(|conn| conn.execute_batch(&connection_pragmas()));

    let pool = Pool::builder()
        .max_size(POOL_MAX_SIZE)
        .min_idle(Some(POOL_MIN_IDLE))
        .connection_timeout(POOL_CONNECTION_TIMEOUT)
        .build(manager)
        .map_err(|error| DbError::Pool(error.to_string()))?;

    {
        let mut conn = pool
            .get()
            .map_err(|error| DbError::Pool(error.to_string()))?;
        check_schema_version(&conn)?;
        apply_schema(&mut conn)?;
    }

    Ok(pool)
}

/// Creates the tables, indexes and the §14 log, then stamps `user_version`.
///
/// One transaction: a failure leaves the DB untouched (§6.4).
pub fn apply_schema(conn: &mut Connection) -> Result<(), DbError> {
    let tx = conn.transaction()?;
    tx.execute_batch(VERBATIM_SCHEMA)?;
    tx.execute_batch(INDEXES)?;
    tx.execute_batch(PROCESS_LOG_SCHEMA)?;
    tx.execute_batch(THUMB_META_SCHEMA)?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}

/// Rejects a database written by a newer build instead of corrupting it.
pub fn check_schema_version(conn: &Connection) -> Result<(), DbError> {
    let found: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if found > SCHEMA_VERSION {
        return Err(DbError::SchemaVersion {
            found,
            expected: SCHEMA_VERSION,
        });
    }
    Ok(())
}

/// §8.2: a gate that asks for a newer schema than the current migration ladder.
pub fn needs_migration(conn: &Connection) -> Result<bool, DbError> {
    let found: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    Ok(found < SCHEMA_VERSION)
}

/// §7.2 `stats(pool, scope) -> StatsDto` — the Dashboard stat cards and the
/// Zone 4 summary. Scope-aware by construction (§6.6): the only difference
/// between the two calls is the bound `is_hidden` value, and it is a
/// **parameter** (§6.5).
pub fn stats(pool: &SqlitePool, scope: MediaScope) -> Result<StatsDto, DbError> {
    let conn = pool
        .get()
        .map_err(|error| DbError::Pool(error.to_string()))?;
    let hidden = scope.hidden_value();

    let totals: (i64, i64, i64, i64, i64, i64, i64, i64) = conn.query_row(
        "SELECT COUNT(*),
                COALESCE(SUM(file_type = 'image'), 0),
                COALESCE(SUM(file_type = 'video'), 0),
                COALESCE(SUM(NOT has_metadata), 0),
                COALESCE(SUM(file_size), 0),
                COALESCE(SUM(CASE WHEN file_type = 'image' THEN file_size ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN file_type = 'video' THEN file_size ELSE 0 END), 0),
                COALESCE(SUM(is_hidden), 0)
         FROM media
         WHERE is_hidden = ?1",
        [hidden],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
            ))
        },
    )?;

    // `is_hidden` is bound, so a hidden row can never leak into a device count.
    let mut stmt = conn.prepare(
        "SELECT device_name, COUNT(*), COALESCE(SUM(file_size), 0)
         FROM media
         WHERE is_hidden = ?1 AND device_name IS NOT NULL
         GROUP BY device_name
         ORDER BY COUNT(*) DESC, device_name ASC",
    )?;
    let top_devices = stmt
        .query_map([hidden], |row| {
            Ok(DeviceCountDto {
                name: row.get(0)?,
                count: row.get(1)?,
                bytes: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(StatsDto {
        total_items: totals.0,
        images: totals.1,
        videos: totals.2,
        no_metadata: totals.3,
        total_bytes: totals.4,
        images_bytes: totals.5,
        videos_bytes: totals.6,
        hidden: totals.7,
        device_count: top_devices.len() as i64,
        top_devices,
    })
}

/// §7.2 `vacuum(pool)`: `VACUUM` rewrites the whole file, so it must never run
/// inside a transaction and must not hold a borrowed connection.
pub fn vacuum(pool: &SqlitePool) -> Result<(), DbError> {
    let conn = pool
        .get()
        .map_err(|error| DbError::Pool(error.to_string()))?;
    conn.execute_batch("VACUUM").map_err(DbError::from)
}

/// §7.2 `vault_probe(pool)`: is the vault configured at all? Deliberately does
/// not touch `password_hash` — c4 has no session, c18 owns unlock/hide.
pub fn vault_probe(pool: &SqlitePool) -> Result<VaultStatusDto, DbError> {
    let conn = pool
        .get()
        .map_err(|error| DbError::Pool(error.to_string()))?;
    let configured: i64 = conn.query_row(
        "SELECT COUNT(*) FROM vault_config WHERE id = 1",
        [],
        |row| row.get(0),
    )?;
    Ok(VaultStatusDto {
        configured: configured > 0,
        unlocked: false,
        has_recovery_hint: false,
    })
}

/// A row the scanner wants to exist, expressed without a `rusqlite` type so
/// `core::scanner` never sees one (§6.5). One per file, flattened with its
/// optional §7.5 location so a batch is a single transaction.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaInsert {
    pub id: String,
    /// Root-relative, `/`-joined (§6.1, §22).
    pub relative_path: String,
    /// The §6.7 400px path, computed at scan time from the §4.1 convention. It
    /// is a *destination*, not a claim that the file exists: c9 writes it.
    pub thumb_path: String,
    pub file_hash: String,
    pub file_size: i64,
    pub file_type: String,
    /// Naive local time, `%Y-%m-%d %H:%M:%S` — what §6.1's `DATETIME` column and
    /// §22's `2024-05-14T09:32:11` mean for a wall clock.
    pub captured_at: Option<String>,
    /// C7, computed by `RawMetadata::has_metadata` and never re-derived here.
    pub has_metadata: bool,
    pub device_name: String,
    /// The §7.5 answer, or `None` for "GPS but no city" *and* for "no GPS" — the
    /// two are distinguished by `captured_at`/the media's own columns, and a row
    /// with no location is correct in both cases.
    pub location: Option<LocationDto>,
}

/// What §7.3 step 2's dedupe check found for one hash: the row that already owns
/// these bytes, and the path it is recorded at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashMatch {
    pub id: String,
    pub relative_path: String,
}

/// §7.3 step 2 — is this content already indexed?
///
/// The lookup is by hash alone, because `media.file_hash` is `UNIQUE` (§6.1): a
/// second row with the same bytes is not representable, so "already indexed" is
/// the only answer that can be returned. The caller compares the path to decide
/// between "already organized" and "a second copy exists" (see §7.3.1 decision 3).
pub fn find_by_hash(conn: &Connection, hash: &str) -> Result<Option<HashMatch>, DbError> {
    let mut stmt =
        conn.prepare_cached("SELECT id, relative_path FROM media WHERE file_hash = ?1")?;
    let mut rows = stmt.query([hash])?;
    match rows.next()? {
        Some(row) => Ok(Some(HashMatch {
            id: row.get(0)?,
            relative_path: row.get(1)?,
        })),
        None => Ok(None),
    }
}

/// §7.3 step 7 — insert a batch in one transaction, flushing at
/// [`SCAN_BATCH_ROWS`].
///
/// One transaction per batch, not per row: 200 single-row commits on an
/// external SSD would spend the whole scan in fsync. Splitting at 200 bounds
/// the WAL growth and keeps a cancel responsive — the unflushed tail is lost, and
/// every flushed batch is already durable (§14 "never a partial file").
pub const SCAN_BATCH_ROWS: usize = 200;

/// Inserts `rows` and their locations, returning how many were written.
///
/// `INSERT` (not upsert) is deliberate: §7.3 step 2 has already decided what to
/// do about existing content, so a conflict here means two files in one batch
/// share a hash, and failing loudly beats silently overwriting a row whose
/// `is_hidden` or `device_name` we did not look at.
pub fn insert_media_batch(pool: &SqlitePool, rows: &[MediaInsert]) -> Result<usize, DbError> {
    if rows.is_empty() {
        return Ok(0);
    }
    let mut conn = pool
        .get()
        .map_err(|error| DbError::Pool(error.to_string()))?;
    let tx = conn.transaction()?;

    {
        let mut media = tx.prepare_cached(
            "INSERT INTO media
                (id, relative_path, thumb_path, file_hash, file_size, file_type,
                 captured_at, has_metadata, is_hidden, device_name)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9)",
        )?;
        let mut location = tx.prepare_cached(
            "INSERT INTO locations (media_id, city, state, country, latitude, longitude)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(media_id) DO UPDATE SET
                city = excluded.city, state = excluded.state,
                country = excluded.country, latitude = excluded.latitude,
                longitude = excluded.longitude",
        )?;

        for row in rows {
            media.execute(rusqlite::params![
                row.id,
                row.relative_path,
                row.thumb_path,
                row.file_hash,
                row.file_size,
                row.file_type,
                row.captured_at,
                row.has_metadata,
                row.device_name,
            ])?;
            if let Some(place) = &row.location {
                location.execute(rusqlite::params![
                    row.id,
                    place.city,
                    place.state,
                    place.country,
                    place.latitude,
                    place.longitude,
                ])?;
            }
        }
    }

    tx.commit()?;
    Ok(rows.len())
}

/// §7.3.1 decision 3: repoint a row whose file was moved inside `Media/` by hand.
///
/// Only the two path columns move. `file_hash` is the identity, and `id`,
/// `captured_at`, `has_metadata`, `is_hidden` and `locations` all still describe
/// the same bytes, so re-reading or rewriting them here would be a chance to lose
/// a flag the scan has no business touching — `is_hidden` above all, since a
/// repair must never un-hide a file.
pub fn update_media_path(
    pool: &SqlitePool,
    id: &str,
    relative_path: &str,
    thumb_path: &str,
) -> Result<(), DbError> {
    let conn = pool
        .get()
        .map_err(|error| DbError::Pool(error.to_string()))?;
    let changed = conn.execute(
        "UPDATE media SET relative_path = ?2, thumb_path = ?3 WHERE id = ?1",
        rusqlite::params![id, relative_path, thumb_path],
    )?;
    if changed == 0 {
        return Err(DbError::Sqlite(rusqlite::Error::InvalidQuery));
    }
    Ok(())
}

/// §7.6 — one row a thumbnail producer enqueues, without a `rusqlite` type
/// (§6.5): `thumb_path` is the 400px destination of §6.7, and the 200px path is
/// the §4.1 `400` → `200` swap derived in `thumbs.rs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumbJobRow {
    pub id: String,
    pub relative_path: String,
    pub thumb_path: String,
}

/// §7.6's two producers read the whole library here: the post-scan mop-up
/// (every row, so rows from before c9 and from an interrupted run are covered)
/// and `thumbs_rebuild_all`. Whether a row still *needs* its thumbs is the
/// worker's existence check, not a SQL question — the files are the truth.
pub fn thumb_jobs(pool: &SqlitePool) -> Result<Vec<ThumbJobRow>, DbError> {
    let conn = pool
        .get()
        .map_err(|error| DbError::Pool(error.to_string()))?;
    let mut stmt = conn.prepare_cached(
        "SELECT id, relative_path, thumb_path FROM media ORDER BY relative_path, id",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ThumbJobRow {
                id: row.get(0)?,
                relative_path: row.get(1)?,
                thumb_path: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// §7.6: record why a media has no tile. `thumb_meta` is a failure log, so the
/// `updated_at` column is stamped again on every retry.
pub fn thumb_mark_failed(pool: &SqlitePool, media_id: &str, error: &str) -> Result<(), DbError> {
    let conn = pool
        .get()
        .map_err(|error| DbError::Pool(error.to_string()))?;
    conn.execute(
        "INSERT INTO thumb_meta (media_id, thumb_state, error, updated_at)
         VALUES (?1, 'Error', ?2, datetime('now'))
         ON CONFLICT(media_id) DO UPDATE SET
            thumb_state = 'Error', error = excluded.error,
            updated_at = datetime('now')",
        rusqlite::params![media_id, error],
    )?;
    Ok(())
}

/// A render that finally succeeded owes nothing to the log: the files explain
/// themselves, so the failure row goes away (a no-op when there is none).
pub fn thumb_clear_failed(pool: &SqlitePool, media_id: &str) -> Result<(), DbError> {
    let conn = pool
        .get()
        .map_err(|error| DbError::Pool(error.to_string()))?;
    conn.execute("DELETE FROM thumb_meta WHERE media_id = ?1", [media_id])?;
    Ok(())
}

/// §7.3 step 2's dedupe check for a whole walk: one cached statement per
/// connection instead of a prepare per file.
pub fn find_hashes(pool: &SqlitePool, hashes: &[&str]) -> Result<Vec<Option<HashMatch>>, DbError> {
    let conn = pool
        .get()
        .map_err(|error| DbError::Pool(error.to_string()))?;
    hashes
        .iter()
        .map(|hash| find_by_hash(&conn, hash))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::paths::{PathResolver, RootSource};
    use std::path::Path;

    fn resolver_at(root: &Path) -> PathResolver {
        PathResolver::new(root.to_path_buf(), RootSource::MarkerWalk)
    }

    /// The real boot order of §5.5: the tree first, then the database. `open`
    /// deliberately does not create `Database/` — that is `layout`'s job.
    fn booted(root: &Path) -> PathResolver {
        let resolver = resolver_at(root);
        crate::core::layout::ensure(&resolver).unwrap();
        resolver
    }

    fn names(conn: &Connection, sql: &str) -> Vec<String> {
        let mut stmt = conn.prepare(sql).unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn tables(conn: &Connection) -> Vec<String> {
        let mut all = names(
            conn,
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        );
        // The additive side tables: §14's log and §7.6's `thumb_meta` are not
        // domain tables, so the §6.1 assertion below stays about the five.
        all.retain(|name| name != "process_log" && name != "thumb_meta");
        all
    }

    #[test]
    fn open_creates_the_thumb_meta_side_table_alongside_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open(&booted(dir.path())).unwrap();
        let conn = pool.get().unwrap();

        let side = names(
            &conn,
            "SELECT name FROM sqlite_master WHERE type = 'table'
             AND name IN ('process_log', 'thumb_meta') ORDER BY name",
        );
        assert_eq!(side, ["process_log", "thumb_meta"]);
    }

    #[test]
    fn open_creates_the_database_and_the_five_verbatim_tables() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = resolver_at(dir.path());
        crate::core::layout::ensure(&resolver).unwrap();
        assert!(
            !resolver.db_path().exists(),
            "layout must not create app.db"
        );

        let pool = open(&resolver).unwrap();

        assert!(resolver.db_path().exists(), "app.db is created by open");
        let conn = pool.get().unwrap();
        assert_eq!(
            tables(&conn),
            ["list_items", "lists", "locations", "media", "vault_config"]
        );
    }

    #[test]
    fn open_refuses_when_the_layout_was_never_built() {
        // The boot order is layout -> db. A missing Database/ is a boot bug, so
        // the error must be loud instead of silently creating a stray folder.
        let dir = tempfile::tempdir().unwrap();
        let resolver = resolver_at(dir.path());

        match open(&resolver) {
            Err(DbError::Pool(message)) => assert!(message.contains("app.db"), "{message}"),
            other => panic!("expected a pool error, got {other:?}"),
        }
        assert!(
            !resolver.db_dir().exists(),
            "open must not create the folder"
        );
    }

    #[test]
    fn open_creates_every_6_3_index() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open(&booted(dir.path())).unwrap();
        let conn = pool.get().unwrap();

        let indexes = names(
            &conn,
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name LIKE 'idx_%' ORDER BY name",
        );
        for expected in [
            "idx_list_items_media",
            "idx_locations_city",
            "idx_media_captured",
            "idx_media_device",
            "idx_media_hasmeta",
            "idx_media_hidden",
            "idx_media_type",
            "idx_process_log_created",
        ] {
            assert!(
                indexes.contains(&expected.to_string()),
                "missing {expected}"
            );
        }
    }

    #[test]
    fn every_pooled_connection_carries_the_6_2_pragmas() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open(&booted(dir.path())).unwrap();

        for _ in 0..POOL_MAX_SIZE {
            let conn = pool.get().unwrap();
            let journal: String = conn
                .pragma_query_value(None, "journal_mode", |row| row.get(0))
                .unwrap();
            let foreign_keys: i64 = conn
                .pragma_query_value(None, "foreign_keys", |row| row.get(0))
                .unwrap();
            let busy: i64 = conn
                .pragma_query_value(None, "busy_timeout", |row| row.get(0))
                .unwrap();
            assert_eq!(journal.to_lowercase(), "wal");
            assert_eq!(foreign_keys, 1, "foreign_keys must be ON per connection");
            assert_eq!(busy, BUSY_TIMEOUT_MS);
            drop(conn);
        }
    }

    #[test]
    fn apply_schema_is_idempotent_and_preserves_rows() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = booted(dir.path());
        let mut conn = Connection::open(resolver.db_path()).unwrap();

        apply_schema(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO media (id, relative_path, thumb_path, file_type) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                "m1",
                "PC/2026/09/28/foto.jpg",
                "Thumbnails/400/PC/2026/09/28/foto.webp",
                "image"
            ],
        )
        .unwrap();
        let tables_before = tables(&conn);
        let indexes_before = names(&conn, "SELECT name FROM sqlite_master WHERE type = 'index'");

        apply_schema(&mut conn).unwrap();
        apply_schema(&mut conn).unwrap();

        assert_eq!(
            tables(&conn),
            tables_before,
            "no table is recreated or lost"
        );
        assert_eq!(
            names(&conn, "SELECT name FROM sqlite_master WHERE type = 'index'"),
            indexes_before,
            "indexes are not duplicated"
        );
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM media", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1, "data survives a re-run");
    }

    #[test]
    fn apply_schema_stamps_user_version_1() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = Connection::open(dir.path().join("app.db")).unwrap();
        assert!(needs_migration(&conn).unwrap());

        apply_schema(&mut conn).unwrap();

        assert!(!needs_migration(&conn).unwrap());
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn a_newer_database_is_refused_instead_of_downgraded() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = booted(dir.path());
        {
            let conn = Connection::open(resolver.db_path()).unwrap();
            conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
                .unwrap();
        }

        match open(&resolver) {
            Err(DbError::SchemaVersion { found, expected }) => {
                assert_eq!(found, SCHEMA_VERSION + 1);
                assert_eq!(expected, SCHEMA_VERSION);
            }
            other => panic!("expected a schema-version refusal, got {other:?}"),
        }
    }

    #[test]
    fn foreign_keys_cascade_from_media_to_locations_and_list_items() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open(&booted(dir.path())).unwrap();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "INSERT INTO media (id, relative_path, thumb_path) VALUES ('m1', 'a.jpg', 'a.webp');
             INSERT INTO locations (media_id, city) VALUES ('m1', 'Sao Paulo');
             INSERT INTO lists (id, name) VALUES ('l1', 'Favoritas');
             INSERT INTO list_items (list_id, media_id) VALUES ('l1', 'm1');",
        )
        .unwrap();

        conn.execute("DELETE FROM media WHERE id = 'm1'", [])
            .unwrap();

        let locations: i64 = conn
            .query_row("SELECT COUNT(*) FROM locations", [], |row| row.get(0))
            .unwrap();
        let items: i64 = conn
            .query_row("SELECT COUNT(*) FROM list_items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            (locations, items),
            (0, 0),
            "CASCADE only works with foreign_keys ON"
        );
    }

    fn seed_media(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO media (id, relative_path, thumb_path, file_type, file_size, captured_at, has_metadata, is_hidden, device_name) VALUES
               ('m1', 'PC/2026/01/01/a.jpg',  'Thumbnails/400/PC/a.webp',  'image', 1000, '2026-01-01 10:00:00', 1, 0, 'PC'),
               ('m2', 'PC/2026/01/02/b.jpg',  'Thumbnails/400/PC/b.webp',  'image', 2000, '2026-01-02 10:00:00', 0, 0, 'PC'),
               ('m3', 'iPhone/2026/01/03/c.mov','Thumbnails/400/iPhone/c.webp','video', 3000, NULL,                   1, 0, 'iPhone'),
               ('m4', 'iPhone/.vault/d.jpg',   'Thumbnails/400/vault/d.webp', 'image', 4000, '2026-01-04 10:00:00', 1, 1, 'iPhone');",
        )
        .unwrap();
    }

    #[test]
    fn stats_counts_by_type_and_keeps_sizes_apart() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open(&booted(dir.path())).unwrap();
        let conn = pool.get().unwrap();
        seed_media(&conn);

        let standard = stats(&pool, MediaScope::Standard).unwrap();

        assert_eq!(standard.total_items, 3, "the hidden row is not in scope");
        assert_eq!(standard.images, 2);
        assert_eq!(standard.videos, 1);
        assert_eq!(standard.no_metadata, 1, "C7: has_metadata = 0");
        assert_eq!(standard.total_bytes, 6000);
        assert_eq!(standard.images_bytes, 3000);
        assert_eq!(standard.videos_bytes, 3000);
        assert_eq!(standard.hidden, 0, "a hidden row never counts as in-scope");
        assert_eq!(standard.device_count, 2);
        assert_eq!(standard.top_devices[0].name, "PC");
        assert_eq!(standard.top_devices[0].count, 2);
        assert_eq!(standard.top_devices[0].bytes, 3000);
    }

    #[test]
    fn stats_never_interpolates_the_scope_value() {
        // §6.6: the scope is bound as a parameter, so a vault row cannot leak
        // into a standard query by any path.
        let dir = tempfile::tempdir().unwrap();
        let pool = open(&booted(dir.path())).unwrap();
        let conn = pool.get().unwrap();
        seed_media(&conn);

        let standard = stats(&pool, MediaScope::Standard).unwrap();
        let vault = stats(&pool, MediaScope::Vault).unwrap();

        assert_eq!(standard.total_items, 3);
        assert_eq!(vault.total_items, 1);
        assert_eq!(vault.images, 1);
        assert_eq!(vault.total_bytes, 4000);
        assert!(standard.top_devices.iter().all(|d| d.name != "v1.1"));
    }

    #[test]
    fn vault_probe_reports_a_single_configured_flag() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open(&booted(dir.path())).unwrap();
        assert_eq!(
            vault_probe(&pool).unwrap(),
            VaultStatusDto {
                configured: false,
                unlocked: false,
                has_recovery_hint: false
            }
        );

        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO vault_config (id, password_hash, recovery_key_hash) VALUES (1, 'h', 'r')",
            [],
        )
        .unwrap();
        let status = vault_probe(&pool).unwrap();
        assert!(status.configured);
        assert!(!status.unlocked, "c4 has no session; c18 owns unlock");
    }

    #[test]
    fn the_hidden_column_is_scope_prunable() {
        // §6.3 rationale: the default scope filters is_hidden = 0, the vault
        // needs is_hidden = 1. Both must be answerable from the index.
        let dir = tempfile::tempdir().unwrap();
        let pool = open(&booted(dir.path())).unwrap();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "INSERT INTO media (id, relative_path, thumb_path, is_hidden) VALUES
               ('m1', 'a.jpg', 'a.webp', 0),
               ('m2', 'b.jpg', 'b.webp', 1);",
        )
        .unwrap();

        let plan = conn
            .prepare("EXPLAIN QUERY PLAN SELECT id FROM media WHERE is_hidden = 0")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join(" ");

        assert!(
            plan.contains("idx_media_hidden"),
            "planner ignored the index: {plan}"
        );
    }
}
