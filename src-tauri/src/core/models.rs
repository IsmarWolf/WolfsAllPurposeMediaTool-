//! Serde DTOs shared by core and commands (PLAN §7.1, §8.3).
//!
//! Two rules hold here: DTO fields mirror DB columns 1:1, and **no media DTO
//! ever carries an absolute path** — only `/`-joined relative values. The single
//! exception is [`RootInfoDto`], which exists to *display* the resolved root in
//! Settings → Origem (§5.6) and is never persisted.

use serde::{Deserialize, Serialize};

use crate::core::paths::RootSource;

/// §10.6.1 `FilterSpec` — the single filter object shared by the gallery and the
/// snapshot builder (§12.4). Every field is `Option` so an absent filter is
/// "no constraint", not "match nothing".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterSpec {
    /// §6.6: the scope is always explicit — a query cannot be written without it.
    pub scope: MediaScope,
    /// Device label filter (`media.device_name`).
    pub device: Option<String>,
    /// City names (exact match, case-insensitive).
    pub cities: Vec<String>,
    /// `YYYY-MM` month filter on `captured_at`.
    pub month: Option<String>,
    /// C7: only files with `has_metadata = 0`.
    pub no_metadata: bool,
    /// Fuzzy subsequence match on filename (relative_path).
    pub search: Option<String>,
    /// `"image"` or `"video"`.
    pub file_type: Option<String>,
    /// Sort order: `capturedDesc` | `capturedAsc` | `name` | `size`.
    pub sort: String,
    /// Page size (default 240, max 1000).
    pub limit: i64,
    /// Offset for pagination.
    pub offset: i64,
}

/// §8.1 `media_query` — one page of gallery results.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaQueryDto {
    pub items: Vec<MediaDto>,
    pub total: i64,
    pub has_more: bool,
}

/// §6.6: the hidden-media scope rule. Every read-side entry point takes one, so
/// a gallery query cannot be written without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaScope {
    /// Standard scope ⟺ `is_hidden = 0`.
    #[default]
    Standard,
    /// Vault scope ⟺ `is_hidden = 1`. Only reachable in Vault Mode (C6).
    Vault,
}

impl MediaScope {
    /// The value bound to `is_hidden`. Always a **parameter** (§6.5) — the scope
    /// never reaches SQL as an interpolated literal.
    pub fn hidden_value(self) -> bool {
        matches!(self, MediaScope::Vault)
    }
}

impl Serialize for MediaScope {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match self {
            MediaScope::Standard => "standard",
            MediaScope::Vault => "vault",
        })
    }
}

/// Totals for the Dashboard stat cards and the Zone 4 summary (§7.2 `stats`).
#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StatsDto {
    pub total_items: i64,
    pub images: i64,
    pub videos: i64,
    /// C7: `has_metadata = 0` — no date **and** no GPS.
    pub no_metadata: i64,
    pub hidden: i64,
    pub total_bytes: i64,
    pub images_bytes: i64,
    pub videos_bytes: i64,
    pub device_count: i64,
    pub top_devices: Vec<DeviceCountDto>,
}

/// Mini-bars of Zone 4 summary / C-1k (top devices by item count).
#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCountDto {
    pub name: String,
    pub count: i64,
    pub bytes: i64,
}

/// §8.1 `vault_status`. `unlocked` is a session flag owned by c18; c4 always
/// reports `false` and only `configured` comes from the DB.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatusDto {
    pub configured: bool,
    pub unlocked: bool,
    pub has_recovery_hint: bool,
}

/// §7.5: the offline reverse-geocode answer for one media item.
///
/// `latitude`/`longitude` are the **media's** own coordinates, not the city's
/// centroid — the `locations` row is keyed by `media_id` (§6.1), so this pair is
/// the position of the photo. See the c7 decision in PLAN §7.5.1.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GeoDto {
    pub city: String,
    /// GeoNames `admin1` as the **code** the index was built with (`27` for São
    /// Paulo), because §7.5.1 decision 12 keeps codes over names on the SSD. A
    /// country with no admin1 round-trips as empty, never as a missing city.
    pub state: String,
    /// ISO country code (`BR`), resolved to a name only if a future index
    /// builder ships the country table alongside it.
    pub country: String,
    pub latitude: f64,
    pub longitude: f64,
}

/// The `locations` half of §7.1's `LocationDto` — the §7.5 geocode answer as it
/// is *persisted*, keyed by `media_id` (§6.1). `Option` fields mirror nullable
/// columns; `city: None` is a real state (§7.5: GPS without a city index, or a
/// point further than 40 km from any city), not a missing row.
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocationDto {
    pub city: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

/// §7.1 `MediaDto` — one gallery row, and the §12.3 lightbox. The two thumbnail
/// fields are both exposed even though §6.7 stores only the 400px one: the 200px
/// path is derived from `thumb400` by the §4.1 folder convention, so the DTO
/// carries the derivation instead of making every reader repeat it.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaDto {
    pub id: String,
    pub relative_path: String,
    pub thumb_200: String,
    pub thumb_400: String,
    pub file_hash: String,
    pub file_size: i64,
    pub file_type: String,
    /// §6.1 stores a naive local `DATETIME` (c6 decision 3), serialized without a
    /// zone suffix so the UI cannot invent one.
    pub captured_at: Option<String>,
    pub has_metadata: bool,
    pub device_name: Option<String>,
    pub is_hidden: bool,
    pub location: Option<LocationDto>,
}

/// §7.3 `scan(...) -> ScanSummary`, the value `scan_start` resolves with (§8.1).
///
/// The counts are the whole point: a scan that says "done" without saying how many
/// files it found, indexed, skipped and organized is indistinguishable from a
/// scan that silently did nothing. `no_metadata` is reported separately from
/// `organized` because the `Sem_Metadados/` split (§7.3 step 5) is the rule the
/// human is most likely to be checking after a drop.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    /// The device whose folder was walked, as stored in `media.device_name`.
    pub device: String,
    /// Media files seen by the walker (after the extension filter).
    pub found: u32,
    /// Rows inserted into `media` by this scan.
    pub inserted: u32,
    /// Files whose sha256 is already indexed — §7.3 step 2's "dedupe by hash
    /// wins". Includes files already organized, so a re-scan reports most of the
    /// library here.
    pub duplicates: u32,
    /// Rows whose `relative_path` was repaired because the file had been moved
    /// **inside** `Media/` by hand and the old path no longer existed. The same
    /// bytes, one row, a pointer that is right again (§7.3.1 decision 3).
    pub repaired: u32,
    /// Files moved into `<YYYY>/<MM>/` or `Sem_Metadados/` by the mop-up rule.
    pub organized: u32,
    /// Of the inserted rows, those with `has_metadata = 0` (C7) — i.e. the ones
    /// that landed in `Sem_Metadados/`.
    pub no_metadata: u32,
    /// Files with GPS that produced a `locations` row (§7.5 step 6).
    pub located: u32,
    /// Files with GPS and no city answer. **Not** an error: §7.5 keeps the
    /// coordinates and shows no city.
    pub located_none: u32,
    /// Files skipped for a reason that is not a duplicate (unreadable, unhashable,
    /// a path that cannot be represented). The scan continues past each one.
    pub skipped: u32,
    /// True when the walk was stopped by `scan_cancel` (§9.3).
    pub cancelled: bool,
}

/// The `wolfs://progress/scan` payload (§9.2: `{ current, total, lastPath }`).
/// `total` is 0 until the walk finishes collecting candidates — a folder walk
/// cannot know its size in advance, so the UI shows an indeterminate bar first
/// and switches to a real one once `total` arrives.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgressDto {
    pub current: u32,
    pub total: u32,
    pub last_path: String,
    /// Files inserted so far, so the row can show "N salvos" next to the bar.
    pub inserted: u32,
}

/// The `wolfs://thumb/progress` payload (§9.2's `{ mediaId, ok }`): one event
/// per media as its pair of WebP slots lands — or fails — so the gallery lights
/// up tile by tile instead of refetching the whole grid.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThumbProgressDto {
    pub media_id: String,
    /// Both slots exist (or the row was already rendered). `false` means a
    /// `thumb_meta` row now explains the missing tile.
    pub ok: bool,
}

/// The `wolfs://thumb/done` payload: fired once when a queue drains (§7.6).
/// §9.2 spells the wildcard channel as `{ mediaId, ok }` — that is the per-item
/// shape of `progress`; `done` is the batch verdict the rebuild UI shows.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThumbSummaryDto {
    /// Rows handed to the renderer, skipped ones included.
    pub processed: u32,
    /// Rows that ended with a `thumb_meta` failure.
    pub failed: u32,
}

/// §8.1 `root_info` + what Settings → Origem shows (§5.6).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RootInfoDto {
    pub root: String,
    pub source: RootSource,
    pub rooted: bool,
    /// The tree had to be created on this boot (§5.9 `is_first_run`).
    pub first_run: bool,
    pub ffmpeg_ok: bool,
}

impl Serialize for RootSource {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match self {
            RootSource::WolfsRootEnv => "wolfsRootEnv",
            RootSource::MarkerWalk => "markerWalk",
            RootSource::PackagedAppDir => "packagedAppDir",
            RootSource::UnrootedFallback => "unrootedFallback",
        })
    }
}

/// §5.6 `[Recalcular Raiz]`: the same info plus whether the root actually moved
/// (which is when the tree and the pool were rebuilt).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RecalcRootDto {
    pub changed: bool,
    #[serde(flatten)]
    pub info: RootInfoDto,
}

/// The Sistema card of §11.7 (app version, OS, arch). Added in c4 — see the
/// §8.1 note: §11.7 needs it and the catalog had no command for it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppVersionsDto {
    pub app_version: &'static str,
    pub os: &'static str,
    pub arch: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dtos_are_camel_case() {
        let stats = StatsDto {
            no_metadata: 7,
            total_bytes: 12,
            ..StatsDto::default()
        };
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("\"noMetadata\":7"), "{json}");
        assert!(json.contains("\"totalBytes\":12"), "{json}");
        assert!(json.contains("\"topDevices\":[]"), "{json}");
    }

    #[test]
    fn root_source_serializes_as_a_tag() {
        let json = serde_json::to_string(&RootSource::UnrootedFallback).unwrap();
        assert_eq!(json, "\"unrootedFallback\"");
    }

    /// Every arm the frontend union in `src/types/api.ts` depends on. A new
    /// variant that forgets its tag would silently show as an unknown source.
    #[test]
    fn every_root_source_has_the_tag_the_frontend_expects() {
        for (source, expected) in [
            (RootSource::WolfsRootEnv, "\"wolfsRootEnv\""),
            (RootSource::MarkerWalk, "\"markerWalk\""),
            (RootSource::PackagedAppDir, "\"packagedAppDir\""),
            (RootSource::UnrootedFallback, "\"unrootedFallback\""),
        ] {
            assert_eq!(serde_json::to_string(&source).unwrap(), expected);
        }
    }

    #[test]
    fn geo_dto_is_camel_case_and_keeps_the_media_point() {
        // §7.5.1 decision 4: the coordinates are the photo's, so the lightbox
        // can show where the shot was without re-reading the EXIF. The `27` is
        // the admin1 code §7.5.1 decision 12 chose to keep, not a state name.
        let geo = GeoDto {
            city: "São Paulo".into(),
            state: "27".into(),
            country: "BR".into(),
            latitude: -23.5105,
            longitude: -46.6033,
        };
        let json = serde_json::to_string(&geo).unwrap();
        assert!(json.contains(r#""city":"São Paulo""#), "{json}");
        assert!(json.contains(r#""state":"27""#), "{json}");
        assert!(json.contains(r#""country":"BR""#), "{json}");
        assert!(json.contains(r#""latitude":-23.5105"#), "{json}");
        assert!(json.contains(r#""longitude":-46.6033"#), "{json}");
    }

    #[test]
    fn scope_binds_the_hidden_flag() {
        assert!(!MediaScope::Standard.hidden_value());
        assert!(MediaScope::Vault.hidden_value());
    }
}
