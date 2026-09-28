//! Fresh-boot layout creation (PLAN §5.5).
//!
//! Creates, in order: `Database/`, `Thumbnails/200`, `Thumbnails/400`, `Media/PC/`,
//! `.vault/items`, `.vault/thumbs`, `App/`, then applies `attrib +h +s` to `.vault`.
//! Idempotent: an existing tree is validated, never rebuilt, and never touched
//! beyond the vault attributes.
//!
//! `PathResolver` (PLAN §5.1) remains the only module that knows the root: this
//! module only composes accessors, never literal paths.

use std::path::{Path, PathBuf};
use std::process::Command;

use thiserror::Error;

use crate::core::paths::{PC_DEVICE_LABEL, PathError, PathResolver, ThumbSize};

#[derive(Debug, Error)]
pub enum LayoutError {
    #[error("i/o error creating {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not set hidden+system attributes on {0}")]
    VaultAttributes(String),
    #[error(transparent)]
    Paths(#[from] PathError),
}

/// What `ensure` did, so the boot log (§14) can tell "first run" from "re-run".
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LayoutReport {
    /// Directories created by this call, in creation order.
    pub created: Vec<PathBuf>,
    /// Directories that already existed, in the §5.5 order.
    pub existed: Vec<PathBuf>,
    /// `App/bin/ffmpeg.exe` is present (§5.5 records this in `AppState` in c4).
    pub ffmpeg_present: bool,
    /// `attrib +h +s` confirmed on `.vault`.
    pub vault_hidden: bool,
}

impl LayoutReport {
    /// First boot on this device: the tree did not exist yet.
    pub fn is_first_run(&self) -> bool {
        !self.created.is_empty()
    }
}

pub fn ensure(resolver: &PathResolver) -> Result<LayoutReport, LayoutError> {
    let mut report = LayoutReport::default();

    // §5.5 order. The root itself is assumed to exist (PathResolver found the
    // marker there, or the unrooted fallback is the exe folder, which exists).
    let dirs = [
        resolver.db_dir(),
        resolver.thumb_dir(ThumbSize::Small),
        resolver.thumb_dir(ThumbSize::Large),
        resolver.media_device(PC_DEVICE_LABEL)?,
        resolver.vault_items(),
        resolver.vault_thumbs(),
        resolver.app_dir(),
    ];

    for dir in dirs {
        ensure_dir(&dir, &mut report)?;
    }

    report.vault_hidden = set_hidden_system(&resolver.vault_root())?;
    report.ffmpeg_present = resolver.ffmpeg_exe().is_file();
    Ok(report)
}

fn ensure_dir(path: &Path, report: &mut LayoutReport) -> Result<(), LayoutError> {
    if path.is_dir() {
        report.existed.push(path.to_path_buf());
        return Ok(());
    }
    std::fs::create_dir_all(path).map_err(|source| LayoutError::Io {
        path: path.display().to_string(),
        source,
    })?;
    report.created.push(path.to_path_buf());
    Ok(())
}

/// `attrib +h +s` (§5.5). The deploy script (§16.1 step 5) uses the same tool, so
/// runtime and packaging agree without a `windows` crate dependency.
fn set_hidden_system(path: &Path) -> Result<bool, LayoutError> {
    let output = Command::new("attrib")
        .arg("+h")
        .arg("+s")
        .arg(path)
        .output()
        .map_err(|source| LayoutError::Io {
            path: path.display().to_string(),
            source,
        })?;
    Ok(output.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::paths::{PC_DEVICE_LABEL, PathResolver, RootSource};
    use std::fs;
    use std::path::Path;

    #[cfg(windows)]
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;

    fn resolver_at(root: &Path) -> PathResolver {
        PathResolver::new(root.to_path_buf(), RootSource::MarkerWalk)
    }

    fn media_pc(resolver: &PathResolver) -> PathBuf {
        resolver.media_device(PC_DEVICE_LABEL).unwrap()
    }

    /// Root-relative, "/"-joined (the §5.4 persisted-path contract).
    fn rel_str(root: &Path, path: &Path) -> String {
        path.strip_prefix(root)
            .unwrap()
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }

    #[test]
    fn creates_the_5_5_tree_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = resolver_at(dir.path());

        let report = ensure(&resolver).unwrap();

        assert!(
            report.is_first_run(),
            "fresh root must count as a first run"
        );
        assert!(report.existed.is_empty());
        let created: Vec<String> = report
            .created
            .iter()
            .map(|p| rel_str(dir.path(), p))
            .collect();
        assert_eq!(
            created,
            [
                "Database",
                "Thumbnails/200",
                "Thumbnails/400",
                "Media/PC",
                ".vault/items",
                ".vault/thumbs",
                "App",
            ],
            "creation order is the §5.5 order"
        );
        for path in &report.created {
            assert!(path.is_dir(), "{path:?} must be a directory");
        }
        assert!(!report.ffmpeg_present, "no ffmpeg on a fresh tree");
    }

    #[test]
    fn rerun_is_idempotent_and_preserves_content() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = resolver_at(dir.path());
        ensure(&resolver).unwrap();

        let sentinel = media_pc(&resolver).join("minha foto.jpg");
        fs::write(&sentinel, b"conteudo").unwrap();

        let report = ensure(&resolver).unwrap();

        assert!(!report.is_first_run(), "second boot is not a first run");
        assert!(report.created.is_empty());
        assert_eq!(report.existed.len(), 7);
        assert_eq!(
            fs::read(&sentinel).unwrap(),
            b"conteudo",
            "media is never touched"
        );
    }

    #[test]
    fn ffmpeg_is_detected_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = resolver_at(dir.path());
        ensure(&resolver).unwrap();
        assert!(!ensure(&resolver).unwrap().ffmpeg_present);

        // The deploy script (§16.1) installs ffmpeg into App/bin/; §5.5 creates
        // only App/, so the test installs it the same way.
        fs::create_dir_all(resolver.app_bin()).unwrap();
        fs::write(resolver.ffmpeg_exe(), b"MZ").unwrap();
        assert!(ensure(&resolver).unwrap().ffmpeg_present);
    }

    #[cfg(windows)]
    #[test]
    fn vault_gets_hidden_and_system_attributes() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = resolver_at(dir.path());

        let report = ensure(&resolver).unwrap();
        assert!(report.vault_hidden, "attrib must have succeeded");

        let attrs = fs::metadata(resolver.vault_root())
            .unwrap()
            .file_attributes();
        assert_ne!(attrs & FILE_ATTRIBUTE_HIDDEN, 0, "vault must be hidden");
        assert_ne!(attrs & FILE_ATTRIBUTE_SYSTEM, 0, "vault must be system");
    }

    #[cfg(windows)]
    #[test]
    fn attributes_survive_a_rerun() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = resolver_at(dir.path());
        ensure(&resolver).unwrap();

        // A user (or a sync tool) clearing the attributes must not break the boot.
        Command::new("attrib")
            .arg("-h")
            .arg("-s")
            .arg(resolver.vault_root())
            .output()
            .unwrap();

        assert!(ensure(&resolver).unwrap().vault_hidden);
        let attrs = fs::metadata(resolver.vault_root())
            .unwrap()
            .file_attributes();
        assert_ne!(attrs & FILE_ATTRIBUTE_HIDDEN, 0);
        assert_ne!(attrs & FILE_ATTRIBUTE_SYSTEM, 0);
    }

    /// The tree the deploy script declares, read straight out of the `.ps1`.
    /// A PowerShell array cannot be shared with Rust, so the two lists are kept
    /// equal by *parsing* rather than by convention.
    fn deploy_script_trees() -> Vec<String> {
        let script_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scripts/deploy-portable.ps1"
        );
        let script = fs::read_to_string(script_path)
            .unwrap_or_else(|e| panic!("cannot read the deploy script at {script_path}: {e}"));

        let marker = "$Script:Trees = @(";
        let start = script
            .find(marker)
            .expect("deploy script must declare $Script:Trees")
            + marker.len();
        let body = &script[start..];
        let end = body.find(')').expect("unterminated $Script:Trees array");
        body[..end]
            .lines()
            .map(|line| line.trim().trim_matches('\'').trim())
            .filter(|line| !line.is_empty())
            .map(|line| line.to_string())
            .collect()
    }

    /// §16.1 step 4 says the deploy script creates "the same tree" as this
    /// module. If a folder is added, renamed or reordered on one side, this
    /// fails.
    #[test]
    fn deploy_script_tree_matches_layout() {
        let declared = deploy_script_trees();

        // Built from the very accessors `ensure` uses, relative to the root and
        // "/" -joined, so the comparison is on the §5.4 relative-path contract.
        let root = Path::new("X:\\ssd");
        let resolver = PathResolver::new(root.to_path_buf(), RootSource::MarkerWalk);
        let expected = [
            resolver.db_dir(),
            resolver.thumb_dir(ThumbSize::Small),
            resolver.thumb_dir(ThumbSize::Large),
            resolver.media_device(PC_DEVICE_LABEL).unwrap(),
            resolver.vault_items(),
            resolver.vault_thumbs(),
            resolver.app_dir(),
        ]
        .iter()
        .map(|p| {
            p.strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect::<Vec<_>>();

        assert_eq!(
            declared, expected,
            "scripts/deploy-portable.ps1 must create exactly the §5.5 order of core/layout.rs"
        );
    }

    ///     /// The deploy script and the resolver must agree on where a fresh drive
    /// lives: §16.1 step 6 seeds `Database/app.db`, and §5.2 step 2 finds it
    /// from `App/`. This is the full first-boot path in miniature - a drive that
    /// only the script has ever touched must resolve to the drive, never to
    /// `App/` inside it.
    #[cfg(windows)]
    #[test]
    fn a_script_deployed_drive_resolves_to_its_own_root() {
        let root = tempfile::tempdir().unwrap();

        // The script's tree, then its marker (mirrors Initialize-Marker).
        for relative in deploy_script_trees() {
            fs::create_dir_all(root.path().join(relative.replace('/', "\\"))).unwrap();
        }
        let marker = root.path().join("Database").join("app.db");
        fs::write(&marker, b"").unwrap();

        // The exe where step 2 installs it.
        let app_dir = root.path().join("App");
        fs::write(app_dir.join(crate::core::paths::EXE_NAME), b"MZ").unwrap();

        let resolver = PathResolver::resolve(&app_dir, None);
        assert_eq!(
            resolver.root(),
            root.path(),
            "the deployed drive itself is the root"
        );
        assert_eq!(resolver.source(), RootSource::MarkerWalk);
        assert!(
            crate::core::paths::has_marker(resolver.root()),
            "§4.3 marker present after deploy"
        );

        // And with the marker gone, the packaged rule must not let it fall back
        // into App/: that is the bug this pairing exists to prevent.
        fs::remove_file(&marker).unwrap();
        let without_marker = PathResolver::resolve(&app_dir, None);
        assert_eq!(without_marker.root(), root.path());
        assert_eq!(without_marker.source(), RootSource::PackagedAppDir);
    }

    #[cfg(windows)]
    #[test]
    fn only_the_vault_is_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = resolver_at(dir.path());
        ensure(&resolver).unwrap();

        for public in [resolver.media_root(), resolver.db_dir(), resolver.app_dir()] {
            let attrs = fs::metadata(&public).unwrap().file_attributes();
            assert_eq!(
                attrs & FILE_ATTRIBUTE_HIDDEN,
                0,
                "{public:?} must stay visible"
            );
        }
    }
}
