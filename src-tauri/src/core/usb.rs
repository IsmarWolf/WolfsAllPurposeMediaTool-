//! PLAN §7.7 — the USB transport seam, and §12.1's copy/move policy.
//!
//! Two halves live here on purpose:
//!
//! * [`DeviceSource`] is the **leaf**: enumerate a device, list its media, pull
//!   one object onto the SSD, delete one object. The only implementation is
//!   `usb::wpd`, and it is the only module allowed to name `winmtp`.
//! * [`ingest_from`] is the **policy**: which objects count as new, where a
//!   pulled object lands, and — the part that matters — when a `Move` is allowed
//!   to delete anything from the phone.
//!
//! The split is what makes this testable. The policy is where the risk lives and
//! it is pure: it can be driven by a fake device on a laptop with no phone
//! attached, which is why the tests at the bottom of this file exist at all.
//!
//! ## The deletion order (§12.1)
//!
//! `Move` deletes **nothing** during the pull. The order is
//! write → hash → DB insert → *then* the PTP delete, and the hash and the insert
//! belong to the §7.3 pass, which runs *after* every object has landed. So the
//! pull only ever records what it placed ([`Pulled`]); [`delete_pulled`] is a
//! separate call the command makes once the pass is done. If the pass is
//! cancelled or fails, [`delete_pulled`] is never reached and the phone keeps
//! everything.
//!
//! ## Why `winmtp` and not raw COM
//!
//! The first cut of this module (commit `ea44337`) claimed a WPD reader that did
//! not exist. The real one could have been ~700 lines of `unsafe` `windows`-crate
//! COM that could not be compiled, reviewed or tested without a phone plugged in.
//! `winmtp` is a safe wrapper over exactly that API, which removes the unsafe and
//! leaves only the part that genuinely needs hardware smoke-testing.

use std::fs;
use std::path::Path;

use crate::core::models::DeviceDto;
use crate::core::placement::{self, Placement};
use crate::core::scanner::CancelFlag;
use crate::error::AppError;

/// One media file on the device, addressed by its MTP object id (`"o2C"`).
///
/// MTP ids are **session-scoped**: they are not stable across a reconnect, which
/// is why nothing persists them and why a `Move` resolves objects and deletes
/// them inside one pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteObject {
    pub id: String,
    pub name: String,
    pub size: u64,
}

/// What one listing pass found, **and what it could not reach**.
///
/// The second field is not diagnostic sugar. `list_media` used to return a bare
/// `Vec`, and the walk behind it treated an unreadable folder as an empty one — so
/// a phone that WPD refused to enumerate and a phone with no photos were the same
/// value. That value then reached §7.3's scanner, which found no
/// `Media/<label>/` (nothing had been pulled, so nothing had created it) and
/// reported *"a pasta do dispositivo não existe: C:\…"* — an error about a local
/// folder, on a device that was plugged in and full of photos. A silent partial
/// read is the same lie at a smaller scale: `Move` is safe on it (an unreadable
/// folder is never pulled, so it is never deleted), but the user is told a backup
/// finished when part of the device was never looked at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaListing {
    pub objects: Vec<RemoteObject>,
    /// One entry per folder the device would not let us enumerate, naming the
    /// folder and the reason. Never fatal on its own — one unreadable album must
    /// not abandon the other 4 000 photos (§12.1).
    pub unreadable: Vec<String>,
}

/// A source of media reachable over USB.
///
/// Every method takes the device's friendly name rather than a handle, because a
/// COM device is bound to the thread that opened it (§9.2's threading rule) and
/// re-resolving by name is what lets a long job re-open the device after a
/// reconnect.
pub trait DeviceSource: Send + Sync {
    fn enumerate(&self) -> Result<Vec<DeviceDto>, AppError>;

    /// Every media-extension object on the device, recursively — plus the folders
    /// it could not read. See [`MediaListing`] for why the two travel together.
    fn list_media(
        &self,
        friendly: &str,
        on_enum: Option<&mut dyn FnMut(&str)>,
        cancel: Option<&CancelFlag>,
    ) -> Result<MediaListing, AppError>;

    /// Copies one object into `staged`, which must not exist. Returns bytes written.
    fn read_to_abs(
        &self,
        friendly: &str,
        object: &RemoteObject,
        staged: &Path,
    ) -> Result<u64, AppError>;

    /// Deletes one object from the device. Called **only** after its bytes are
    /// in the library and indexed.
    fn delete(&self, friendly: &str, object: &RemoteObject) -> Result<(), AppError>;
}

/// One object that reached the library, and the record a later `Move` deletes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pulled {
    pub object: RemoteObject,
    /// Library-relative path, §5.5.
    pub relative: String,
    pub bytes: u64,
}

/// What a pull pass did. The §7.3 pass that follows produces the user-facing
/// [`crate::core::models::ScanSummary`]; this is the transport's own tally.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsbOutcome {
    pub pulled: Vec<Pulled>,
    /// Objects the device offered that we chose not to pull (`only_new`).
    pub skipped: u32,
    /// Per-object failures. The pass carries on (§12.1: one bad file is not a
    /// failed backup), so these are collected, not returned as an error.
    pub errors: Vec<String>,
    pub cancelled: bool,
}

/// Everything one pull pass needs besides the source and the destination.
///
/// A struct rather than seven positional arguments: `friendly`/`label`/`only_new`
/// are three `String`s in a row, which is exactly the shape where a caller
/// silently swaps two arguments and imports into the wrong folder.
pub struct PullPass<'a> {
    /// The device's friendly name — how [`DeviceSource`] re-resolves it per call.
    pub friendly: &'a str,
    /// The sanitized folder name under `Media/`. Usually `friendly`.
    pub label: &'a str,
    /// §8.1's "só arquivos novos".
    pub only_new: bool,
    /// The `only_new` predicate, injected rather than looked up here so this
    /// function stays free of the database — the caller closes over the pool —
    /// and testable on its own.
    ///
    /// A `&dyn Fn` rather than a generic: a generic bound would have to be
    /// higher-ranked (`for<'o> Fn(&'o RemoteObject)`), which a closure inferred
    /// at one concrete lifetime does not satisfy, so every call site would need
    /// a coercion. This is a callback that exists to be a test seam, not to be
    /// monomorphised.
    pub already_indexed: &'a dyn Fn(&RemoteObject) -> bool,
    /// `Database/staging/usb`, created by the caller.
    pub staging: &'a Path,
    /// Shared with the job's cancel button and the `Move` delete phase.
    pub cancel: &'a CancelFlag,
    /// Reports each object as it lands, with the device-reported total, so the
    /// pane can show real progress against a number it can trust.
    pub on_pulled: &'a mut dyn FnMut(&RemoteObject, u32),
}

/// The pull half of §12.1: read each new object into staging and let
/// [`Placement`] decide where it lands.
///
/// Nothing is deleted here. See the module docs for why.
pub fn ingest_from<S: DeviceSource + ?Sized>(
    source: &S,
    placement: Placement<'_>,
    pass: PullPass<'_>,
) -> Result<UsbOutcome, AppError> {
    let PullPass {
        friendly,
        label,
        only_new,
        already_indexed,
        staging,
        cancel,
        on_pulled,
    } = pass;
    let listing = source.list_media(friendly, None, Some(pass.cancel))?;
    let total = listing.objects.len() as u32;
    let mut outcome = UsbOutcome::default();

    // Carried into the same channel as a per-file failure, because from the
    // user's side it is the same kind of news: some of what is on the device did
    // not make it into the backup.
    outcome.errors.extend(listing.unreadable);

    for object in listing.objects {
        if cancel.is_cancelled() {
            outcome.cancelled = true;
            break;
        }

        // Extension first: the device's own content type is not trustworthy (see
        // `placement::is_media`), and pulling a 4 GB file the pipeline will
        // reject anyway wastes the most expensive step.
        if !placement::is_media(&object.name) {
            outcome.skipped += 1;
            continue;
        }

        if only_new && already_indexed(&object) {
            outcome.skipped += 1;
            continue;
        }

        // A per-object staging file: the parts arrive one at a time and a shared
        // name would let the second append to the first (§7.8's staging rule).
        let staged = staging.join(format!("{}.part", uuid::Uuid::new_v4()));

        match pull_one(source, placement, friendly, label, &object, &staged) {
            Ok(pulled) => {
                outcome.pulled.push(pulled);
                on_pulled(&object, total);
            }
            Err(error) => {
                // The phone still has the original (§14); drop the partial bytes.
                let _ = fs::remove_file(&staged);
                outcome.errors.push(format!("{}: {}", object.name, error));
            }
        }
    }

    Ok(outcome)
}

fn pull_one<S: DeviceSource + ?Sized>(
    source: &S,
    placement: Placement<'_>,
    friendly: &str,
    label: &str,
    object: &RemoteObject,
    staged: &Path,
) -> Result<Pulled, AppError> {
    let bytes = source.read_to_abs(friendly, object, staged)?;

    // §12.1's real safety gate for `Move`. The device reports `size` *before* the
    // transfer, so a mismatch means the cable wobbled or the stream ended early.
    // Placing a short file would let the §7.3 pass index it and then
    // `delete_pulled` remove the *only* intact copy from the phone — silent,
    // irrecoverable data loss. A zero size is skipped because some drivers report
    // no size at all (see `walk_media`'s `unwrap_or(0)`), and that must not be
    // read as "every file is truncated".
    if object.size > 0 && bytes != object.size {
        let _ = fs::remove_file(staged);
        return Err(AppError::Device(format!(
            "transferência incompleta: o aparelho informou {} bytes e chegaram {}",
            object.size, bytes
        )));
    }

    // `place` renames the staging file into the library, so a rejection here
    // (bad name, unwritable folder) leaves `staged` for the caller to remove.
    let placed = placement.place(label, &object.name, staged)?;
    Ok(Pulled {
        object: object.clone(),
        relative: placed.relative,
        bytes,
    })
}

/// The delete half of §12.1 — called **only** after the §7.3 pass has indexed the
/// pulled files.
///
/// A failure here is counted, not fatal: the bytes are already safe on the SSD,
/// so refusing to finish the deletion is a worse outcome than a file the user
/// has to remove by hand. Returns how many objects were removed.
pub fn delete_pulled<S: DeviceSource + ?Sized>(
    source: &S,
    friendly: &str,
    pulled: &[Pulled],
    errors: &mut Vec<String>,
) -> u32 {
    let mut deleted = 0;
    for entry in pulled {
        match source.delete(friendly, &entry.object) {
            Ok(()) => deleted += 1,
            // The device is the untrusted party here: it may be unplugged
            // mid-pass, or refuse. Neither may abort a pass whose files are
            // already safely written.
            Err(error) => errors.push(format!("{}: {error}", entry.object.name)),
        }
    }
    deleted
}

/// The build used when `usb-ingest` is *not* compiled in — since c12 that is a
/// deliberate `--no-default-features` build, not the normal one, because USB is a
/// default feature. Enumerating on such a build reports no devices, and starting
/// an ingest is a typed `E_UNAVAILABLE` rather than a silent zero.
#[derive(Debug, Clone, Default)]
pub struct NullDeviceSource;

impl DeviceSource for NullDeviceSource {
    fn enumerate(&self) -> Result<Vec<DeviceDto>, AppError> {
        Ok(Vec::new())
    }

    fn list_media(
        &self,
        _friendly: &str,
        _on_enum: Option<&mut dyn FnMut(&str)>,
        _cancel: Option<&CancelFlag>,
    ) -> Result<MediaListing, AppError> {
        Err(AppError::Unavailable(
            "ingestão USB exige o build com o transporte MTP".into(),
        ))
    }

    fn read_to_abs(
        &self,
        _friendly: &str,
        _object: &RemoteObject,
        _staged: &Path,
    ) -> Result<u64, AppError> {
        Err(AppError::Unavailable("sem transporte MTP".into()))
    }

    fn delete(&self, _friendly: &str, _object: &RemoteObject) -> Result<(), AppError> {
        Err(AppError::Unavailable("sem transporte MTP".into()))
    }
}

#[cfg(not(feature = "usb-ingest"))]
pub use NullDeviceSource as ActiveDeviceSource;
#[cfg(feature = "usb-ingest")]
pub use wpd::WpdDeviceSource;
#[cfg(feature = "usb-ingest")]
pub use wpd::WpdDeviceSource as ActiveDeviceSource;

#[cfg(feature = "usb-ingest")]
mod wpd;

#[cfg(test)]
mod tests;
