//! Vault core (PLAN §7.10, §12.5, Phase 5 / c17).
//!
//! The vault is a **separate surface** (§12.5.6, C6): media inside it is
//! unreachable from the gallery, from any other folder and from every list. This
//! module owns the *storage* side of that promise — the credential check
//! (§12.5.1/§12.5.5), the move of an original into `.vault/` and back
//! (§12.5.2/§12.5.3), and the Windows hidden/system attributes (§12.5.4). The
//! SQL lives in `core::db` (§6.5); `core::vault` never names a `rusqlite` type.
//!
//! ## c17 contract (human-confirmed 2026-10-15/16)
//!
//! * **Physical path model.** While hidden, `media.relative_path` *is* the file's
//!   real `.vault/items/...` location and `media.is_hidden = 1` is the C6 scope
//!   flag; the `Media/...` it came from is kept in the new `media.restore_path`
//!   column (v2 → v3, §6.4) so §12.5.3 can put it back. Keeping the logical path
//!   in `relative_path` would leave the freed `Media/...` UNIQUE-occupied and
//!   collide with a later import of a file at the same path (schema
//!   `relative_path TEXT NOT NULL UNIQUE`, §6.1).
//! * **Mirror layout.** A hidden item keeps its subtree: `Media/<rest>` →
//!   `.vault/items/<rest>` — the tree §12.5.6 asked for, replacing §12.5.2's flat
//!   `{hash[:8]}_{safe_name}` layout. Folder-level hide preserves the subtree too.
//! * **No encryption.** Files are *moved* and marked (`attrib +h +s`,
//!   `is_hidden`); the password only gates the session (§12.5.6). Encryption is a
//!   §21.1 seam, not this slice.
//!
//! The module is deliberately **session-free about events and IPC**: `setup`,
//! `unlock` and `recover` verify credentials and flip a [`VaultSession`] flag; the
//! `vault_*` commands (c18) own the session in `AppState` and emit `wolfs://vault`
//! / `wolfs://db-changed` (§9.2).

use std::path::{Path, PathBuf};

use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{
        SaltString,
        rand_core::{OsRng, RngCore},
    },
};
use thiserror::Error;

use crate::core::db::{self, DbError, HideUpdate, SqlitePool, UnhideUpdate, VaultItem};
use crate::core::paths::{
    MEDIA_DIR, PathError, PathResolver, VAULT_DIR, VAULT_ITEMS_DIR, VAULT_THUMBS_DIR,
};
use crate::core::scanner;

/// §12.5.1 — the recovery key is 24 characters long.
pub const RECOVERY_KEY_LEN: usize = 24;

/// §12.5.1 — the password floor the strength meter is measured against.
pub const MIN_PASSWORD_LEN: usize = 8;

/// §12.5.1 — the alphabet that avoids ambiguous glyphs: no `I`, `L`, `O`, `0` or
/// `1`, so a hand-copied recovery key cannot be mistyped into another character.
const RECOVERY_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

/// A vault operation that failed for a reason the UI must tell apart (§8.2: the
/// stable `E_*` codes are assigned in `error.rs`, the only module that maps them).
#[derive(Debug, Error)]
pub enum VaultError {
    #[error("{0}")]
    Db(#[from] DbError),
    #[error("{0}")]
    Path(#[from] PathError),
    #[error("o cofre já foi configurado")]
    AlreadyConfigured,
    #[error("o cofre ainda não foi configurado")]
    NotConfigured,
    #[error("senha incorreta")]
    WrongPassword,
    #[error("chave de recuperação incorreta")]
    WrongRecoveryKey,
    #[error("o cofre está trancado")]
    Locked,
    #[error("a senha precisa ter ao menos {MIN_PASSWORD_LEN} caracteres")]
    WeakPassword,
    /// Argon2 could not hash a secret. Unreachable for a normal password/key.
    #[error("falha ao calcular o hash: {0}")]
    Hash(String),
    /// The row and the disk disagree (a vanished original, a row without the
    /// restore target c17 added). Surfaced rather than hidden as a fake success.
    #[error("{0}")]
    Integrity(String),
    #[error("falha ao mover {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Attributes(String),
}

/// §7.10 — the in-memory session flag. It lives in `AppState` once c18 wires the
/// commands; c17 only defines the lock/unlock transitions.
#[derive(Debug, Clone, Default)]
pub struct VaultSession {
    unlocked: bool,
}

impl VaultSession {
    pub fn new() -> Self {
        Self { unlocked: false }
    }

    pub fn is_unlocked(&self) -> bool {
        self.unlocked
    }

    /// §12.5.3/§11.6 — leaving Vault Mode closes the session. The files stay
    /// hidden; only the ability to see them is dropped.
    pub fn lock(&mut self) {
        self.unlocked = false;
    }

    fn open(&mut self) {
        self.unlocked = true;
    }
}

/// §12.5.1 — first run. Hashes the password and a freshly generated recovery key
/// (both Argon2id), stores them, and **returns the recovery key once** for the
/// one-time display. Refuses a second setup and a password below the floor.
pub fn setup(pool: &SqlitePool, password: &str) -> Result<String, VaultError> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(VaultError::WeakPassword);
    }
    if db::vault_config_get(pool)?.is_some() {
        return Err(VaultError::AlreadyConfigured);
    }

    let recovery_key = generate_recovery_key();
    let password_hash = hash_secret(password)?;
    let recovery_key_hash = hash_secret(&recovery_key)?;
    db::vault_config_insert(pool, &password_hash, &recovery_key_hash)?;
    Ok(recovery_key)
}

/// §12.5.1 — verify the password and open the session. A wrong password is
/// `E_AUTH` (`WrongPassword`), never a generic failure.
pub fn unlock(
    pool: &SqlitePool,
    session: &mut VaultSession,
    password: &str,
) -> Result<(), VaultError> {
    let config = db::vault_config_get(pool)?.ok_or(VaultError::NotConfigured)?;
    if verify_secret(password, &config.password_hash) {
        session.open();
        Ok(())
    } else {
        Err(VaultError::WrongPassword)
    }
}

/// §12.5.5 — the recovery key is an alternative unlock. The key is normalized
/// (case-folded, separators dropped) before it is checked, so a key copied in
/// lowercase or with spaces still works; a wrong key is `E_AUTH`.
pub fn recover(
    pool: &SqlitePool,
    session: &mut VaultSession,
    recovery_key: &str,
) -> Result<(), VaultError> {
    let config = db::vault_config_get(pool)?.ok_or(VaultError::NotConfigured)?;
    let normalized = normalize_recovery_key(recovery_key);
    if verify_secret(&normalized, &config.recovery_key_hash) {
        session.open();
        Ok(())
    } else {
        Err(VaultError::WrongRecoveryKey)
    }
}

/// §7.10 names `lock`; the transition itself is [`VaultSession::lock`].
pub fn lock(session: &mut VaultSession) {
    session.lock();
}

/// §12.5.2 / §7.10 c19 decision 12 — **hiding needs no password.** Putting a file
/// away is a write into the vault, not a look at what is inside it, so it must
/// not be gated on the session (human, 2026-10-17: "colocar mídia no oculto não
/// deveria pedir senha; só ver que ela está oculta deveria pedir"). Only reading
/// the hidden set (the §11.6 grid, `vault_unhide`) stays behind the session.
///
/// It does, however, require the vault to **exist**: without a `vault_config` row
/// the bytes would land in a `.vault/` that no password could ever open. So the
/// hide commands assert configuration, where they used to assert unlock.
pub fn ensure_configured(pool: &SqlitePool) -> Result<(), VaultError> {
    if db::vault_config_get(pool)?.is_some() {
        Ok(())
    } else {
        Err(VaultError::NotConfigured)
    }
}

/// §12.5.2 — hide the named items. Returns how many were actually moved (ids
/// that are unknown, already hidden, or whose original has vanished are skipped,
/// so one dead row cannot abort a batch).
pub fn hide_batch(
    resolver: &PathResolver,
    pool: &SqlitePool,
    ids: &[String],
) -> Result<usize, VaultError> {
    let items = db::vault_items_by_id(pool, ids)?;
    hide_items(resolver, pool, &items)
}

/// §12.5.6 folder-level hide — every visible item under `folder` (a root-relative
/// `Media/...` path) leaves its `Media/` location and lives in the vault, its
/// subtree preserved. Empty source folders are pruned afterwards.
pub fn hide_folder(
    resolver: &PathResolver,
    pool: &SqlitePool,
    folder: &str,
) -> Result<usize, VaultError> {
    let folder = folder.trim_matches('/');
    let items = db::vault_visible_items_in_folder(pool, folder)?;
    let hidden = hide_items(resolver, pool, &items)?;
    if let Ok(abs) = resolver.rel_to_abs(folder) {
        prune_empty_dirs(resolver, &abs);
    }
    Ok(hidden)
}

/// §12.5.3 — bring the named items back to their `restore_path` (auto-suffixed
/// when a new file occupies it), delete the vault thumb, and restore visibility.
pub fn unhide_batch(
    resolver: &PathResolver,
    pool: &SqlitePool,
    ids: &[String],
) -> Result<usize, VaultError> {
    let items = db::vault_items_by_id(pool, ids)?;
    restore_items(resolver, pool, &items)
}

/// §12.5.4 — set or clear `attrib +h +s` on one path. The caller treats a failure
/// as a warning (visibility in Explorer is cosmetic and recovery stays possible),
/// but the error is returned so it can be reported honestly.
#[cfg(windows)]
pub fn set_hidden_attrs(path: &Path, on: bool) -> Result<(), VaultError> {
    use std::process::Command;

    let mut command = Command::new("attrib");
    if on {
        command.arg("+h").arg("+s");
    } else {
        command.arg("-h").arg("-s");
    }
    let output = command
        .arg(path)
        .output()
        .map_err(|source| VaultError::Io {
            path: path.display().to_string(),
            source,
        })?;
    if !output.status.success() {
        return Err(VaultError::Attributes(format!(
            "attrib saiu com {}",
            output.status
        )));
    }
    Ok(())
}

/// §12.5.4 is Windows-specific; elsewhere the attributes do not exist and the
/// operation is a no-op (the vault still hides files by moving them).
#[cfg(not(windows))]
pub fn set_hidden_attrs(_path: &Path, _on: bool) -> Result<(), VaultError> {
    Ok(())
}

/// A completed `from → to` move, recorded so a mid-batch failure can be undone in
/// reverse before the DB is touched (§12.5.2 "rollback-safe").
struct CompletedMove {
    from: PathBuf,
    to: PathBuf,
}

fn hide_items(
    resolver: &PathResolver,
    pool: &SqlitePool,
    items: &[VaultItem],
) -> Result<usize, VaultError> {
    let mut moves: Vec<CompletedMove> = Vec::new();
    let mut updates: Vec<HideUpdate> = Vec::new();

    let staged = (|| -> Result<(), VaultError> {
        for item in items {
            if item.is_hidden {
                continue;
            }
            let source = resolver.rel_to_abs(&item.relative_path)?;
            if !source.is_file() {
                // The row points at a file that is not there: skip it rather than
                // abort the batch. A rescan repairs the row.
                continue;
            }

            let mirror = mirror_relative(&item.relative_path);
            let target_rel = unique_relative(resolver, pool, &mirror)?;
            let target = resolver.rel_to_abs(&target_rel)?;
            perform(&mut moves, &source, &target)?;

            let thumb_rel = vault_thumb_relative(item);
            let thumb_source = resolver.rel_to_abs(&item.thumb_path)?;
            if thumb_source.is_file() {
                let thumb_target = resolver.rel_to_abs(&thumb_rel)?;
                perform(&mut moves, &thumb_source, &thumb_target)?;
            }
            // §12.5.2: the 200 slot is dropped and regenerated from the 400 on
            // unhide, so it is never carried into the vault.
            let small = item.thumb_path.replacen("/400/", "/200/", 1);
            if small != item.thumb_path
                && let Ok(small_abs) = resolver.rel_to_abs(&small)
            {
                let _ = std::fs::remove_file(small_abs);
            }

            updates.push(HideUpdate {
                id: item.id.clone(),
                relative_path: target_rel,
                restore_path: item.relative_path.clone(),
                thumb_path: thumb_rel,
            });
        }
        Ok(())
    })();

    if let Err(error) = staged {
        rollback(&moves);
        return Err(error);
    }

    let applied = match db::vault_apply_hide(pool, &updates) {
        Ok(applied) => applied,
        Err(error) => {
            rollback(&moves);
            return Err(error.into());
        }
    };

    // §12.5.4 — non-fatal, and only after the DB agrees the item is hidden.
    for update in &updates {
        if let Ok(abs) = resolver.rel_to_abs(&update.relative_path) {
            let _ = set_hidden_attrs(&abs, true);
        }
    }
    let _ = apply_vault_attributes(resolver);
    Ok(applied)
}

fn restore_items(
    resolver: &PathResolver,
    pool: &SqlitePool,
    items: &[VaultItem],
) -> Result<usize, VaultError> {
    let mut moves: Vec<CompletedMove> = Vec::new();
    let mut updates: Vec<UnhideUpdate> = Vec::new();

    let staged = (|| -> Result<(), VaultError> {
        for item in items {
            if !item.is_hidden {
                continue;
            }
            let restore = item.restore_path.clone().ok_or_else(|| {
                VaultError::Integrity(format!("item {} não tem restore_path", item.id))
            })?;
            let source = resolver.rel_to_abs(&item.relative_path)?;
            if !source.is_file() {
                return Err(VaultError::Integrity(format!(
                    "arquivo do cofre ausente: {}",
                    item.relative_path
                )));
            }

            let target_rel = unique_relative(resolver, pool, &restore)?;
            let target = resolver.rel_to_abs(&target_rel)?;
            perform(&mut moves, &source, &target)?;

            // §12.5.3 — the 400 WebP goes back to its §6.7 slot; the 200 is
            // regenerated by the thumbs queue, so it is not worth re-encoding here.
            let thumb_rel = scanner::thumb_path_for(&target_rel);
            let thumb_source = resolver.rel_to_abs(&item.thumb_path)?;
            if thumb_source.is_file() {
                let thumb_target = resolver.rel_to_abs(&thumb_rel)?;
                perform(&mut moves, &thumb_source, &thumb_target)?;
            }

            updates.push(UnhideUpdate {
                id: item.id.clone(),
                relative_path: target_rel,
                thumb_path: thumb_rel,
            });
        }
        Ok(())
    })();

    if let Err(error) = staged {
        rollback(&moves);
        return Err(error);
    }

    let applied = match db::vault_apply_unhide(pool, &updates) {
        Ok(applied) => applied,
        Err(error) => {
            rollback(&moves);
            return Err(error.into());
        }
    };

    for update in &updates {
        if let Ok(abs) = resolver.rel_to_abs(&update.relative_path) {
            let _ = set_hidden_attrs(&abs, false);
        }
    }
    Ok(applied)
}

/// §12.5.6 mirror: `Media/<rest>` → `.vault/items/<rest>` (the `Media/` root is
/// dropped, the device and every folder below it are preserved).
fn mirror_relative(media_relative: &str) -> String {
    let rest = media_relative
        .strip_prefix(&format!("{MEDIA_DIR}/"))
        .unwrap_or(media_relative);
    format!("{VAULT_DIR}/{VAULT_ITEMS_DIR}/{rest}")
}

/// §12.5.2 — the vault thumbnail for an item is named after its `file_hash`
/// (falling back to the id, which is a uuid and always path-safe).
fn vault_thumb_relative(item: &VaultItem) -> String {
    let stem = item.file_hash.as_deref().unwrap_or(&item.id);
    format!("{VAULT_DIR}/{VAULT_THUMBS_DIR}/{stem}.webp")
}

/// A free root-relative name: the requested one when neither the disk nor a row
/// holds it, otherwise ` (n)` appended to the stem (§12.5.3 "conflict → suffix").
fn unique_relative(
    resolver: &PathResolver,
    pool: &SqlitePool,
    relative: &str,
) -> Result<String, VaultError> {
    if !path_used(resolver, pool, relative)? {
        return Ok(relative.to_string());
    }
    for n in 1..10_000usize {
        let candidate = with_suffix(relative, n);
        if !path_used(resolver, pool, &candidate)? {
            return Ok(candidate);
        }
    }
    Err(VaultError::Integrity(format!(
        "não há nome livre para {relative}"
    )))
}

fn path_used(
    resolver: &PathResolver,
    pool: &SqlitePool,
    relative: &str,
) -> Result<bool, VaultError> {
    if resolver.rel_to_abs(relative)?.exists() {
        return Ok(true);
    }
    Ok(db::relative_path_taken(pool, relative)?)
}

/// Insert ` (n)` before the extension, preserving the directory (§12.5.3).
fn with_suffix(relative: &str, n: usize) -> String {
    let (dir, file) = match relative.rsplit_once('/') {
        Some((dir, file)) => (Some(dir), file),
        None => (None, relative),
    };
    let named = match file.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem} ({n}).{ext}"),
        None => format!("{file} ({n})"),
    };
    match dir {
        Some(dir) => format!("{dir}/{named}"),
        None => named,
    }
}

/// Rename `from → to`, creating the target's parent, and remember the move.
fn perform(moves: &mut Vec<CompletedMove>, from: &Path, to: &Path) -> Result<(), VaultError> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|source| VaultError::Io {
            path: to.display().to_string(),
            source,
        })?;
    }
    std::fs::rename(from, to).map_err(|source| VaultError::Io {
        path: to.display().to_string(),
        source,
    })?;
    moves.push(CompletedMove {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
    });
    Ok(())
}

/// Undo the file half of a failed batch, newest first. Best-effort: the DB was
/// never written, so a missed undo only leaves bytes to be re-scanned.
fn rollback(moves: &[CompletedMove]) {
    for step in moves.iter().rev() {
        let _ = std::fs::rename(&step.to, &step.from);
    }
}

/// §12.5.4 — the `.vault` folder itself, `items/` and `thumbs/`.
fn apply_vault_attributes(resolver: &PathResolver) -> Result<(), VaultError> {
    for dir in [
        resolver.vault_root(),
        resolver.vault_items(),
        resolver.vault_thumbs(),
    ] {
        set_hidden_attrs(&dir, true)?;
    }
    Ok(())
}

/// Best-effort removal of folders emptied by a folder-level hide: the deepest
/// directories first, then the ancestors up to (but never past) `Media/`.
fn prune_empty_dirs(resolver: &PathResolver, folder: &Path) {
    let media_root = resolver.media_root();

    let mut dirs: Vec<PathBuf> = walkdir::WalkDir::new(folder)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_dir())
        .map(walkdir::DirEntry::into_path)
        .collect();
    dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for dir in dirs {
        let _ = std::fs::remove_dir(&dir);
    }

    let mut current = folder.parent().map(Path::to_path_buf);
    while let Some(dir) = current {
        if dir == media_root || !dir.starts_with(&media_root) {
            break;
        }
        if std::fs::remove_dir(&dir).is_err() {
            break;
        }
        current = dir.parent().map(Path::to_path_buf);
    }
}

fn hash_secret(secret: &str) -> Result<String, VaultError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(secret.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| VaultError::Hash(error.to_string()))
}

fn verify_secret(secret: &str, stored: &str) -> bool {
    PasswordHash::new(stored)
        .map(|parsed| {
            Argon2::default()
                .verify_password(secret.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

/// §12.5.1 — a 24-character key from [`RECOVERY_ALPHABET`]. Rejection sampling
/// keeps the distribution uniform (256 is not a multiple of 31).
fn generate_recovery_key() -> String {
    let mut rng = OsRng;
    let mut key = String::with_capacity(RECOVERY_KEY_LEN);
    let limit = (256 / RECOVERY_ALPHABET.len()) * RECOVERY_ALPHABET.len();
    let mut byte = [0u8; 1];
    while key.len() < RECOVERY_KEY_LEN {
        rng.fill_bytes(&mut byte);
        let value = byte[0] as usize;
        if value < limit {
            key.push(RECOVERY_ALPHABET[value % RECOVERY_ALPHABET.len()] as char);
        }
    }
    key
}

/// §12.5.5 — the key as typed, reduced to the alphabet's uppercase letters and
/// digits, so spaces or dashes the user adds do not matter.
fn normalize_recovery_key(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::db::MediaInsert;
    use crate::core::layout;
    use crate::core::models::MediaScope;
    use crate::core::paths::RootSource;
    use tempfile::TempDir;

    fn root() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    fn resolver(root: &Path) -> PathResolver {
        PathResolver::new(root.to_path_buf(), RootSource::MarkerWalk)
    }

    fn open(resolver: &PathResolver) -> SqlitePool {
        layout::ensure(resolver).unwrap();
        db::open(resolver).unwrap()
    }

    fn write(resolver: &PathResolver, relative: &str, bytes: &[u8]) {
        let abs = resolver.rel_to_abs(relative).unwrap();
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(&abs, bytes).unwrap();
    }

    fn insert_photo(
        pool: &SqlitePool,
        resolver: &PathResolver,
        id: &str,
        relative: &str,
        hash: &str,
    ) {
        let thumb = scanner::thumb_path_for(relative);
        write(resolver, relative, b"photo-bytes");
        write(resolver, &thumb, b"thumb-400");
        write(resolver, &thumb.replace("/400/", "/200/"), b"thumb-200");
        db::insert_media_batch(
            pool,
            &[MediaInsert {
                id: id.into(),
                relative_path: relative.into(),
                thumb_path: thumb,
                file_hash: hash.into(),
                file_size: 11,
                file_type: "image".into(),
                captured_at: Some("2024-05-14 09:32:11".into()),
                has_metadata: true,
                device_name: "PC".into(),
                location: None,
            }],
        )
        .unwrap();
    }

    fn item(pool: &SqlitePool, id: &str) -> VaultItem {
        db::vault_items_by_id(pool, &[id.to_string()])
            .unwrap()
            .remove(0)
    }

    #[test]
    fn setup_creates_the_config_row_and_returns_a_recovery_key() {
        let dir = root();
        let resolver = resolver(dir.path());
        let pool = open(&resolver);

        let key = setup(&pool, "senha-forte").unwrap();

        assert_eq!(key.chars().count(), RECOVERY_KEY_LEN);
        assert!(
            key.bytes().all(|b| RECOVERY_ALPHABET.contains(&b)),
            "a chave usa apenas o alfabeto sem glifos ambíguos: {key}"
        );
        assert!(db::vault_config_get(&pool).unwrap().is_some());
        assert!(matches!(
            setup(&pool, "outra-senha"),
            Err(VaultError::AlreadyConfigured)
        ));
    }

    #[test]
    fn a_password_below_the_floor_is_refused() {
        let dir = root();
        let resolver = resolver(dir.path());
        let pool = open(&resolver);

        assert!(matches!(
            setup(&pool, "curta"),
            Err(VaultError::WeakPassword)
        ));
        assert!(db::vault_config_get(&pool).unwrap().is_none());
    }

    #[test]
    fn ensure_configured_gates_on_setup_not_on_the_session() {
        // §7.10 c19 decision 12: hiding is allowed while locked, so the guard is
        // "is there a vault?" — the session never enters the picture.
        let dir = root();
        let resolver = resolver(dir.path());
        let pool = open(&resolver);

        assert!(matches!(
            ensure_configured(&pool),
            Err(VaultError::NotConfigured)
        ));

        setup(&pool, "senha-forte").unwrap();
        ensure_configured(&pool).expect("configured vault: hiding is allowed");
    }

    #[test]
    fn unlock_checks_the_password_and_closes_on_lock() {
        let dir = root();
        let resolver = resolver(dir.path());
        let pool = open(&resolver);
        setup(&pool, "senha-forte").unwrap();

        let mut session = VaultSession::new();
        assert!(matches!(
            unlock(&pool, &mut session, "errada"),
            Err(VaultError::WrongPassword)
        ));
        assert!(!session.is_unlocked());

        unlock(&pool, &mut session, "senha-forte").unwrap();
        assert!(session.is_unlocked());

        lock(&mut session);
        assert!(!session.is_unlocked());
    }

    #[test]
    fn recover_accepts_only_the_generated_key() {
        let dir = root();
        let resolver = resolver(dir.path());
        let pool = open(&resolver);
        let key = setup(&pool, "senha-forte").unwrap();

        let mut session = VaultSession::new();
        assert!(matches!(
            recover(&pool, &mut session, "AAAAAAAAAAAAAAAAAAAAAAAA"),
            Err(VaultError::WrongRecoveryKey)
        ));
        assert!(!session.is_unlocked());

        recover(&pool, &mut session, &key).unwrap();
        assert!(session.is_unlocked());
    }

    #[test]
    fn a_lowercase_recovery_key_still_recovers() {
        let dir = root();
        let resolver = resolver(dir.path());
        let pool = open(&resolver);
        let key = setup(&pool, "senha-forte").unwrap();

        let mut session = VaultSession::new();
        recover(&pool, &mut session, &key.to_lowercase()).unwrap();
        assert!(session.is_unlocked());
    }

    #[test]
    fn hide_moves_the_file_under_the_vault_and_drops_the_small_thumb() {
        let dir = root();
        let resolver = resolver(dir.path());
        let pool = open(&resolver);
        insert_photo(&pool, &resolver, "m1", "Media/PC/2024/05/a.jpg", "hash-a");

        let hidden = hide_batch(&resolver, &pool, &["m1".to_string()]).unwrap();
        assert_eq!(hidden, 1);

        let row = item(&pool, "m1");
        assert!(row.is_hidden);
        assert_eq!(row.relative_path, ".vault/items/PC/2024/05/a.jpg");
        assert_eq!(row.restore_path.as_deref(), Some("Media/PC/2024/05/a.jpg"));
        assert_eq!(row.thumb_path, ".vault/thumbs/hash-a.webp");

        assert!(
            !resolver
                .rel_to_abs("Media/PC/2024/05/a.jpg")
                .unwrap()
                .exists(),
            "the original left Media/"
        );
        assert!(
            resolver
                .rel_to_abs(".vault/items/PC/2024/05/a.jpg")
                .unwrap()
                .is_file(),
            "and lives in the vault"
        );
        assert!(
            resolver
                .rel_to_abs(".vault/thumbs/hash-a.webp")
                .unwrap()
                .is_file()
        );
        assert!(
            !resolver
                .rel_to_abs("Thumbnails/200/PC/2024/05/a.webp")
                .unwrap()
                .exists(),
            "the 200 slot is dropped"
        );

        // C6 at the DB layer: the row is invisible to standard scope and visible
        // to the vault scope.
        assert_eq!(
            db::stats(&pool, MediaScope::Standard).unwrap().total_items,
            0
        );
        assert_eq!(db::stats(&pool, MediaScope::Vault).unwrap().total_items, 1);
    }

    #[test]
    fn unhide_restores_the_exact_bytes_and_clears_restore_path() {
        let dir = root();
        let resolver = resolver(dir.path());
        let pool = open(&resolver);
        insert_photo(&pool, &resolver, "m1", "Media/PC/2024/05/a.jpg", "hash-a");
        hide_batch(&resolver, &pool, &["m1".to_string()]).unwrap();

        let restored = unhide_batch(&resolver, &pool, &["m1".to_string()]).unwrap();
        assert_eq!(restored, 1);

        let row = item(&pool, "m1");
        assert!(!row.is_hidden);
        assert_eq!(row.relative_path, "Media/PC/2024/05/a.jpg");
        assert!(row.restore_path.is_none());
        assert_eq!(row.thumb_path, "Thumbnails/400/PC/2024/05/a.webp");

        assert_eq!(
            std::fs::read(resolver.rel_to_abs("Media/PC/2024/05/a.jpg").unwrap()).unwrap(),
            b"photo-bytes",
            "byte-identical after a round trip"
        );
        assert!(
            resolver
                .rel_to_abs("Thumbnails/400/PC/2024/05/a.webp")
                .unwrap()
                .is_file(),
            "the 400 came back to its §6.7 slot"
        );
        assert!(
            !resolver
                .rel_to_abs(".vault/thumbs/hash-a.webp")
                .unwrap()
                .exists()
        );
    }

    #[test]
    fn a_restore_into_an_occupied_path_is_suffixed() {
        let dir = root();
        let resolver = resolver(dir.path());
        let pool = open(&resolver);
        insert_photo(&pool, &resolver, "m1", "Media/PC/2024/05/a.jpg", "hash-a");
        hide_batch(&resolver, &pool, &["m1".to_string()]).unwrap();

        // A new file lands at the freed location before m1 comes back.
        insert_photo(&pool, &resolver, "m2", "Media/PC/2024/05/a.jpg", "hash-b");

        unhide_batch(&resolver, &pool, &["m1".to_string()]).unwrap();

        assert_eq!(item(&pool, "m2").relative_path, "Media/PC/2024/05/a.jpg");
        assert_eq!(
            item(&pool, "m1").relative_path,
            "Media/PC/2024/05/a (1).jpg"
        );
        assert!(
            resolver
                .rel_to_abs("Media/PC/2024/05/a (1).jpg")
                .unwrap()
                .is_file()
        );
    }

    #[test]
    fn hiding_a_folder_moves_the_whole_subtree_and_prunes_the_source() {
        let dir = root();
        let resolver = resolver(dir.path());
        let pool = open(&resolver);
        insert_photo(&pool, &resolver, "m1", "Media/PC/Trip/2024/a.jpg", "hash-a");
        insert_photo(&pool, &resolver, "m2", "Media/PC/Trip/2024/b.jpg", "hash-b");
        insert_photo(&pool, &resolver, "m3", "Media/PC/Other/c.jpg", "hash-c");

        let hidden = hide_folder(&resolver, &pool, "Media/PC/Trip").unwrap();
        assert_eq!(hidden, 2);

        assert!(item(&pool, "m1").is_hidden);
        assert!(item(&pool, "m2").is_hidden);
        assert!(!item(&pool, "m3").is_hidden, "a sibling is untouched");
        assert_eq!(
            item(&pool, "m1").relative_path,
            ".vault/items/PC/Trip/2024/a.jpg",
            "the subtree is preserved in the vault"
        );
        assert!(
            !resolver.rel_to_abs("Media/PC/Trip").unwrap().exists(),
            "the emptied source folder is pruned"
        );
        assert!(
            resolver
                .rel_to_abs("Media/PC/Other/c.jpg")
                .unwrap()
                .is_file()
        );
    }

    #[test]
    fn an_idempotent_hide_does_not_move_twice() {
        let dir = root();
        let resolver = resolver(dir.path());
        let pool = open(&resolver);
        insert_photo(&pool, &resolver, "m1", "Media/PC/a.jpg", "hash-a");

        assert_eq!(
            hide_batch(&resolver, &pool, &["m1".to_string()]).unwrap(),
            1
        );
        assert_eq!(
            hide_batch(&resolver, &pool, &["m1".to_string()]).unwrap(),
            0,
            "a second hide of the same id is a no-op, not a re-move"
        );
    }

    #[test]
    fn a_row_with_a_missing_original_is_skipped_not_fatal() {
        let dir = root();
        let resolver = resolver(dir.path());
        let pool = open(&resolver);
        insert_photo(&pool, &resolver, "m1", "Media/PC/2024/a.jpg", "hash-a");
        insert_photo(&pool, &resolver, "m2", "Media/PC/2024/b.jpg", "hash-b");
        std::fs::remove_file(resolver.rel_to_abs("Media/PC/2024/a.jpg").unwrap()).unwrap();

        let hidden = hide_batch(&resolver, &pool, &["m1".to_string(), "m2".to_string()]).unwrap();

        assert_eq!(hidden, 1);
        assert!(!item(&pool, "m1").is_hidden, "the dead row stays visible");
        assert!(item(&pool, "m2").is_hidden, "the healthy row is hidden");
    }

    #[cfg(windows)]
    #[test]
    fn hidden_attributes_are_set_and_cleared() {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;

        let dir = root();
        let path = dir.path().join("segredo.txt");
        std::fs::write(&path, b"x").unwrap();

        set_hidden_attrs(&path, true).unwrap();
        assert_ne!(
            std::fs::metadata(&path).unwrap().file_attributes() & FILE_ATTRIBUTE_HIDDEN,
            0
        );

        set_hidden_attrs(&path, false).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().file_attributes() & FILE_ATTRIBUTE_HIDDEN,
            0
        );
    }
}
