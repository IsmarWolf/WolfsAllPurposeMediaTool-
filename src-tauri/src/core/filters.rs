//! `FilterSpec` -> parameterized SQL (PLAN §7 module map, §13.2 c14).
//!
//! The gallery and the snapshot builder (§12.4) share one filter object
//! ([`FilterSpec`]), and this module is the single place that turns it into SQL.
//! Two properties are load-bearing and pinned by tests:
//!
//! * **Every user value is a bound parameter.** No field of the spec is ever
//!   interpolated into a statement, so a device label or a search string cannot
//!   change the query's shape (see `the_builder_never_interpolates_a_user_value`).
//! * **The scope is always the first parameter** (§6.6), so a gallery query
//!   cannot be written without deciding what to do with hidden media.
//!
//! `LIMIT`/`OFFSET` are deliberately *not* part of a [`FilterSql`]: the caller
//! binds them last so the same compiled filter drives both the `COUNT(*)` and
//! the page, and the two can never disagree about what is being counted.

use rusqlite::ToSql;
use serde::Serialize;

use crate::core::models::FilterSpec;

/// A compiled filter: the `WHERE` clauses, their bound parameters **in order**,
/// and the `ORDER BY` body (without the keyword). Parameter placeholders are
/// numbered `?1`, `?2`, … matching [`FilterSql::params`].
pub struct FilterSql {
    pub clauses: Vec<String>,
    pub params: Vec<Box<dyn ToSql>>,
    pub order: String,
}

impl FilterSql {
    /// The `WHERE` body to drop after `WHERE` — the clauses joined with `AND`.
    pub fn where_sql(&self) -> String {
        self.clauses.join(" AND ")
    }
}

/// Compile a [`FilterSpec`] into a [`FilterSql`].
///
/// The scope is bound first (§6.6); an absent filter adds no clause, so an empty
/// spec is just the scope.
pub fn build(spec: &FilterSpec) -> FilterSql {
    let mut clauses: Vec<String> = vec!["m.is_hidden = ?1".to_string()];
    let mut params: Vec<Box<dyn ToSql>> = vec![Box::new(spec.scope.hidden_value())];

    if let Some(device) = &spec.device {
        clauses.push(format!("m.device_name = ?{}", params.len() + 1));
        params.push(Box::new(device.clone()));
    }

    if !spec.cities.is_empty() {
        let placeholders: Vec<String> = (0..spec.cities.len())
            .map(|i| format!("?{}", params.len() + i + 1))
            .collect();
        clauses.push(format!("LOWER(l.city) IN ({})", placeholders.join(", ")));
        for city in &spec.cities {
            params.push(Box::new(city.to_lowercase()));
        }
    }

    if let Some(month) = &spec.month {
        clauses.push(format!(
            "strftime('%Y-%m', m.captured_at) = ?{}",
            params.len() + 1
        ));
        params.push(Box::new(month.clone()));
    }

    if spec.no_metadata {
        clauses.push("m.has_metadata = 0".to_string());
    }

    if let Some(search) = &spec.search {
        // Fuzzy subsequence on the *filename* (§10.6.1), not the whole path:
        // a whole-path match lets "IMG" hit `Sem_Metadados/bare.jpg` through
        // `...m...g(.jpg)`. The filename is the tail after the last `/`
        // (`RTRIM` strips the trailing non-slash run, `REPLACE` drops the head).
        // User input is escaped first, then the wildcards are added.
        let escaped: Vec<String> = search
            .chars()
            .map(|c| match c {
                '\\' => "\\\\".to_string(),
                '%' => "\\%".to_string(),
                '_' => "\\_".to_string(),
                _ => c.to_string(),
            })
            .collect();
        clauses.push(format!(
            "LOWER(REPLACE(m.relative_path, RTRIM(m.relative_path, REPLACE(m.relative_path, '/', '')), '')) LIKE ?{} ESCAPE '\\'",
            params.len() + 1
        ));
        params.push(Box::new(format!("%{}%", escaped.join("%").to_lowercase())));
    }

    if let Some(file_type) = &spec.file_type {
        clauses.push(format!("m.file_type = ?{}", params.len() + 1));
        params.push(Box::new(file_type.clone()));
    }

    let order = match spec.sort.as_str() {
        "capturedAsc" => "m.captured_at ASC",
        "name" => "m.relative_path ASC",
        "size" => "m.file_size DESC",
        // v2 sort: most recently *imported* first. Rows from before the column
        // existed carry NULL and sort after every dated row (`IS NULL` first key),
        // never pretending a legacy import happened "just now".
        "addedDesc" => "m.added_at IS NULL, m.added_at DESC",
        _ => "m.captured_at DESC",
    }
    .to_string();

    FilterSql {
        clauses,
        params,
        order,
    }
}

/// One active filter, ready to become a chip or a line of a snapshot's
/// criteria summary (§10.6.1, §11.2).
///
/// This is deliberately **locale-free**: `field` is a stable key and `value` the
/// raw filter value, so the frontend owns the label text through i18n exactly
/// like the rest of the UI. `value` is `None` for boolean filters (no-metadata),
/// which have no value to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedFilter {
    pub field: String,
    pub value: Option<String>,
}

/// The filters that are actually active on a spec, in chip order: search,
/// device, cities, month, file type, then no-metadata.
///
/// A boolean filter with no value is still a chip — that is the point of
/// returning it here instead of letting each caller re-derive "is this one on?".
pub fn applied(spec: &FilterSpec) -> Vec<AppliedFilter> {
    let mut out = Vec::new();

    if let Some(search) = &spec.search
        && !search.is_empty()
    {
        out.push(AppliedFilter {
            field: "search".to_string(),
            value: Some(search.clone()),
        });
    }
    if let Some(device) = &spec.device {
        out.push(AppliedFilter {
            field: "device".to_string(),
            value: Some(device.clone()),
        });
    }
    for city in &spec.cities {
        out.push(AppliedFilter {
            field: "city".to_string(),
            value: Some(city.clone()),
        });
    }
    if let Some(month) = &spec.month {
        out.push(AppliedFilter {
            field: "month".to_string(),
            value: Some(month.clone()),
        });
    }
    if let Some(file_type) = &spec.file_type {
        out.push(AppliedFilter {
            field: "fileType".to_string(),
            value: Some(file_type.clone()),
        });
    }
    if spec.no_metadata {
        out.push(AppliedFilter {
            field: "noMetadata".to_string(),
            value: None,
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::db::{self, SqlitePool};
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

    /// A small gallery that separates every filter axis: two devices, two
    /// months, a located row, a bare row and a hidden row.
    fn seed(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO media (id, relative_path, thumb_path, file_hash, file_size, file_type, captured_at, added_at, has_metadata, is_hidden, device_name) VALUES
               ('g1', 'Media/PC/2024/03/IMG_1.jpg',  'Thumbnails/400/PC/2024/03/IMG_1.webp',  'h1', 1000, 'image', '2024-03-15 10:00:00', '2026-06-01T09:00:00', 1, 0, 'PC'),
               ('g2', 'Media/iPhone/2024/04/VID_2.mov','Thumbnails/400/iPhone/2024/04/VID_2.webp','h2', 2000, 'video', '2024-04-01 10:00:00', '2026-06-02T09:00:00', 1, 0, 'iPhone'),
               ('g3', 'Media/PC/Sem_Metadados/bare.jpg','Thumbnails/400/PC/Sem_Metadados/bare.webp','h3', 500, 'image', NULL, NULL, 0, 0, 'PC'),
               ('g4', 'Media/PC/2024/03/hidden.jpg', 'Thumbnails/400/PC/2024/03/hidden.webp', 'h4', 700, 'image', '2024-03-20 10:00:00', '2026-06-03T09:00:00', 1, 1, 'PC');
             INSERT INTO locations (media_id, city, state, country, latitude, longitude) VALUES
               ('g1', 'Sao Paulo', 'SP', 'BR', -23.5, -46.6);",
        )
        .unwrap();
    }

    fn ids(pool: &SqlitePool, spec: FilterSpec) -> Vec<String> {
        let mut rows: Vec<String> = db::media_query(pool, &spec)
            .unwrap()
            .items
            .into_iter()
            .map(|item| item.id)
            .collect();
        rows.sort();
        rows
    }

    fn spec() -> FilterSpec {
        FilterSpec {
            scope: MediaScope::Standard,
            sort: "capturedDesc".into(),
            limit: 240,
            offset: 0,
            ..FilterSpec::default()
        }
    }

    /// The wire contract is §22's flat object with camelCase keys — the shape the
    /// frontend already sends. Re-reading what we write is the only check that a
    /// field added to the struct also round-trips.
    #[test]
    fn filter_spec_round_trips_through_json_in_the_22_shape() {
        let spec = FilterSpec {
            scope: MediaScope::Vault,
            device: Some("iPhone".into()),
            cities: vec!["São Paulo".into()],
            month: Some("2024-05".into()),
            no_metadata: true,
            search: Some("IMG_45".into()),
            file_type: Some("image".into()),
            sort: "name".into(),
            limit: 50,
            offset: 10,
        };

        let json = serde_json::to_string(&spec).unwrap();
        let back: FilterSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back, spec);

        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["scope"], "vault");
        assert_eq!(value["device"], "iPhone");
        assert_eq!(value["cities"][0], "São Paulo");
        assert_eq!(value["month"], "2024-05");
        assert_eq!(value["noMetadata"], true);
        assert_eq!(value["fileType"], "image");
        assert_eq!(value["sort"], "name");
        assert_eq!(value["limit"], 50);
        assert_eq!(value["offset"], 10);
    }

    /// A hostile device label must stay data. If it ever reached the `WHERE`
    /// body, the clause count would change; here the SQL is fixed and the value
    /// is bound at `?2`.
    #[test]
    fn the_builder_never_interpolates_a_user_value() {
        let hostile = FilterSpec {
            device: Some("'; DROP TABLE media; --".into()),
            ..spec()
        };
        let sql = build(&hostile);

        assert!(
            !sql.where_sql().contains("DROP TABLE"),
            "the value leaked into the SQL text"
        );
        assert_eq!(sql.clauses.len(), 2, "scope plus the device clause");
        assert_eq!(sql.params.len(), 2);
        assert!(sql.where_sql().ends_with("m.device_name = ?2"));
    }

    /// The count is a *scope-bound* count and the labels cover exactly the
    /// active filters — nothing is invented for an absent one.
    #[test]
    fn applied_filters_lists_exactly_the_active_ones() {
        assert!(applied(&spec()).is_empty());

        let full = FilterSpec {
            search: Some("IMG".into()),
            device: Some("PC".into()),
            cities: vec!["Sao Paulo".into(), "Recife".into()],
            month: Some("2024-03".into()),
            no_metadata: true,
            file_type: Some("image".into()),
            ..spec()
        };
        let chips = applied(&full);

        assert_eq!(
            chips,
            vec![
                AppliedFilter {
                    field: "search".into(),
                    value: Some("IMG".into())
                },
                AppliedFilter {
                    field: "device".into(),
                    value: Some("PC".into())
                },
                AppliedFilter {
                    field: "city".into(),
                    value: Some("Sao Paulo".into())
                },
                AppliedFilter {
                    field: "city".into(),
                    value: Some("Recife".into())
                },
                AppliedFilter {
                    field: "month".into(),
                    value: Some("2024-03".into())
                },
                AppliedFilter {
                    field: "fileType".into(),
                    value: Some("image".into())
                },
                AppliedFilter {
                    field: "noMetadata".into(),
                    value: None
                },
            ]
        );

        // An empty search string is not a chip.
        let blank = FilterSpec {
            search: Some(String::new()),
            ..spec()
        };
        assert!(applied(&blank).is_empty());
    }

    /// The c14 gate: every operator returns the exact row set, and the scope
    /// still hides what it must. One seeded gallery, one assertion per axis.
    #[test]
    fn operator_and_date_filters_return_exact_row_sets() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open_pool(dir.path());
        seed(&pool.get().unwrap());

        // Scope first: standard hides g4, vault is exactly g4.
        assert_eq!(ids(&pool, spec()), vec!["g1", "g2", "g3"]);
        assert_eq!(
            ids(
                &pool,
                FilterSpec {
                    scope: MediaScope::Vault,
                    ..spec()
                }
            ),
            vec!["g4"]
        );

        // Device.
        assert_eq!(
            ids(
                &pool,
                FilterSpec {
                    device: Some("iPhone".into()),
                    ..spec()
                }
            ),
            vec!["g2"]
        );

        // Date (month).
        assert_eq!(
            ids(
                &pool,
                FilterSpec {
                    month: Some("2024-03".into()),
                    ..spec()
                }
            ),
            vec!["g1"]
        );

        // Type.
        assert_eq!(
            ids(
                &pool,
                FilterSpec {
                    file_type: Some("video".into()),
                    ..spec()
                }
            ),
            vec!["g2"]
        );

        // C7 "no metadata".
        assert_eq!(
            ids(
                &pool,
                FilterSpec {
                    no_metadata: true,
                    ..spec()
                }
            ),
            vec!["g3"]
        );

        // City, case-insensitive.
        assert_eq!(
            ids(
                &pool,
                FilterSpec {
                    cities: vec!["sao paulo".into()],
                    ..spec()
                }
            ),
            vec!["g1"]
        );

        // Search is a filename subsequence, not a path match: "IMG" hits g1,
        // never the bare row through "Sem_Metadados".
        assert_eq!(
            ids(
                &pool,
                FilterSpec {
                    search: Some("IMG".into()),
                    ..spec()
                }
            ),
            vec!["g1"]
        );

        // Paging reads the same count the page is drawn from.
        let page = db::media_query(
            &pool,
            &FilterSpec {
                limit: 2,
                offset: 0,
                ..spec()
            },
        )
        .unwrap();
        assert_eq!(page.total, 3);
        assert_eq!(page.items.len(), 2);
        assert!(page.has_more);
        assert_eq!(page.items[0].id, "g2", "capturedDesc: newest first");

        let tail = db::media_query(
            &pool,
            &FilterSpec {
                limit: 2,
                offset: 2,
                ..spec()
            },
        )
        .unwrap();
        assert_eq!(tail.items.len(), 1);
        assert!(!tail.has_more);
    }

    /// v2's `addedDesc` reads import time, not capture time: g2 was imported
    /// after g1 although it was captured before any of the others, and the
    /// pre-v2 row (g3, NULL) sinks below every dated row instead of pretending
    /// a legacy import happened "just now".
    #[test]
    fn added_desc_orders_by_import_time_with_nulls_last() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open_pool(dir.path());
        seed(&pool.get().unwrap());

        let page = db::media_query(
            &pool,
            &FilterSpec {
                sort: "addedDesc".into(),
                ..spec()
            },
        )
        .unwrap();
        let ids: Vec<String> = page.items.into_iter().map(|item| item.id).collect();
        assert_eq!(ids, vec!["g2", "g1", "g3"]);

        // The scope still wins: g4 is hidden and never surfaces under Vault-shy
        // sorts.
        let vault = db::media_query(
            &pool,
            &FilterSpec {
                scope: MediaScope::Vault,
                sort: "addedDesc".into(),
                ..spec()
            },
        )
        .unwrap();
        let vault_ids: Vec<String> = vault.items.into_iter().map(|item| item.id).collect();
        assert_eq!(vault_ids, vec!["g4"]);
    }
}
