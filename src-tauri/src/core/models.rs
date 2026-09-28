//! Serde DTOs shared by core and commands (PLAN §7.1, §8.3).
//!
//! Two rules hold here: DTO fields mirror DB columns 1:1, and **no media DTO
//! ever carries an absolute path** — only `/`-joined relative values. The single
//! exception is [`RootInfoDto`], which exists to *display* the resolved root in
//! Settings → Origem (§5.6) and is never persisted.

use serde::Serialize;

use crate::core::paths::RootSource;

/// §6.6: the hidden-media scope rule. Every read-side entry point takes one, so
/// a gallery query cannot be written without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaScope {
    /// Standard scope ⟺ `is_hidden = 0`.
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

/// §8.1 `resolve_app_root` + what Settings → Origem shows (§5.6).
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

    #[test]
    fn scope_binds_the_hidden_flag() {
        assert!(!MediaScope::Standard.hidden_value());
        assert!(MediaScope::Vault.hidden_value());
    }
}
