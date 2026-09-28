//! The single error surface of the IPC layer (PLAN §8.2, §8.4).
//!
//! Domain modules keep their own typed errors (`PathError`, `DbError`,
//! `LayoutError`); this module is the only place that maps them to the stable
//! `E_*` codes the frontend can branch on. Commands return `Result<T, AppError>`
//! and Tauri serializes it as `{ code, message }` (§10.4).

use serde::{Serialize, Serializer};
use thiserror::Error;

use crate::core::db::DbError;
use crate::core::layout::LayoutError;
use crate::core::paths::PathError;

#[derive(Debug, Error)]
pub enum AppError {
    /// SQLite failed.
    #[error("{0}")]
    Db(#[from] DbError),
    /// The root is missing, not writable, or a stored path escapes it (E_ROOT).
    #[error("{0}")]
    Root(#[from] PathError),
    /// The tree could not be created, or `.vault` lost its hidden attributes.
    #[error("{0}")]
    Layout(#[from] LayoutError),
    /// ffmpeg missing or failed: toast + disable video/HEIC paths.
    #[error("{0}")]
    Ffmpeg(String),
    /// WPD/PTP failure: reconnect hint.
    #[error("{0}")]
    Device(String),
    /// Storage full / path conflict, surfaced with detail.
    #[error("{0}")]
    Conflict(String),
    /// Wrong vault password or recovery key: inline error on the gate.
    #[error("{0}")]
    Auth(String),
    /// Ingest job error: the job row shows the message.
    #[error("{0}")]
    Ingest(String),
    /// User cancelled: silent in the UI.
    #[error("cancelled")]
    Cancelled,
    /// The UI is not ready yet (called before boot completed).
    #[error("{0}")]
    Unavailable(String),
}

impl AppError {
    /// The stable code of §8.2. This is the only thing the frontend switches on.
    pub fn code(&self) -> &'static str {
        match self {
            AppError::Db(_) => "E_DB",
            AppError::Root(_) | AppError::Layout(_) => "E_ROOT",
            AppError::Ffmpeg(_) => "E_FFMPEG",
            AppError::Device(_) => "E_DEVICE",
            AppError::Conflict(_) => "E_CONFLICT",
            AppError::Auth(_) => "E_AUTH",
            AppError::Ingest(_) => "E_INGEST",
            AppError::Cancelled => "E_CANCEL",
            AppError::Unavailable(_) => "E_UNAVAILABLE",
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("AppError", 2)?;
        state.serialize_field("code", self.code())?;
        state.serialize_field("message", &self.to_string())?;
        state.end()
    }
}

/// Failed to spawn a helper process (Explorer, ffmpeg, `attrib`).
#[derive(Debug, Error)]
#[error("could not run {program}: {source}")]
pub struct SpawnError {
    pub program: String,
    #[source]
    pub source: std::io::Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(AppError::Cancelled.code(), "E_CANCEL");
        assert_eq!(AppError::Db(DbError::Pool("x".into())).code(), "E_DB");
        assert_eq!(AppError::Root(PathError::IsRoot).code(), "E_ROOT");
        assert_eq!(
            AppError::Layout(LayoutError::VaultAttributes("x".into())).code(),
            "E_ROOT",
            "a broken vault attribute is a root problem, not a new code"
        );
        assert_eq!(AppError::Ffmpeg("no ffmpeg".into()).code(), "E_FFMPEG");
        assert_eq!(AppError::Auth("senha".into()).code(), "E_AUTH");
    }

    #[test]
    fn serializes_as_code_and_message() {
        let json = serde_json::to_string(&AppError::Cancelled).unwrap();
        assert_eq!(json, r#"{"code":"E_CANCEL","message":"cancelled"}"#);

        let json =
            serde_json::to_string(&AppError::Db(DbError::Pool("sem conexão".into()))).unwrap();
        assert!(json.contains(r#""code":"E_DB""#), "{json}");
        assert!(json.contains("sem conexão"), "{json}");
    }
}
