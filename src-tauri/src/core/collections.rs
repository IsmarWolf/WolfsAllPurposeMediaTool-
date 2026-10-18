//! Smart collections (PLAN §7.2d, §24.1, c21) — the **derived** counterpart to
//! manual lists.
//!
//! A collection is never stored: it is one predicate over the gallery, compiled
//! through the same [`crate::core::filters`] every query uses, so a collection and
//! the gallery can never disagree about what a filter means (§7.2d). This module
//! owns the *semantics* — the five keys, the one predicate each, and the C6 rule
//! that a hidden item is never counted or shown (exactly like a list, §7.9).
//!
//! Running a collection is `db::media_query` with the predicate's spec pre-applied
//! (`collection_open`), so the scoped gallery reuses the only grid implementation
//! (§24.1 decision 3 — no second grid).

use thiserror::Error;

use crate::core::db::{self, DbError, SqlitePool};
use crate::core::models::{CollectionDto, FilterSpec, MediaQueryDto, MediaScope};

/// §24.3 — the screenshot vocabulary, locale-aware and conservative. Kept here,
/// next to the collection that owns it; `filters::build` splices the clause when a
/// spec sets `screenshot` (the c14 rule that `filters.rs` is the only spec -> SQL
/// orchestrator). The four terms already cover the `Screenshots/`/`Capturas/`
/// folders, but the folder forms are spelled out too because §24.3 names them
/// explicitly.
pub(crate) const SCREENSHOT_CLAUSE: &str = "(LOWER(m.relative_path) LIKE '%screenshot%' \
     OR LOWER(m.relative_path) LIKE '%screen shot%' \
     OR LOWER(m.relative_path) LIKE '%captura de tela%' \
     OR LOWER(m.relative_path) LIKE '%captura%' \
     OR LOWER(m.relative_path) LIKE '%/screenshots/%' \
     OR LOWER(m.relative_path) LIKE '%/capturas/%')";

/// The five collection keys, in card order (§11.10).
const KEYS: [&str; 5] = ["favorites", "screenshots", "videos", "documents", "people"];

/// §7.2d — how many cover ids a card carries.
const COVER_LIMIT: i64 = 4;

/// A collection operation that failed for a reason the UI must tell apart (§8.2:
/// `error.rs` owns the `E_*` mapping).
#[derive(Debug, Error)]
pub enum CollectionError {
    #[error("{0}")]
    Db(#[from] DbError),
    /// A collection whose predicate is not available in this phase (Documentos /
    /// Pessoas & Animais, §11.10 "disponível em breve") — or an unknown key.
    #[error("coleção indisponível: {0}")]
    Unavailable(String),
}

/// §7.2d `list(pool, scope)` — the five collections with a count and a cover.
///
/// The scope is bound like every read (§6.6); the shell passes `Standard`
/// (collections are a standard-scope surface, §24.1 decision 2/C6). A collection
/// whose predicate is not yet available still appears, with `count = 0` and no
/// cover, so the Coleções screen can render its disabled "em breve" card
/// (§11.10).
pub fn list(pool: &SqlitePool, scope: MediaScope) -> Result<Vec<CollectionDto>, CollectionError> {
    KEYS.iter()
        .map(|key| one(pool, key, scope))
        .collect::<Result<Vec<_>, _>>()
}

/// One collection card. A missing predicate is a zeroed card, not an error.
fn one(pool: &SqlitePool, key: &str, scope: MediaScope) -> Result<CollectionDto, CollectionError> {
    let label_key = format!("collections.{key}");
    let Some(mut spec) = predicate(key) else {
        return Ok(CollectionDto {
            key: key.to_string(),
            label_key,
            count: 0,
            cover_media_ids: Vec::new(),
        });
    };

    // The cover is the newest-added members (§7.2d: `media.added_at` supplies the
    // default cover order), and the page's `total` is the scoped match count.
    spec.scope = scope;
    spec.sort = "addedDesc".to_string();
    spec.limit = COVER_LIMIT;
    spec.offset = 0;

    let page = db::media_query(pool, &spec)?;
    Ok(CollectionDto {
        key: key.to_string(),
        label_key,
        count: page.total,
        cover_media_ids: page.items.into_iter().map(|item| item.id).collect(),
    })
}

/// §7.2d `predicate(key)` — the one place each collection's rule lives.
///
/// `None` for a collection whose predicate needs storage this phase does not have
/// (Documentos/Pessoas & Animais detect in Phase 8, §24.5/§24.6) and for an
/// unknown key.
pub fn predicate(key: &str) -> Option<FilterSpec> {
    let base = FilterSpec::default();
    match key {
        "favorites" => Some(FilterSpec {
            favorite: true,
            ..base
        }),
        "screenshots" => Some(FilterSpec {
            screenshot: true,
            ..base
        }),
        "videos" => Some(FilterSpec {
            file_type: Some("video".to_string()),
            ..base
        }),
        _ => None,
    }
}

/// §8.1 `collection_open` — run one collection through the gallery pipeline.
///
/// The collection's predicate is applied on top of the caller's spec, so opening
/// "Favoritos" can already carry a search or a device filter (§24.1 decision 3).
/// The scope is **forced to `standard`** (C6): a collection is a standard surface
/// and can never be pointed at the vault, exactly like a list (§7.9 decision 1).
pub fn open(
    pool: &SqlitePool,
    key: &str,
    spec: &FilterSpec,
) -> Result<MediaQueryDto, CollectionError> {
    let predicate = predicate(key).ok_or_else(|| CollectionError::Unavailable(key.to_string()))?;

    let mut merged = spec.clone();
    merged.scope = MediaScope::Standard;
    // The collection's own constraint always wins over a caller value.
    merged.favorite = predicate.favorite;
    merged.screenshot = predicate.screenshot;
    if predicate.file_type.is_some() {
        merged.file_type = predicate.file_type;
    }

    Ok(db::media_query(pool, &merged)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::db;
    use crate::core::paths::{PathResolver, RootSource};
    use rusqlite::Connection;
    use std::path::Path;

    fn resolver_at(root: &Path) -> PathResolver {
        PathResolver::new(root.to_path_buf(), RootSource::MarkerWalk)
    }

    /// The real boot order of §5.5: the tree first, then the database.
    fn booted(root: &Path) -> PathResolver {
        let resolver = resolver_at(root);
        crate::core::layout::ensure(&resolver).unwrap();
        resolver
    }

    fn open_pool(root: &Path) -> SqlitePool {
        db::open(&booted(root)).unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    fn insert(
        conn: &Connection,
        id: &str,
        path: &str,
        file_type: &str,
        is_favorite: bool,
        is_hidden: bool,
    ) {
        conn.execute(
            "INSERT INTO media (id, relative_path, thumb_path, file_hash, file_size, file_type,
                                captured_at, added_at, has_metadata, is_hidden, is_favorite, device_name)
             VALUES (?1, ?2, ?3, ?4, 1000, ?5, '2024-03-15 10:00:00', '2026-06-01T09:00:00', 1, ?6, ?7, 'PC')",
            rusqlite::params![
                id,
                path,
                format!("Thumbnails/400/{id}.webp"),
                format!("hash-{id}"),
                file_type,
                is_hidden,
                is_favorite,
            ],
        )
        .unwrap();
    }

    /// A gallery that separates the axes: a favorite image, a plain image, a
    /// screenshot, a video, a hidden favorite and a hidden image.
    fn seed(conn: &Connection) {
        insert(
            conn,
            "f1",
            "Media/PC/2024/03/IMG_1.jpg",
            "image",
            true,
            false,
        );
        insert(
            conn,
            "p1",
            "Media/PC/2024/03/IMG_2.jpg",
            "image",
            false,
            false,
        );
        insert(
            conn,
            "s1",
            "Media/iPhone/2024/03/Screenshot_2024.png",
            "image",
            false,
            false,
        );
        insert(
            conn,
            "v1",
            "Media/PC/2024/04/VID_2.mov",
            "video",
            false,
            false,
        );
        // The vault rows: never counted by a standard collection (C6).
        insert(
            conn,
            "hf",
            "Media/PC/2024/03/secret.jpg",
            "image",
            true,
            true,
        );
        insert(
            conn,
            "hs",
            "Media/PC/Screenshots/hidden.png",
            "image",
            false,
            true,
        );
    }

    fn ids(page: &MediaQueryDto) -> Vec<String> {
        let mut ids: Vec<String> = page.items.iter().map(|item| item.id.clone()).collect();
        ids.sort();
        ids
    }

    /// A caller spec with a real page size. `FilterSpec::default()` is `limit: 0`,
    /// which `media_query` clamps to 1, so a test that expects more than one row
    /// must set a limit — as the app does (`DEFAULT_SPEC`, §10.6.1).
    fn page_spec() -> FilterSpec {
        FilterSpec {
            limit: 240,
            ..FilterSpec::default()
        }
    }

    fn card<'a>(cards: &'a [CollectionDto], key: &str) -> &'a CollectionDto {
        cards.iter().find(|c| c.key == key).unwrap()
    }

    /// The c21 gate: the Favoritos/Capturas/Vídeos counts match a manual query,
    /// and the hidden rows are invisible to all of them (C6).
    #[test]
    fn live_collections_count_their_members_and_ignore_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open_pool(dir.path());
        seed(&pool.get().unwrap());

        let cards = list(&pool, MediaScope::Standard).unwrap();
        assert_eq!(cards.len(), 5, "all five cards are always listed");
        assert_eq!(card(&cards, "favorites").count, 1, "f1; hf is hidden");
        assert_eq!(card(&cards, "screenshots").count, 1, "s1; hs is hidden");
        assert_eq!(card(&cards, "videos").count, 1, "v1");

        // Same facts as a manual query, which is the gate's wording.
        let favorites = open(&pool, "favorites", &FilterSpec::default()).unwrap();
        assert_eq!(favorites.total, 1);
        assert_eq!(ids(&favorites), vec!["f1"]);
        let screenshots = open(&pool, "screenshots", &FilterSpec::default()).unwrap();
        assert_eq!(screenshots.total, 1);
        assert_eq!(ids(&screenshots), vec!["s1"]);
    }

    /// The cover is up to four recent members (`addedDesc`); a small collection
    /// simply carries what it has.
    #[test]
    fn a_cover_carries_up_to_four_member_ids() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open_pool(dir.path());
        let conn = pool.get().unwrap();
        for n in 0..6 {
            insert(
                &conn,
                &format!("f{n}"),
                &format!("Media/PC/2024/03/IMG_{n}.jpg"),
                "image",
                true,
                false,
            );
        }

        let cards = list(&pool, MediaScope::Standard).unwrap();
        let favorites = card(&cards, "favorites");
        assert_eq!(favorites.count, 6);
        assert_eq!(favorites.cover_media_ids.len(), 4);
    }

    /// Documentos / Pessoas & Animais appear (their cards exist) but carry no
    /// predicate and no count until Phase 8, and opening one is refused.
    #[test]
    fn unavailable_collections_list_as_empty_cards_and_cannot_open() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open_pool(dir.path());
        seed(&pool.get().unwrap());

        let cards = list(&pool, MediaScope::Standard).unwrap();
        for key in ["documents", "people"] {
            let c = card(&cards, key);
            assert_eq!(c.count, 0);
            assert!(c.cover_media_ids.is_empty());
            assert_eq!(c.label_key, format!("collections.{key}"));
            assert!(matches!(
                open(&pool, key, &FilterSpec::default()),
                Err(CollectionError::Unavailable(_))
            ));
        }
        assert!(predicate("nonsense").is_none());
    }

    /// C6/id-edge: `open` forces the standard scope, so a caller cannot smuggle the
    /// vault in by asking for it, exactly like `snapshot_apply` for lists.
    #[test]
    fn opening_a_collection_forces_the_standard_scope() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open_pool(dir.path());
        seed(&pool.get().unwrap());

        let spy = FilterSpec {
            scope: MediaScope::Vault,
            ..FilterSpec::default()
        };
        let page = open(&pool, "favorites", &spy).unwrap();
        // Only the visible favorite, never the hidden hf.
        assert_eq!(ids(&page), vec!["f1"]);
    }

    /// The caller's spec refines the collection: a search narrows Favoritos.
    #[test]
    fn the_callers_spec_refines_the_collection() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open_pool(dir.path());
        seed(&pool.get().unwrap());

        let narrowed = FilterSpec {
            search: Some("IMG_2".to_string()),
            ..FilterSpec::default()
        };
        // p1 matches the search but is not a favorite; the collection filter wins.
        assert_eq!(open(&pool, "favorites", &narrowed).unwrap().total, 0);

        let hit = FilterSpec {
            search: Some("IMG_1".to_string()),
            ..FilterSpec::default()
        };
        assert_eq!(ids(&open(&pool, "favorites", &hit).unwrap()), vec!["f1"]);
    }

    /// §24.3: the screenshot predicate is case-insensitive and matches both the
    /// vocabulary and a `Screenshots/` folder.
    #[test]
    fn screenshots_match_the_vocabulary_and_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open_pool(dir.path());
        let conn = pool.get().unwrap();
        insert(
            &conn,
            "a",
            "Media/PC/2024/03/screenshot_x.png",
            "image",
            false,
            false,
        );
        insert(
            &conn,
            "b",
            "Media/PC/2024/03/SCREEN SHOT y.png",
            "image",
            false,
            false,
        );
        insert(
            &conn,
            "c",
            "Media/PC/2024/03/Captura de tela z.png",
            "image",
            false,
            false,
        );
        insert(
            &conn,
            "d",
            "Media/iPhone/Screenshots/IMG_1.png",
            "image",
            false,
            false,
        );
        insert(
            &conn,
            "e",
            "Media/PC/2024/03/IMG_plain.jpg",
            "image",
            false,
            false,
        );

        let page = open(&pool, "screenshots", &page_spec()).unwrap();
        assert_eq!(ids(&page), vec!["a", "b", "c", "d"]);
    }
}
