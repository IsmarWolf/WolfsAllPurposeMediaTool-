use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

pub const ENV_ROOT_VAR: &str = "WOLFS_ROOT";
pub const MARKER_DB: &str = "app.db";
pub const DATABASE_DIR: &str = "Database";
pub const GEONAMES_FILE: &str = "geonames.bin";
pub const MEDIA_DIR: &str = "Media";
pub const NO_META_DIR: &str = "Sem_Metadados";
pub const THUMBNAILS_DIR: &str = "Thumbnails";
pub const VAULT_DIR: &str = ".vault";
pub const VAULT_ITEMS_DIR: &str = "items";
pub const VAULT_THUMBS_DIR: &str = "thumbs";
pub const APP_DIR: &str = "App";
pub const APP_BIN_DIR: &str = "bin";
pub const FFMPEG_EXE_NAME: &str = "ffmpeg.exe";
/// The explicit `[[bin]] name` from §4.4, in production as in dev.
pub const EXE_NAME: &str = "BackupManager.exe";
pub const PC_DEVICE_LABEL: &str = "PC";
pub const FALLBACK_DEVICE_LABEL: &str = "Dispositivo";
pub const MAX_LABEL_LEN: usize = 64;

const ILLEGAL_CHARS: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];
const WINDOWS_RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootSource {
    WolfsRootEnv,
    MarkerWalk,
    /// The exe sits in an `App/` folder that holds the deployed binary, so the
    /// root is its parent - recognized by layout, not by the marker (§5.2 step
    /// 2b, added in c5). Rooted, but it is a *guess* about an unproven root, so
    /// the boot log must not claim a marker was found.
    PackagedAppDir,
    UnrootedFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbSize {
    Small,
    Large,
}

impl ThumbSize {
    pub fn px(self) -> u16 {
        match self {
            ThumbSize::Small => 200,
            ThumbSize::Large => 400,
        }
    }

    pub fn dir_name(self) -> String {
        self.px().to_string()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PathError {
    #[error("empty path component")]
    EmptyComponent,
    #[error("illegal characters in path component: {0}")]
    IllegalCharacters(String),
    #[error("windows reserved name: {0}")]
    ReservedName(String),
    #[error("reserved device label: {0}")]
    ReservedLabel(String),
    #[error("stored path is not relative: {0}")]
    NotRelative(String),
    #[error("path escapes the portable root: {0}")]
    EscapesRoot(String),
    #[error("path belongs to another volume: {0}")]
    ForeignVolume(String),
    #[error("path is the portable root itself")]
    IsRoot,
    /// The portable root itself is not a usable directory (a file sits there, or
    /// it cannot be written). Added in c4: the degraded boot needs to report it.
    #[error("portable root is not a writable directory: {0}")]
    RootNotADirectory(String),
}

#[derive(Debug, Clone)]
pub struct PathResolver {
    root: PathBuf,
    source: RootSource,
}

impl PathResolver {
    pub fn new(root: PathBuf, source: RootSource) -> Self {
        Self { root, source }
    }

    pub fn resolve_app_root() -> Self {
        let start = start_dir();
        Self::resolve(&start, std::env::var_os(ENV_ROOT_VAR).as_deref())
    }

    pub fn resolve(start: &Path, wolfs_root: Option<&OsStr>) -> Self {
        if let Some(root) = env_root(wolfs_root) {
            return Self::new(root, RootSource::WolfsRootEnv);
        }

        if let Some(found) = walk_up_for_marker(start) {
            return Self::new(found, RootSource::MarkerWalk);
        }

        // Step 2b (c5): a deployed root has no marker until its first boot, and
        // the fallback below would then build the whole tree one level too deep,
        // inside App/. Recognizing the packaged layout is not the "first ancestor
        // with a Media dir" guess the §5.2 rationale rejects: it requires the
        // *current exe* to sit in a folder literally named `App`, so an ordinary
        // directory can never trigger it.
        if let Some(found) = packaged_root(start) {
            return Self::new(found, RootSource::PackagedAppDir);
        }

        Self::new(start.to_path_buf(), RootSource::UnrootedFallback)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn source(&self) -> RootSource {
        self.source
    }

    pub fn is_rooted(&self) -> bool {
        self.source != RootSource::UnrootedFallback
    }

    pub fn db_path(&self) -> PathBuf {
        self.root.join(DATABASE_DIR).join(MARKER_DB)
    }

    /// `Database/` — the folder holding `app.db` (§5.5 boot step 1).
    pub fn db_dir(&self) -> PathBuf {
        self.root.join(DATABASE_DIR)
    }

    pub fn geonames_path(&self) -> PathBuf {
        self.root.join(DATABASE_DIR).join(GEONAMES_FILE)
    }

    pub fn media_root(&self) -> PathBuf {
        self.root.join(MEDIA_DIR)
    }

    pub fn media_device(&self, device: &str) -> Result<PathBuf, PathError> {
        let label = sanitize_label(device);
        ensure_safe_component(&label)?;
        Ok(self.media_root().join(label))
    }

    pub fn media_no_meta(&self, device: &str) -> Result<PathBuf, PathError> {
        Ok(self.media_device(device)?.join(NO_META_DIR))
    }

    pub fn thumb_root(&self) -> PathBuf {
        self.root.join(THUMBNAILS_DIR)
    }

    pub fn thumb_dir(&self, size: ThumbSize) -> PathBuf {
        self.thumb_root().join(size.dir_name())
    }

    pub fn vault_root(&self) -> PathBuf {
        self.root.join(VAULT_DIR)
    }

    /// `.vault/items/` — where encrypted originals live (§4.1, §5.5).
    pub fn vault_items(&self) -> PathBuf {
        self.vault_root().join(VAULT_ITEMS_DIR)
    }

    /// `.vault/thumbs/` — where vault thumbnails live (§4.1, §5.5).
    pub fn vault_thumbs(&self) -> PathBuf {
        self.vault_root().join(VAULT_THUMBS_DIR)
    }

    /// `App/` — sibling of `BackupManager.exe`, holds `bin/ffmpeg.exe` (§4.1, §5.5).
    pub fn app_dir(&self) -> PathBuf {
        self.root.join(APP_DIR)
    }

    /// `App/bin/` — where ffmpeg is deployed (§4.1).
    pub fn app_bin(&self) -> PathBuf {
        self.app_dir().join(APP_BIN_DIR)
    }

    pub fn ffmpeg_exe(&self) -> PathBuf {
        self.app_bin().join(FFMPEG_EXE_NAME)
    }

    pub fn rel_to_abs(&self, rel: &str) -> Result<PathBuf, PathError> {
        if rel.is_empty() {
            return Err(PathError::EmptyComponent);
        }

        let candidate = Path::new(rel);
        if candidate.has_root() || candidate.is_absolute() {
            return Err(PathError::NotRelative(rel.to_string()));
        }

        let mut out = self.root.clone();
        let mut appended = 0usize;

        for component in candidate.components() {
            match component {
                Component::Normal(name) => {
                    let name = name.to_string_lossy().to_string();
                    ensure_safe_component(&name)?;
                    out.push(name);
                    appended += 1;
                }
                Component::CurDir => {}
                Component::ParentDir => return Err(PathError::EscapesRoot(rel.to_string())),
                _ => return Err(PathError::NotRelative(rel.to_string())),
            }
        }

        if appended == 0 || !is_within(&out, &self.root) {
            return Err(PathError::EscapesRoot(rel.to_string()));
        }

        Ok(out)
    }

    pub fn abs_to_rel(&self, abs: &Path) -> Result<String, PathError> {
        if volume_key(&self.root) != volume_key(abs) {
            return Err(PathError::ForeignVolume(abs.to_string_lossy().to_string()));
        }

        let root_parts = normal_components(&self.root);
        let abs_parts = normal_components(abs);

        if abs_parts.len() < root_parts.len() {
            return Err(PathError::EscapesRoot(abs.to_string_lossy().to_string()));
        }

        for (index, root_part) in root_parts.iter().enumerate() {
            if !root_part.eq_ignore_ascii_case(&abs_parts[index]) {
                return Err(self.out_of_root_error(abs));
            }
        }

        let rel = abs_parts[root_parts.len()..].join("/");
        if rel.is_empty() {
            return Err(PathError::IsRoot);
        }

        Ok(rel)
    }

    fn out_of_root_error(&self, abs: &Path) -> PathError {
        if volume_key(&self.root) == volume_key(abs) {
            PathError::EscapesRoot(abs.to_string_lossy().to_string())
        } else {
            PathError::ForeignVolume(abs.to_string_lossy().to_string())
        }
    }
}

pub fn sanitize_label(raw: &str) -> String {
    let filtered: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == ' ' || *c == '_' || *c == '-')
        .collect();

    let collapsed = filtered.split_whitespace().collect::<Vec<_>>().join(" ");

    if collapsed.is_empty() {
        return FALLBACK_DEVICE_LABEL.to_string();
    }

    collapsed
        .chars()
        .take(MAX_LABEL_LEN)
        .collect::<String>()
        .trim_end()
        .to_string()
}

pub fn sanitize_device_label(raw: &str) -> Result<String, PathError> {
    let label = sanitize_label(raw);

    if label.eq_ignore_ascii_case(PC_DEVICE_LABEL) {
        return Err(PathError::ReservedLabel(label));
    }

    ensure_safe_component(&label)?;
    Ok(label)
}

pub fn is_reserved_windows_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    WINDOWS_RESERVED.contains(&stem.as_str())
}

pub fn ensure_safe_component(name: &str) -> Result<(), PathError> {
    if name.is_empty() {
        return Err(PathError::EmptyComponent);
    }

    if name.chars().any(|c| ILLEGAL_CHARS.contains(&c))
        || name.ends_with('.')
        || name.ends_with(' ')
    {
        return Err(PathError::IllegalCharacters(name.to_string()));
    }

    if is_reserved_windows_name(name) {
        return Err(PathError::ReservedName(name.to_string()));
    }

    Ok(())
}

pub fn has_marker(dir: &Path) -> bool {
    dir.join(DATABASE_DIR).join(MARKER_DB).is_file()
}

fn env_root(wolfs_root: Option<&OsStr>) -> Option<PathBuf> {
    let raw = wolfs_root?;
    if raw.is_empty() {
        return None;
    }

    let candidate = Path::new(raw);
    if candidate.is_dir() && has_marker(candidate) {
        return Some(candidate.to_path_buf());
    }

    None
}

fn walk_up_for_marker(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|ancestor| has_marker(ancestor))
        .map(Path::to_path_buf)
}

/// §5.2 step 2b. `start` is the folder holding the running executable; the root
/// is its parent when - and only when - `start` is the deployed `App/` folder,
/// identified by the exe name §4.4 pins. Case-insensitive, because Windows is.
fn packaged_root(start: &Path) -> Option<PathBuf> {
    let is_app_dir = start
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(APP_DIR));
    if !is_app_dir {
        return None;
    }
    if !start.join(EXE_NAME).is_file() {
        return None;
    }
    start.parent().map(Path::to_path_buf)
}

fn start_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn normal_components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_string_lossy().to_string()),
            _ => None,
        })
        .collect()
}

fn volume_key(path: &Path) -> String {
    let lowered = path.to_string_lossy().to_lowercase();
    match lowered.find(':') {
        Some(index) => lowered[..index].to_string(),
        None => lowered,
    }
}

fn is_within(path: &Path, root: &Path) -> bool {
    let root_text = root.to_string_lossy().to_lowercase();
    let path_text = path.to_string_lossy().to_lowercase();
    let prefix = if root_text.ends_with('\\') || root_text.ends_with('/') {
        root_text.clone()
    } else {
        format!("{root_text}\\")
    };

    path_text.starts_with(&prefix) || path_text == root_text.trim_end_matches(['\\', '/'])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn portable_root() -> TempDir {
        let dir = TempDir::new().expect("temp dir");
        fs::create_dir_all(dir.path().join(DATABASE_DIR)).expect("database dir");
        fs::write(dir.path().join(DATABASE_DIR).join(MARKER_DB), b"").expect("marker");
        dir
    }

    fn resolver_at(root: &Path) -> PathResolver {
        PathResolver::new(root.to_path_buf(), RootSource::MarkerWalk)
    }

    #[test]
    fn marker_walk_finds_the_portable_root() {
        let root = portable_root();
        let deep = root.path().join("App").join("bin");
        fs::create_dir_all(&deep).expect("exe dir");

        let resolver = PathResolver::resolve(&deep, None);

        assert_eq!(resolver.root(), root.path());
        assert_eq!(resolver.source(), RootSource::MarkerWalk);
        assert!(resolver.is_rooted());
    }

    #[test]
    fn marker_walk_prefers_the_nearest_marker() {
        let outer = portable_root();
        let inner = outer.path().join("inner");
        fs::create_dir_all(inner.join(DATABASE_DIR)).expect("inner database");
        fs::write(inner.join(DATABASE_DIR).join(MARKER_DB), b"").expect("inner marker");
        let deep = inner.join("src-tauri").join("target").join("debug");
        fs::create_dir_all(&deep).expect("exe dir");

        let resolver = PathResolver::resolve(&deep, None);

        assert_eq!(resolver.root(), inner);
    }

    #[test]
    fn resolution_survives_the_tree_moving_to_another_folder() {
        let root = portable_root();
        let before = PathResolver::resolve(&root.path().join("App"), None);
        assert_eq!(before.source(), RootSource::MarkerWalk);

        let moved_parent = TempDir::new().expect("new volume");
        let moved = moved_parent.path().join("wolfs");
        fs::rename(root.path(), &moved).expect("move tree");

        let after = PathResolver::resolve(&moved.join("App"), None);

        assert_eq!(after.source(), RootSource::MarkerWalk);
        assert_eq!(after.root(), moved.as_path());
        assert_eq!(after.media_root(), moved.join(MEDIA_DIR));
        assert_eq!(after.db_path(), moved.join(DATABASE_DIR).join(MARKER_DB));
    }

    #[test]
    fn wolfs_root_env_wins_over_the_marker_walk() {
        let exe_root = portable_root();
        let env_root = portable_root();

        let resolver = PathResolver::resolve(
            &exe_root.path().join("App"),
            Some(env_root.path().as_os_str()),
        );

        assert_eq!(resolver.root(), env_root.path());
        assert_eq!(resolver.source(), RootSource::WolfsRootEnv);
    }

    #[test]
    fn wolfs_root_env_is_ignored_without_a_marker() {
        let exe_root = portable_root();
        let bare = TempDir::new().expect("plain dir");

        let resolver =
            PathResolver::resolve(&exe_root.path().join("App"), Some(bare.path().as_os_str()));

        assert_eq!(resolver.root(), exe_root.path());
        assert_eq!(resolver.source(), RootSource::MarkerWalk);
    }

    #[test]
    fn wolfs_root_env_is_ignored_when_it_is_not_a_directory() {
        let exe_root = portable_root();
        let file = exe_root.path().join(DATABASE_DIR).join(MARKER_DB);

        let resolver = PathResolver::resolve(&exe_root.path().join("App"), Some(file.as_os_str()));

        assert_eq!(resolver.root(), exe_root.path());
        assert_eq!(resolver.source(), RootSource::MarkerWalk);
    }

    #[test]
    fn an_empty_env_var_does_not_override_the_walk() {
        let exe_root = portable_root();

        let resolver = PathResolver::resolve(&exe_root.path().join("App"), Some(OsStr::new("")));

        assert_eq!(resolver.source(), RootSource::MarkerWalk);
    }

    #[test]
    fn without_any_marker_the_exe_folder_is_used_and_flagged() {
        let bare = TempDir::new().expect("plain dir");
        let exe_dir = bare.path().join("src-tauri").join("target").join("debug");
        fs::create_dir_all(&exe_dir).expect("exe dir");

        let resolver = PathResolver::resolve(&exe_dir, None);

        assert_eq!(resolver.root(), exe_dir);
        assert_eq!(resolver.source(), RootSource::UnrootedFallback);
        assert!(!resolver.is_rooted());
    }

    /// §5.2 step 2b, and the reason it exists: a drive deployed by c5 has no
    /// marker until the first boot, so without this the root would be the exe
    /// folder and the whole tree would land inside `App/`.
    #[test]
    fn a_deployed_app_folder_yields_its_parent() {
        let bare = TempDir::new().expect("plain dir");
        let app_dir = bare.path().join(APP_DIR);
        fs::create_dir_all(&app_dir).expect("app dir");
        fs::write(app_dir.join(EXE_NAME), b"MZ").expect("exe");
        assert!(!has_marker(bare.path()), "precondition: no marker yet");

        let resolver = PathResolver::resolve(&app_dir, None);

        assert_eq!(resolver.root(), bare.path());
        assert_eq!(resolver.source(), RootSource::PackagedAppDir);
        assert!(
            resolver.is_rooted(),
            "a deployed root is not the unrooted case"
        );
    }

    /// The rule must not fire on a folder that merely happens to be called `App`:
    /// it is only the deployed layout when the pinned exe is actually in there.
    #[test]
    fn an_app_folder_without_the_exe_is_not_taken_as_a_root() {
        let bare = TempDir::new().expect("plain dir");
        let app_dir = bare.path().join(APP_DIR);
        fs::create_dir_all(&app_dir).expect("app dir");
        fs::write(app_dir.join("notes.txt"), b"oi").expect("unrelated file");

        let resolver = PathResolver::resolve(&app_dir, None);

        assert_eq!(resolver.root(), app_dir);
        assert_eq!(resolver.source(), RootSource::UnrootedFallback);
    }

    /// The marker is the proof; the layout guess is only the fallback. Once a
    /// marker exists anywhere up the walk it wins, so re-deploying never moves a
    /// root that the app is already using.
    #[test]
    fn a_real_marker_outranks_the_packaged_layout_guess() {
        let root = portable_root();
        let app_dir = root.path().join(APP_DIR);
        fs::create_dir_all(&app_dir).expect("app dir");
        fs::write(app_dir.join(EXE_NAME), b"MZ").expect("exe");

        let resolver = PathResolver::resolve(&app_dir, None);

        assert_eq!(resolver.root(), root.path());
        assert_eq!(resolver.source(), RootSource::MarkerWalk);
    }

    /// The env escape hatch still wins over both (human-confirmed §5.2 step 1).
    #[test]
    fn the_env_root_outranks_the_packaged_layout_guess() {
        let bare = TempDir::new().expect("plain dir");
        let app_dir = bare.path().join(APP_DIR);
        fs::create_dir_all(&app_dir).expect("app dir");
        fs::write(app_dir.join(EXE_NAME), b"MZ").expect("exe");

        let override_root = portable_root();
        let resolver = PathResolver::resolve(&app_dir, Some(override_root.path().as_os_str()));

        assert_eq!(resolver.root(), override_root.path());
        assert_eq!(resolver.source(), RootSource::WolfsRootEnv);
    }

    #[test]
    fn derived_directories_are_joined_from_the_root() {
        let resolver = resolver_at(Path::new("E:\\wolfs"));

        assert_eq!(resolver.media_root(), PathBuf::from("E:\\wolfs\\Media"));
        assert_eq!(
            resolver.thumb_root(),
            PathBuf::from("E:\\wolfs\\Thumbnails")
        );
        assert_eq!(resolver.vault_root(), PathBuf::from("E:\\wolfs\\.vault"));
        assert_eq!(
            resolver.db_path(),
            PathBuf::from("E:\\wolfs\\Database\\app.db")
        );
        assert_eq!(
            resolver.geonames_path(),
            PathBuf::from("E:\\wolfs\\Database\\geonames.bin")
        );
        assert_eq!(
            resolver.ffmpeg_exe(),
            PathBuf::from("E:\\wolfs\\App\\bin\\ffmpeg.exe")
        );
        assert_eq!(
            resolver.media_device("iPhone 14 Pro").unwrap(),
            PathBuf::from("E:\\wolfs\\Media\\iPhone 14 Pro")
        );
        assert_eq!(
            resolver.media_no_meta("iPhone 14 Pro").unwrap(),
            PathBuf::from("E:\\wolfs\\Media\\iPhone 14 Pro\\Sem_Metadados")
        );
        assert_eq!(
            resolver.media_device(PC_DEVICE_LABEL).unwrap(),
            PathBuf::from("E:\\wolfs\\Media\\PC")
        );
    }

    #[test]
    fn thumb_dir_maps_the_two_standard_sizes() {
        let resolver = resolver_at(Path::new("E:\\wolfs"));

        assert_eq!(
            resolver.thumb_dir(ThumbSize::Small),
            PathBuf::from("E:\\wolfs\\Thumbnails\\200")
        );
        assert_eq!(
            resolver.thumb_dir(ThumbSize::Large),
            PathBuf::from("E:\\wolfs\\Thumbnails\\400")
        );
        assert_eq!(ThumbSize::Small.px(), 200);
        assert_eq!(ThumbSize::Large.px(), 400);
    }

    #[test]
    fn sanitize_label_keeps_only_the_allowed_charset() {
        assert_eq!(sanitize_label("iPhone 14 Pro"), "iPhone 14 Pro");
        assert_eq!(sanitize_label("iPhone_14-Pro"), "iPhone_14-Pro");
        assert_eq!(sanitize_label("iPhone/14:Pro?"), "iPhone14Pro");
        assert_eq!(sanitize_label("Fone do Bandejão"), "Fone do Bandejo");
        assert_eq!(sanitize_label("café ☕"), "caf");
    }

    #[test]
    fn sanitize_label_collapses_and_trims_whitespace() {
        assert_eq!(sanitize_label("  iPhone   14 \t Pro  "), "iPhone 14 Pro");
    }

    #[test]
    fn sanitize_label_clamps_to_sixty_four_characters() {
        let long = "A".repeat(120);
        let label = sanitize_label(&long);

        assert_eq!(label.chars().count(), MAX_LABEL_LEN);
    }

    #[test]
    fn sanitize_label_falls_back_when_nothing_survives() {
        assert_eq!(sanitize_label(""), FALLBACK_DEVICE_LABEL);
        assert_eq!(sanitize_label("///"), FALLBACK_DEVICE_LABEL);
        assert_eq!(sanitize_label("   "), FALLBACK_DEVICE_LABEL);
    }

    #[test]
    fn sanitize_device_label_rejects_the_reserved_pc_label() {
        assert_eq!(
            sanitize_device_label("pc"),
            Err(PathError::ReservedLabel("pc".to_string()))
        );
        assert_eq!(
            sanitize_device_label("PC"),
            Err(PathError::ReservedLabel("PC".to_string()))
        );
        assert_eq!(sanitize_device_label("iPhone").unwrap(), "iPhone");
    }

    #[test]
    fn sanitize_device_label_rejects_windows_reserved_names() {
        assert_eq!(
            sanitize_device_label("CON"),
            Err(PathError::ReservedName("CON".to_string()))
        );
        assert_eq!(
            sanitize_device_label("nul"),
            Err(PathError::ReservedName("nul".to_string()))
        );
    }

    #[test]
    fn media_device_sanitizes_unsafe_names_and_rejects_reserved_ones() {
        let resolver = resolver_at(Path::new("E:\\wolfs"));

        assert_eq!(
            resolver.media_device("Tab/let").unwrap(),
            PathBuf::from("E:\\wolfs\\Media\\Tablet")
        );
        assert_eq!(
            resolver.media_device("iPhone: 15").unwrap(),
            PathBuf::from("E:\\wolfs\\Media\\iPhone 15")
        );
        assert!(matches!(
            resolver.media_device("COM1"),
            Err(PathError::ReservedName(_))
        ));
    }

    #[test]
    fn ensure_safe_component_rejects_windows_name_rules() {
        assert_eq!(ensure_safe_component(""), Err(PathError::EmptyComponent));
        assert!(matches!(
            ensure_safe_component("a:b"),
            Err(PathError::IllegalCharacters(_))
        ));
        assert!(matches!(
            ensure_safe_component("trailing."),
            Err(PathError::IllegalCharacters(_))
        ));
        assert!(matches!(
            ensure_safe_component("trailing "),
            Err(PathError::IllegalCharacters(_))
        ));
        assert!(matches!(
            ensure_safe_component("aux.txt"),
            Err(PathError::ReservedName(_))
        ));
        assert_eq!(ensure_safe_component("Sem_Metadados"), Ok(()));
        assert_eq!(ensure_safe_component("COM10"), Ok(()));
    }

    #[test]
    fn rel_to_abs_joins_forward_slash_paths_onto_the_root() {
        let resolver = resolver_at(Path::new("E:\\wolfs"));

        let abs = resolver
            .rel_to_abs("Media/iPhone 14 Pro/2024/05/IMG_4501.HEIC")
            .expect("relative path");

        assert_eq!(
            abs,
            PathBuf::from("E:\\wolfs\\Media\\iPhone 14 Pro\\2024\\05\\IMG_4501.HEIC")
        );
    }

    #[test]
    fn rel_to_abs_rejects_absolute_and_volume_prefixed_paths() {
        let resolver = resolver_at(Path::new("E:\\wolfs"));

        for bad in [
            "E:\\wolfs\\Media",
            "/etc/passwd",
            "C:Media",
            "\\\\server\\share\\x",
        ] {
            assert!(
                matches!(resolver.rel_to_abs(bad), Err(PathError::NotRelative(_))),
                "expected NotRelative for {bad}"
            );
        }
    }

    #[test]
    fn rel_to_abs_rejects_traversal() {
        let resolver = resolver_at(Path::new("E:\\wolfs"));

        for bad in ["../outside", "Media/../../outside", "./../outside"] {
            assert!(
                matches!(resolver.rel_to_abs(bad), Err(PathError::EscapesRoot(_))),
                "expected EscapesRoot for {bad}"
            );
        }
    }

    #[test]
    fn rel_to_abs_rejects_empty_and_unsafe_components() {
        let resolver = resolver_at(Path::new("E:\\wolfs"));

        assert_eq!(resolver.rel_to_abs(""), Err(PathError::EmptyComponent));
        assert!(matches!(
            resolver.rel_to_abs("Media/NUL.jpg"),
            Err(PathError::ReservedName(_))
        ));
        assert!(matches!(
            resolver.rel_to_abs("Media/a:b.jpg"),
            Err(PathError::IllegalCharacters(_))
        ));
    }

    #[test]
    fn abs_to_rel_round_trips_unicode_and_nested_paths() {
        let resolver = resolver_at(Path::new("E:\\wolfs"));
        let rel = "Media/iPhone/2024/05/IMG_4501 (1).HEIC";

        let abs = resolver.rel_to_abs(rel).expect("relative path");
        let back = resolver.abs_to_rel(&abs).expect("back to relative");

        assert_eq!(back, rel);
    }

    #[test]
    fn abs_to_rel_rejects_sibling_folders_with_a_shared_prefix() {
        let resolver = resolver_at(Path::new("E:\\wolfs"));

        assert!(matches!(
            resolver.abs_to_rel(Path::new("E:\\wolfs-old\\Media\\a.jpg")),
            Err(PathError::EscapesRoot(_))
        ));
    }

    #[test]
    fn abs_to_rel_rejects_another_volume() {
        let resolver = resolver_at(Path::new("E:\\wolfs"));

        assert!(matches!(
            resolver.abs_to_rel(Path::new("F:\\wolfs\\Media\\a.jpg")),
            Err(PathError::ForeignVolume(_))
        ));
    }

    #[test]
    fn abs_to_rel_rejects_the_root_itself() {
        let resolver = resolver_at(Path::new("E:\\wolfs"));

        assert_eq!(
            resolver.abs_to_rel(Path::new("E:\\wolfs")),
            Err(PathError::IsRoot)
        );
    }

    #[cfg(windows)]
    #[test]
    fn path_conversion_is_case_insensitive_like_windows() {
        let resolver = resolver_at(Path::new("E:\\Wolfs"));

        let abs = PathBuf::from("E:\\wolfs\\media\\iPhone\\a.jpg");
        assert_eq!(resolver.abs_to_rel(&abs).unwrap(), "media/iPhone/a.jpg");
    }
}
