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
        all.retain(|name| name != "process_log");
        all
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
