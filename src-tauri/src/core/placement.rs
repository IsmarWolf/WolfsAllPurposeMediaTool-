//! PLAN §7.7/§7.8 — the one place a **staged** file becomes a library file.
//!
//! Both transports need the identical ending: Wi-Fi renames a received multipart
//! part, USB renames a file pulled off an MTP device. The rules that ending obeys
//! are §14's data-safety rules, so they are written once here instead of twice:
//!
//! 1. Reject an unsupported type **before** any metadata read.
//! 2. Sanitize the device label and the file name (§5.4). A phone's `filename`
//!    is attacker-controlled and historically arrived as `C:\fakepath\IMG_1.jpg`.
//! 3. Choose the folder by **C7** — `<YYYY>/<MM>/` only when the file has
//!    metadata of its own, `Sem_Metadados/` otherwise.
//! 4. Pick a free destination on the **disk** (§7.3.1 decision: an interrupted
//!    pass may have moved files without writing rows).
//! 5. `fs::rename` — same volume, because staging lives under the same root.
//!
//! On any failure the staged file is the caller's to delete and the source still
//! has the original, so nothing is lost (§14). `place` never deletes anything and
//! never touches the source device: §12.1 orders a move as
//! write → hash → DB insert → *then* the PTP delete, which happens in the
//! transport, after the §7.3 pass, never here.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Datelike;

use crate::core::exif::{self, RawMetadata};
use crate::core::paths::{self, NO_META_DIR, PathResolver};
use crate::core::scanner;
use crate::error::AppError;

/// What a successful placement produced. `bytes` is read back from the target
/// after the rename, so the transport's counters report what actually landed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedFile {
    /// Library-relative, forward-slashed (§5.5).
    pub relative: String,
    pub bytes: u64,
}

/// The resolver plus the optional ffmpeg path — the two inputs §5/§6 extraction
/// needs. Borrowed, never resolved here: §7.4.1's rule is that a module never
/// resolves the root itself.
#[derive(Debug, Clone, Copy)]
pub struct Placement<'a> {
    resolver: &'a PathResolver,
    ffmpeg: Option<&'a Path>,
}

impl<'a> Placement<'a> {
    pub fn new(resolver: &'a PathResolver, ffmpeg: Option<&'a Path>) -> Self {
        Self { resolver, ffmpeg }
    }

    /// The moment of truth: staged bytes become a library file.
    ///
    /// `device_raw` is whatever the transport knows the device is called and
    /// `raw_name` whatever it was last called — both are sanitized here, because
    /// "sanitize at the edge" is exactly the assumption that a phone, a browser
    /// form and a camera would each get wrong differently.
    pub fn place(
        &self,
        device_raw: &str,
        raw_name: &str,
        staged: &Path,
    ) -> Result<PlacedFile, AppError> {
        let label = sanitize_device_label(device_raw)?;
        let name = safe_file_name(raw_name)?;

        if !is_media(&name) {
            return Err(AppError::Ingest(format!("tipo não suportado: {name}")));
        }

        let metadata = exif::extract(staged, self.ffmpeg).ok();
        let directory = target_dir(self.resolver, &label, metadata.as_ref())?;
        fs::create_dir_all(&directory).map_err(|error| AppError::Ingest(error.to_string()))?;

        let target = scanner::free_destination(&directory.join(&name))?;
        fs::rename(staged, &target).map_err(|error| AppError::Ingest(error.to_string()))?;

        let relative = self.resolver.abs_to_rel(&target)?;
        let bytes = fs::metadata(&target).map(|meta| meta.len()).unwrap_or(0);
        Ok(PlacedFile { relative, bytes })
    }
}

/// §5.4's device label, with the same fallback chain the transports used: prefer
/// the dedicated rule, and never fail outright just because a phone reported an
/// exotic name. `sanitize_label` is the general-purpose last resort.
pub fn sanitize_device_label(raw: &str) -> Result<String, AppError> {
    paths::sanitize_device_label(raw).or_else(|_| Ok(paths::sanitize_label(raw)))
}

/// §7.8/§5.4: `<YYYY>/<MM>/` for a file with metadata of its own,
/// `Sem_Metadados/` otherwise — C7's single definition, the same one the scanner
/// uses. A file placed in the wrong folder self-heals: §7.3's organize step
/// renames it on the next pass.
pub fn target_dir(
    resolver: &PathResolver,
    label: &str,
    metadata: Option<&RawMetadata>,
) -> Result<PathBuf, AppError> {
    let device = resolver.media_device(label)?;
    let dated = metadata
        .filter(|meta| meta.has_metadata())
        .and_then(|meta| {
            meta.captured_at
                .map(|captured| (captured.year(), captured.month()))
        });
    match dated {
        Some((year, month)) => Ok(device.join(year.to_string()).join(format!("{month:02}"))),
        None => Ok(device.join(NO_META_DIR)),
    }
}

/// A transport-supplied name is untrusted: only the last path component is ever
/// used, and §5.4's Windows rules still have the last word.
pub fn safe_file_name(raw: &str) -> Result<String, AppError> {
    let base = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(raw)
        .trim()
        .trim_start_matches("\u{feff}")
        .trim();
    if base.is_empty() || base == "." || base == ".." {
        return Err(AppError::Conflict("nome de arquivo vazio".into()));
    }
    paths::ensure_safe_component(base)?;
    Ok(base.to_string())
}

pub fn extension_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// The extension test is authoritative, **not** the device's own content type.
/// A Kindle reports an `.m3u` playlist as `Unspecified`, and plenty of devices
/// label a plain `.jpg` as a generic file, so `ObjectType` cannot be trusted to
/// decide what is media.
pub fn is_media(name: &str) -> bool {
    let extension = extension_of(name);
    exif::IMAGE_EXTS.contains(&extension.as_str()) || exif::VIDEO_EXTS.contains(&extension.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::layout;
    use crate::core::paths::RootSource;

    /// The same three lines every core test starts with: a temp volume, a
    /// resolver bound to it, and the §5.9 tree.
    fn temp_resolver(dir: &Path) -> PathResolver {
        let resolver = PathResolver::new(dir.to_path_buf(), RootSource::MarkerWalk);
        layout::ensure(&resolver).expect("a temp tree");
        resolver
    }

    #[test]
    fn a_media_file_with_no_metadata_lands_in_sem_metadados_c7() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = temp_resolver(dir.path());
        let staged = dir.path().join("staged.jpg");
        fs::write(&staged, b"not really a jpeg").unwrap();

        let placed = Placement::new(&resolver, None)
            .place("iPhone", "IMG_0001.jpg", &staged)
            .expect("a supported extension is placed");

        // C7: an unreadable file has no metadata of its own, so it never gets a
        // dated folder - the mtime must not lift it out of Sem_Metadados/.
        assert!(
            placed.relative.contains("/Sem_Metadados/IMG_0001.jpg"),
            "{placed:?}"
        );
        assert_eq!(placed.bytes, b"not really a jpeg".len() as u64);
        assert!(!staged.exists(), "the staged file was renamed, not copied");
    }

    #[test]
    fn an_unsupported_type_is_rejected_before_anything_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = temp_resolver(dir.path());
        let staged = dir.path().join("notes.txt");
        fs::write(&staged, b"hello").unwrap();

        let error = Placement::new(&resolver, None)
            .place("iPhone", "notes.txt", &staged)
            .expect_err("txt is not media");

        assert!(error.to_string().contains("tipo não suportado"), "{error}");
        assert!(
            staged.exists(),
            "a rejection leaves the staged file for the caller"
        );
        assert!(
            !resolver.media_device("iPhone").unwrap().exists(),
            "and creates no Media/ directory"
        );
    }

    /// The name a phone reports is the one thing a transport cannot be trusted
    /// about, so §5.4 runs on every path a transport could hand us.
    #[test]
    fn traversal_and_reserved_names_are_refused() {
        assert_eq!(
            safe_file_name("C:\\fakepath\\IMG_0001.jpg").unwrap(),
            "IMG_0001.jpg"
        );
        assert_eq!(
            safe_file_name("/var/mobile/../a/IMG 2.jpg").unwrap(),
            "IMG 2.jpg"
        );
        assert!(safe_file_name("").is_err());
        assert!(safe_file_name("..").is_err());
        assert!(
            safe_file_name("CON.jpg").is_err(),
            "§5.4 reserved device name"
        );
        assert!(
            safe_file_name("a/b:c.jpg").is_err(),
            "§5.4 illegal character"
        );
    }

    #[test]
    fn media_is_decided_by_extension_never_by_the_device_content_type() {
        assert!(is_media("IMG_0001.JPG"), "case-insensitive");
        assert!(is_media("clip.mov"));
        assert!(!is_media("playlist.m3u"));
        assert!(!is_media("noextension"));
    }
}
