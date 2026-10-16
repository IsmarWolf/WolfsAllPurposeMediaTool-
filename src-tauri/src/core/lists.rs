//! Lists (PLAN §7.9, §12.4, c15) — manual membership and smart snapshots.
//!
//! A list is one row of §6.1's `lists` table plus rows in `list_items`, and its
//! **kind is `filter_criteria`**: `NULL` is a **manual** list (the user adds and
//! removes media by hand), non-null JSON is a **smart snapshot** whose membership
//! is a snapshot of a stored [`FilterSpec`]. This module owns the *semantics* —
//! which operations each kind allows, and the C6 rule that a list is a normal
//! surface that can never see hidden media. The SQL is `core::db`'s job (§6.5).
//!
//! Snapshot semantics (§7.9/§12.4):
//! * `snapshot_apply` runs the *given* spec now and **replaces** membership,
//!   storing the criteria. A manual list becomes smart this way; applying again
//!   overwrites the criteria and the members.
//! * `snapshot_sync` re-runs the *stored* criteria and re-syncs membership. A
//!   manual list (no criteria) has nothing to re-run and is refused.
//! * Both force `scope = standard` before they query or store, so a list can
//!   never capture vault media (§12.5.6, C6).

use serde_json::Value;
use thiserror::Error;

use crate::core::db::{self, DbError, SqlitePool};
use crate::core::models::{FilterSpec, ListDetailDto, ListDto, MediaScope};

/// A list operation that failed for a reason the UI must tell apart (§8.2: the
/// codes are assigned in `error.rs`, the only module that maps to `E_*`).
#[derive(Debug, Error)]
pub enum ListError {
    #[error("{0}")]
    Db(#[from] DbError),
    #[error("lista não encontrada: {0}")]
    NotFound(String),
    #[error("esta lista é smart; use [Sincronizar] ou [Aplicar critérios]")]
    SmartList,
    #[error("esta lista é manual (sem critérios) e não tem o que sincronizar")]
    ManualList,
    #[error("o nome da lista não pode ficar vazio")]
    EmptyName,
    #[error("critérios de lista inválidos: {0}")]
    Criteria(String),
}

/// §7.9 — all lists, sidebar order.
pub fn list(pool: &SqlitePool) -> Result<Vec<ListDto>, ListError> {
    db::lists_all(pool)?.into_iter().map(to_dto).collect()
}

/// §7.9 — one list with its members, or `None` when it does not exist.
pub fn detail(pool: &SqlitePool, id: &str) -> Result<Option<ListDetailDto>, ListError> {
    let Some(stored) = db::list_get(pool, id)? else {
        return Ok(None);
    };
    let members = db::list_members(pool, id)?;
    Ok(Some(ListDetailDto {
        list: to_dto(stored)?,
        members,
    }))
}

/// §12.4 — create a list. Always **manual** (no criteria); a smart list is only
/// born from [`snapshot_apply`]. Returns the created list with its count.
pub fn create(pool: &SqlitePool, name: &str) -> Result<ListDto, ListError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ListError::EmptyName);
    }
    let id = uuid::Uuid::new_v4().to_string();
    db::list_insert(pool, &id, name)?;
    let stored = db::list_get(pool, &id)?.ok_or_else(|| ListError::NotFound(id.clone()))?;
    to_dto(stored)
}

/// §8.1 `lists_update` — rename only (c15 decision). Criteria change through
/// [`snapshot_apply`], so exactly one path owns them.
pub fn rename(pool: &SqlitePool, id: &str, name: &str) -> Result<(), ListError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ListError::EmptyName);
    }
    if !db::list_update_name(pool, id, name)? {
        return Err(ListError::NotFound(id.to_string()));
    }
    Ok(())
}

/// §8.1 `lists_delete` — delete a list and its membership (cascade).
pub fn delete(pool: &SqlitePool, id: &str) -> Result<(), ListError> {
    if !db::list_delete(pool, id)? {
        return Err(ListError::NotFound(id.to_string()));
    }
    Ok(())
}

/// §7.9 — add media to a **manual** list. A smart list's membership is derived
/// from its criteria and is refused (`SmartList`).
pub fn add_items(pool: &SqlitePool, list_id: &str, media_ids: &[String]) -> Result<(), ListError> {
    require_manual(pool, list_id)?;
    db::list_add_items(pool, list_id, media_ids)?;
    Ok(())
}

/// §7.9 — remove media from a **manual** list. Smart lists are refused, exactly
/// as for [`add_items`].
pub fn remove_items(
    pool: &SqlitePool,
    list_id: &str,
    media_ids: &[String],
) -> Result<(), ListError> {
    require_manual(pool, list_id)?;
    db::list_remove_items(pool, list_id, media_ids)?;
    Ok(())
}

/// §7.9/§12.4 — apply the active filter to a list: store the criteria and
/// **replace** membership with the current matches.
///
/// The scope is forced to `standard` (C6): whatever the caller sent, a list is
/// never allowed to see hidden media, so the stored criteria can only produce
/// visible members. On a list that already has criteria this overwrites both.
pub fn snapshot_apply(
    pool: &SqlitePool,
    list_id: &str,
    spec: &FilterSpec,
) -> Result<(), ListError> {
    if db::list_get(pool, list_id)?.is_none() {
        return Err(ListError::NotFound(list_id.to_string()));
    }

    let mut spec = spec.clone();
    spec.scope = MediaScope::Standard;
    let ids = db::media_ids_matching(pool, &spec)?;
    let criteria = serde_json::to_string(&spec).map_err(|e| ListError::Criteria(e.to_string()))?;

    if !db::list_apply_snapshot(pool, list_id, &criteria, &ids)? {
        return Err(ListError::NotFound(list_id.to_string()));
    }
    Ok(())
}

/// §7.9/§12.4 — re-run the **stored** criteria and re-sync membership.
///
/// A manual list has no criteria to re-run and is refused (`ManualList`): §12.4
/// "manual lists never sync". The stored spec is re-normalized to `standard` in
/// case an older row ever stored something else.
pub fn snapshot_sync(pool: &SqlitePool, list_id: &str) -> Result<(), ListError> {
    let stored =
        db::list_get(pool, list_id)?.ok_or_else(|| ListError::NotFound(list_id.to_string()))?;
    let Some(json) = stored.criteria_json else {
        return Err(ListError::ManualList);
    };

    let mut spec: FilterSpec =
        serde_json::from_str(&json).map_err(|e| ListError::Criteria(e.to_string()))?;
    spec.scope = MediaScope::Standard;
    let ids = db::media_ids_matching(pool, &spec)?;

    if !db::list_replace_items(pool, list_id, &ids)? {
        return Err(ListError::NotFound(list_id.to_string()));
    }
    Ok(())
}

/// A list must exist and be **manual** (no criteria) for item edits.
fn require_manual(pool: &SqlitePool, list_id: &str) -> Result<(), ListError> {
    let stored =
        db::list_get(pool, list_id)?.ok_or_else(|| ListError::NotFound(list_id.to_string()))?;
    if stored.criteria_json.is_some() {
        return Err(ListError::SmartList);
    }
    Ok(())
}

/// The DB row → wire DTO, parsing the stored criteria back into JSON.
fn to_dto(stored: db::StoredList) -> Result<ListDto, ListError> {
    let filter_criteria = stored
        .criteria_json
        .map(|json| {
            serde_json::from_str::<Value>(&json).map_err(|e| ListError::Criteria(e.to_string()))
        })
        .transpose()?;
    Ok(ListDto {
        id: stored.id,
        name: stored.name,
        filter_criteria,
        created_at: stored.created_at.unwrap_or_default(),
        member_count: stored.member_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::db;
    use crate::core::models::MediaScope;
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
        captured_at: Option<&str>,
        has_metadata: bool,
        is_hidden: bool,
    ) {
        conn.execute(
            "INSERT INTO media (id, relative_path, thumb_path, file_hash, file_size, file_type,
                                captured_at, added_at, has_metadata, is_hidden, device_name)
             VALUES (?1, ?2, ?3, ?4, 1000, ?5, ?6, '2026-06-01T09:00:00', ?7, ?8, 'PC')",
            rusqlite::params![
                id,
                path,
                format!("Thumbnails/400/{id}.webp"),
                format!("hash-{id}"),
                file_type,
                captured_at,
                has_metadata,
                is_hidden,
            ],
        )
        .unwrap();
    }

    /// A gallery that separates visible from hidden and image from video.
    fn seed(conn: &Connection) {
        insert(
            conn,
            "a1",
            "Media/PC/2024/03/IMG_1.jpg",
            "image",
            Some("2024-03-15 10:00:00"),
            true,
            false,
        );
        insert(
            conn,
            "a2",
            "Media/PC/2024/04/VID_2.mov",
            "video",
            Some("2024-04-01 10:00:00"),
            true,
            false,
        );
        insert(
            conn,
            "a3",
            "Media/PC/Sem_Metadados/bare.jpg",
            "image",
            None,
            false,
            false,
        );
        // The vault row: never visible to a list (C6).
        insert(
            conn,
            "h1",
            "Media/PC/2024/03/secret.jpg",
            "image",
            Some("2024-03-20 10:00:00"),
            true,
            true,
        );
    }

    fn spec_image() -> FilterSpec {
        FilterSpec {
            scope: MediaScope::Standard,
            file_type: Some("image".into()),
            sort: "capturedDesc".into(),
            limit: 240,
            offset: 0,
            ..FilterSpec::default()
        }
    }

    fn member_ids(pool: &SqlitePool, list_id: &str) -> Vec<String> {
        let mut ids: Vec<String> = detail(pool, list_id)
            .unwrap()
            .unwrap()
            .members
            .into_iter()
            .map(|m| m.id)
            .collect();
        ids.sort();
        ids
    }

    #[test]
    fn manual_list_add_and_remove_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open_pool(dir.path());
        seed(&pool.get().unwrap());

        let created = create(&pool, "Favoritas").unwrap();
        assert_eq!(created.member_count, 0);
        assert!(created.filter_criteria.is_none(), "a new list is manual");

        add_items(&pool, &created.id, &["a1".into(), "a2".into()]).unwrap();
        assert_eq!(member_ids(&pool, &created.id), vec!["a1", "a2"]);
        assert_eq!(
            detail(&pool, &created.id)
                .unwrap()
                .unwrap()
                .list
                .member_count,
            2
        );

        remove_items(&pool, &created.id, &["a1".into()]).unwrap();
        assert_eq!(member_ids(&pool, &created.id), vec!["a2"]);
    }

    #[test]
    fn creating_a_list_with_a_blank_name_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open_pool(dir.path());

        assert!(matches!(create(&pool, "   "), Err(ListError::EmptyName)));
        assert!(matches!(create(&pool, ""), Err(ListError::EmptyName)));
        // The name is trimmed, not stored with padding.
        let created = create(&pool, "  Viagem  ").unwrap();
        assert_eq!(created.name, "Viagem");
    }

    #[test]
    fn rename_and_delete_report_a_missing_list() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open_pool(dir.path());

        assert!(matches!(
            rename(&pool, "nope", "x"),
            Err(ListError::NotFound(_))
        ));
        assert!(matches!(delete(&pool, "nope"), Err(ListError::NotFound(_))));

        let created = create(&pool, "Antiga").unwrap();
        rename(&pool, &created.id, "Nova").unwrap();
        assert_eq!(list(&pool).unwrap()[0].name, "Nova");
        delete(&pool, &created.id).unwrap();
        assert!(list(&pool).unwrap().is_empty());
    }

    #[test]
    fn add_and_remove_items_are_refused_on_a_smart_list() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open_pool(dir.path());
        seed(&pool.get().unwrap());

        let created = create(&pool, "Imagens").unwrap();
        snapshot_apply(&pool, &created.id, &spec_image()).unwrap();

        assert!(matches!(
            add_items(&pool, &created.id, &["a2".into()]),
            Err(ListError::SmartList)
        ));
        assert!(matches!(
            remove_items(&pool, &created.id, &["a1".into()]),
            Err(ListError::SmartList)
        ));
    }

    #[test]
    fn sync_is_refused_on_a_manual_list() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open_pool(dir.path());
        seed(&pool.get().unwrap());

        let created = create(&pool, "Manual").unwrap();
        assert!(matches!(
            snapshot_sync(&pool, &created.id),
            Err(ListError::ManualList)
        ));
    }

    #[test]
    fn snapshot_apply_stores_criteria_and_replaces_membership() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open_pool(dir.path());
        seed(&pool.get().unwrap());

        let created = create(&pool, "Imagens").unwrap();
        snapshot_apply(&pool, &created.id, &spec_image()).unwrap();

        // Images only: a1 and the no-metadata a3, never the video.
        assert_eq!(member_ids(&pool, &created.id), vec!["a1", "a3"]);
        let dto = detail(&pool, &created.id).unwrap().unwrap().list;
        assert!(dto.filter_criteria.is_some(), "apply makes it a smart list");

        // Apply a different spec: criteria AND members are overwritten.
        let video = FilterSpec {
            file_type: Some("video".into()),
            ..spec_image()
        };
        snapshot_apply(&pool, &created.id, &video).unwrap();
        assert_eq!(member_ids(&pool, &created.id), vec!["a2"]);
        let dto = detail(&pool, &created.id).unwrap().unwrap().list;
        assert_eq!(dto.filter_criteria.unwrap()["fileType"], "video");
    }

    #[test]
    fn snapshot_apply_forces_the_standard_scope() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open_pool(dir.path());
        seed(&pool.get().unwrap());

        let created = create(&pool, "Imagens").unwrap();

        // A caller cannot smuggle vault media in by asking for the vault scope…
        let vault_spec = FilterSpec {
            scope: MediaScope::Vault,
            file_type: Some("image".into()),
            ..spec_image()
        };
        snapshot_apply(&pool, &created.id, &vault_spec).unwrap();

        // …the hidden h1 is absent, and the stored criteria say `standard`.
        assert_eq!(member_ids(&pool, &created.id), vec!["a1", "a3"]);
        let dto = detail(&pool, &created.id).unwrap().unwrap().list;
        assert_eq!(dto.filter_criteria.unwrap()["scope"], "standard");
    }

    #[test]
    fn snapshot_sync_adds_new_matches_and_drops_stale_members() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open_pool(dir.path());
        seed(&pool.get().unwrap());

        let created = create(&pool, "Imagens").unwrap();
        snapshot_apply(&pool, &created.id, &spec_image()).unwrap();
        assert_eq!(member_ids(&pool, &created.id), vec!["a1", "a3"]);

        // A new image arrives, and an existing member stops matching.
        let conn = pool.get().unwrap();
        insert(
            &conn,
            "a4",
            "Media/PC/2024/05/IMG_4.jpg",
            "image",
            Some("2024-05-01 10:00:00"),
            true,
            false,
        );
        conn.execute("UPDATE media SET is_hidden = 1 WHERE id = 'a1'", [])
            .unwrap();

        snapshot_sync(&pool, &created.id).unwrap();
        assert_eq!(member_ids(&pool, &created.id), vec!["a3", "a4"]);
    }

    /// The c15 gate: criteria persist across a restart. A smart list's stored
    /// JSON and its membership must both survive the pool being closed and
    /// reopened on the same file.
    #[test]
    fn snapshot_criteria_and_members_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();

        let created_id = {
            let pool = open_pool(dir.path());
            seed(&pool.get().unwrap());
            let created = create(&pool, "Imagens").unwrap();
            snapshot_apply(&pool, &created.id, &spec_image()).unwrap();
            assert_eq!(member_ids(&pool, &created.id), vec!["a1", "a3"]);
            created.id
        };

        let pool = open_pool(dir.path());
        let lists = list(&pool).unwrap();
        assert_eq!(lists.len(), 1);

        // Criteria survived: still a smart list, still `standard`.
        let criteria = lists[0].filter_criteria.as_ref().unwrap();
        assert_eq!(criteria["fileType"], "image");
        assert_eq!(criteria["scope"], "standard");

        // Membership survived too — not just the JSON.
        assert_eq!(member_ids(&pool, &created_id), vec!["a1", "a3"]);

        // And sync still knows it is smart (no ManualList error).
        snapshot_sync(&pool, &created_id).unwrap();
    }

    #[test]
    fn deleting_a_list_cascades_its_items() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open_pool(dir.path());
        seed(&pool.get().unwrap());

        let created = create(&pool, "Favoritas").unwrap();
        add_items(&pool, &created.id, &["a1".into(), "a2".into()]).unwrap();

        delete(&pool, &created.id).unwrap();

        let remaining: i64 = pool
            .get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM list_items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn deleting_media_removes_it_from_lists() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open_pool(dir.path());
        seed(&pool.get().unwrap());

        let created = create(&pool, "Favoritas").unwrap();
        add_items(&pool, &created.id, &["a1".into(), "a2".into()]).unwrap();

        db::media_remove(&pool, &["a1".into()]).unwrap();

        assert_eq!(member_ids(&pool, &created.id), vec!["a2"]);
        assert_eq!(
            detail(&pool, &created.id)
                .unwrap()
                .unwrap()
                .list
                .member_count,
            1
        );
    }

    #[test]
    fn hidden_media_cannot_be_added_and_disappears_when_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open_pool(dir.path());
        seed(&pool.get().unwrap());

        let created = create(&pool, "Favoritas").unwrap();

        // h1 is already hidden: adding it changes nothing (C6 at the write side).
        add_items(&pool, &created.id, &["h1".into()]).unwrap();
        assert!(member_ids(&pool, &created.id).is_empty());

        // a1 is visible when added, then hidden → it vanishes from the list.
        add_items(&pool, &created.id, &["a1".into(), "a2".into()]).unwrap();
        assert_eq!(member_ids(&pool, &created.id), vec!["a1", "a2"]);
        pool.get()
            .unwrap()
            .execute("UPDATE media SET is_hidden = 1 WHERE id = 'a1'", [])
            .unwrap();
        assert_eq!(member_ids(&pool, &created.id), vec!["a2"]);
        assert_eq!(
            detail(&pool, &created.id)
                .unwrap()
                .unwrap()
                .list
                .member_count,
            1
        );
    }

    #[test]
    fn lists_all_is_alphabetical_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open_pool(dir.path());

        for name in ["zeta", "Alpha", "beta"] {
            create(&pool, name).unwrap();
        }
        let names: Vec<String> = list(&pool).unwrap().into_iter().map(|l| l.name).collect();
        assert_eq!(names, vec!["Alpha", "beta", "zeta"]);
    }
}
