//! c12 policy tests. Everything here runs on a laptop with **no phone attached**:
//! the copy/move rules — which is where the data loss would be — are pure, so
//! they are pinned against a fake device instead of a hardware smoke test.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::*;

use crate::core::layout;
use crate::core::paths::{PathResolver, RootSource};
use crate::core::scanner::CancelFlag;

/// A phone that lives in a `BTreeMap`: the fake is deterministic, which is the
/// only reason `only_new` and cancellation can be asserted exactly.
struct FakeDevice {
    files: BTreeMap<String, Vec<u8>>,
    /// Names whose read must fail, to prove one bad file is not a failed backup.
    unreadable: Vec<String>,
    /// Folders the device would not let us enumerate (`MediaListing::unreadable`).
    unreadable_folders: Vec<String>,
    deleted: Mutex<Vec<String>>,
    /// Every staged path the fake was asked to write, to assert nothing leaks.
    staged_seen: Mutex<Vec<PathBuf>>,
}

impl FakeDevice {
    fn new(files: &[(&str, &[u8])]) -> Self {
        Self {
            files: files
                .iter()
                .map(|(name, bytes)| (name.to_string(), bytes.to_vec()))
                .collect(),
            unreadable: Vec::new(),
            unreadable_folders: Vec::new(),
            deleted: Mutex::new(Vec::new()),
            staged_seen: Mutex::new(Vec::new()),
        }
    }

    fn refusing(mut self, names: &[&str]) -> Self {
        self.unreadable = names.iter().map(|name| name.to_string()).collect();
        self
    }

    /// A device whose listing is empty *because* the walk failed — the state a
    /// locked iPhone used to be indistinguishable from.
    fn blind(mut self, folders: &[&str]) -> Self {
        self.unreadable_folders = folders.iter().map(|f| f.to_string()).collect();
        self
    }

    fn deleted(&self) -> Vec<String> {
        self.deleted.lock().expect("the delete log").clone()
    }
}

impl DeviceSource for FakeDevice {
    fn enumerate(&self) -> Result<Vec<DeviceDto>, AppError> {
        Ok(vec![DeviceDto {
            id: "fake-1".into(),
            name: "iPhone".into(),
            friendly_name: "iPhone".into(),
        }])
    }

    fn list_media(
        &self,
        _friendly: &str,
        _on_enum: Option<&mut dyn FnMut(&str)>,
        _cancel: Option<&crate::core::scanner::CancelFlag>,
    ) -> Result<MediaListing, AppError> {
        Ok(MediaListing {
            objects: self
                .files
                .iter()
                .map(|(name, bytes)| RemoteObject {
                    id: format!("id-{name}"),
                    name: name.clone(),
                    size: bytes.len() as u64,
                })
                .collect(),
            unreadable: self.unreadable_folders.clone(),
        })
    }

    fn read_to_abs(
        &self,
        _friendly: &str,
        object: &RemoteObject,
        staged: &Path,
    ) -> Result<u64, AppError> {
        self.staged_seen
            .lock()
            .expect("the staged log")
            .push(staged.to_path_buf());
        if self.unreadable.contains(&object.name) {
            return Err(AppError::Device("MTP read failed".into()));
        }
        let bytes = self
            .files
            .get(&object.name)
            .ok_or_else(|| AppError::Device("gone".into()))?;
        fs::write(staged, bytes).map_err(|error| AppError::Ingest(error.to_string()))?;
        Ok(bytes.len() as u64)
    }

    fn delete(&self, _friendly: &str, object: &RemoteObject) -> Result<(), AppError> {
        self.deleted
            .lock()
            .expect("the delete log")
            .push(object.name.clone());
        Ok(())
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    resolver: PathResolver,
    staging: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("a temp volume");
    let resolver = PathResolver::new(dir.path().to_path_buf(), RootSource::MarkerWalk);
    layout::ensure(&resolver).expect("the §5.9 tree");
    let staging = resolver.db_dir().join("staging").join("usb");
    fs::create_dir_all(&staging).expect("a staging dir");
    Fixture {
        _dir: dir,
        resolver,
        staging,
    }
}

const PHOTO: &[u8] = b"\xff\xd8\xff\xe0 not a real jpeg";

/// The default pass: everything is new, nothing is cancelled.
fn pull(device: &FakeDevice, fx: &Fixture, only_new: bool) -> UsbOutcome {
    let cancel = CancelFlag::new();
    let placement = Placement::new(&fx.resolver, None);
    ingest_from(
        device,
        placement,
        PullPass {
            friendly: "iPhone",
            label: "iPhone",
            only_new,
            already_indexed: &|_: &RemoteObject| false,
            staging: &fx.staging,
            cancel: &cancel,
            on_pulled: &mut |_, _| {},
        },
    )
    .expect("the pull pass completes")
}

#[test]
fn copy_pulls_every_media_file_and_deletes_nothing() {
    let fx = fixture();
    let device = FakeDevice::new(&[("IMG_0001.jpg", PHOTO), ("IMG_0002.jpg", PHOTO)]);

    let outcome = pull(&device, &fx, false);

    assert_eq!(outcome.pulled.len(), 2, "{:?}", outcome.errors);
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    // A copy must be invisible to the phone.
    assert!(
        device.deleted().is_empty(),
        "Copy deleted {:?}",
        device.deleted()
    );
    for entry in &outcome.pulled {
        assert!(entry.relative.starts_with("Media/iPhone/"), "{entry:?}");
        assert!(fx.resolver.rel_to_abs(&entry.relative).unwrap().is_file());
    }
}

#[test]
fn only_new_skips_what_the_caller_says_is_indexed() {
    let fx = fixture();
    let device = FakeDevice::new(&[("OLD.jpg", PHOTO), ("NEW.jpg", PHOTO)]);

    let cancel = CancelFlag::new();
    let placement = Placement::new(&fx.resolver, None);
    let outcome = ingest_from(
        &device,
        placement,
        PullPass {
            friendly: "iPhone",
            label: "iPhone",
            only_new: true,
            already_indexed: &|object: &RemoteObject| object.name == "OLD.jpg",
            staging: &fx.staging,
            cancel: &cancel,
            on_pulled: &mut |_, _| {},
        },
    )
    .expect("the pull pass completes");

    assert_eq!(outcome.skipped, 1, "the indexed object is skipped");
    assert_eq!(outcome.pulled.len(), 1);
    assert_eq!(outcome.pulled[0].object.name, "NEW.jpg");
    // Skipped means *not read*: the predicate runs before the expensive step.
    assert_eq!(device.staged_seen.lock().expect("log").len(), 1);
}

/// With `only_new` off the predicate must not be consulted at all — a stale
/// index must never silently turn a full backup into a partial one.
#[test]
fn the_predicate_is_ignored_when_only_new_is_off() {
    let fx = fixture();
    let device = FakeDevice::new(&[("OLD.jpg", PHOTO), ("NEW.jpg", PHOTO)]);

    let outcome = pull(&device, &fx, false);

    assert_eq!(outcome.skipped, 0);
    assert_eq!(outcome.pulled.len(), 2);
}

#[test]
fn a_non_media_extension_is_skipped_without_being_pulled() {
    let fx = fixture();
    let device = FakeDevice::new(&[("notes.txt", b"hello"), ("IMG_0001.jpg", PHOTO)]);

    let outcome = pull(&device, &fx, false);

    assert_eq!(outcome.pulled.len(), 1, "{:?}", outcome.errors);
    assert_eq!(outcome.skipped, 1);
    // It was never even asked for: the extension test runs before the read.
    assert_eq!(device.staged_seen.lock().expect("log").len(), 1);
}

#[test]
fn one_unreadable_file_does_not_fail_the_pass() {
    let fx = fixture();
    let device = FakeDevice::new(&[("BAD.jpg", PHOTO), ("GOOD.jpg", PHOTO)]).refusing(&["BAD.jpg"]);

    let outcome = pull(&device, &fx, false);

    assert_eq!(outcome.pulled.len(), 1, "the good file still lands");
    assert_eq!(outcome.errors.len(), 1, "{:?}", outcome.errors);
    assert!(outcome.errors[0].contains("BAD.jpg"));
    assert!(outcome.errors[0].contains("MTP read failed"));
}

/// §12.1's order, asserted as an invariant rather than a comment: the pull
/// touches the device read-only, and the delete is a separate call the command
/// makes only after the §7.3 pass has indexed the files.
#[test]
fn a_short_transfer_is_never_placed_and_so_never_deleted() {
    let fx = fixture();
    // The device *claims* 500 bytes and delivers 10 — the cable-wobble case.
    let device = LyingDevice {
        inner: FakeDevice::new(&[("TRUNC.jpg", PHOTO)]),
        claimed: 500,
    };

    let (outcome, cancel) = {
        let cancel = CancelFlag::new();
        let placement = Placement::new(&fx.resolver, None);
        let outcome = ingest_from(
            &device,
            placement,
            PullPass {
                friendly: "iPhone",
                label: "iPhone",
                only_new: false,
                already_indexed: &|_: &RemoteObject| false,
                staging: &fx.staging,
                cancel: &cancel,
                on_pulled: &mut |_, _| {},
            },
        )
        .expect("the pass completes");
        (outcome, cancel)
    };

    assert!(outcome.pulled.is_empty(), "a short file must not be placed");
    assert_eq!(outcome.errors.len(), 1);
    assert!(outcome.errors[0].contains("TRUNC.jpg"));
    assert!(
        outcome.errors[0].contains("500"),
        "the claim is in the message: {}",
        outcome.errors[0]
    );
    assert!(
        outcome.errors[0].contains(&PHOTO.len().to_string()),
        "the delivered count is in the message: {}",
        outcome.errors[0]
    );
    // The staging part was cleaned up, and nothing reached the library. The
    // device folder need not exist at all — refusing to place is allowed to
    // create nothing — so "absent or empty", not "exists and is empty".
    let device_dir = fx.resolver.media_device("iPhone").unwrap();
    let landed = match std::fs::read_dir(&device_dir) {
        Ok(entries) => entries.count(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => panic!("reading {}: {error}", device_dir.display()),
    };
    assert_eq!(landed, 0, "no file may land in the library");
    assert!(device.inner.deleted().is_empty());
    drop(cancel);
}

/// Wraps a real fake and lies about the object size, which is how a truncated
/// transfer reaches the policy layer.
struct LyingDevice {
    inner: FakeDevice,
    claimed: u64,
}

impl DeviceSource for LyingDevice {
    fn enumerate(&self) -> Result<Vec<DeviceDto>, AppError> {
        self.inner.enumerate()
    }
    fn list_media(
        &self,
        friendly: &str,
        _on_enum: Option<&mut dyn FnMut(&str)>,
        _cancel: Option<&crate::core::scanner::CancelFlag>,
    ) -> Result<MediaListing, AppError> {
        let mut listing = self.inner.list_media(friendly, None, None)?;
        for object in &mut listing.objects {
            object.size = self.claimed;
        }
        Ok(listing)
    }
    fn read_to_abs(
        &self,
        friendly: &str,
        object: &RemoteObject,
        staged: &Path,
    ) -> Result<u64, AppError> {
        self.inner.read_to_abs(friendly, object, staged)
    }
    fn delete(&self, friendly: &str, object: &RemoteObject) -> Result<(), AppError> {
        self.inner.delete(friendly, object)
    }
}

#[test]
fn an_unreadable_folder_reaches_the_user_as_a_problem_not_a_silent_gap() {
    let fx = fixture();
    let device = FakeDevice::new(&[("IMG_0001.jpg", PHOTO)])
        .blind(&["Internal Storage/DCIM/100APPLE: MTP indisponível (0x8007001F)"]);

    let outcome = pull(&device, &fx, false);

    assert_eq!(outcome.pulled.len(), 1, "the readable file still lands");
    assert_eq!(outcome.errors.len(), 1, "{outcome:?}");
    assert!(
        outcome.errors[0].contains("100APPLE"),
        "the unreadable folder is named: {outcome:?}"
    );
}

#[test]
fn the_pull_never_deletes_and_delete_pulled_is_separate() {
    let fx = fixture();
    let device = FakeDevice::new(&[("IMG_0001.jpg", PHOTO), ("IMG_0002.jpg", PHOTO)]);

    let outcome = pull(&device, &fx, false);

    assert_eq!(outcome.pulled.len(), 2);
    assert!(
        device.deleted().is_empty(),
        "a Move must not delete during the pull: {:?}",
        device.deleted()
    );

    let mut errors = Vec::new();
    let deleted = delete_pulled(&device, "iPhone", &outcome.pulled, &mut errors);

    assert_eq!(deleted, 2);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(device.deleted(), vec!["IMG_0001.jpg", "IMG_0002.jpg"]);
}

#[test]
fn a_cancelled_pass_pulls_nothing_further() {
    let fx = fixture();
    let device = FakeDevice::new(&[("A.jpg", PHOTO), ("B.jpg", PHOTO)]);

    let cancel = CancelFlag::new();
    cancel.cancel();
    let placement = Placement::new(&fx.resolver, None);
    let outcome = ingest_from(
        &device,
        placement,
        PullPass {
            friendly: "iPhone",
            label: "iPhone",
            only_new: false,
            already_indexed: &|_: &RemoteObject| false,
            staging: &fx.staging,
            cancel: &cancel,
            on_pulled: &mut |_, _| {},
        },
    )
    .expect("a cancelled pass is not an error");

    assert!(outcome.cancelled);
    assert!(outcome.pulled.is_empty());
    assert!(device.staged_seen.lock().expect("log").is_empty());
}

/// A delete the device refuses is counted, never fatal: the bytes are already on
/// the SSD, so aborting would leave the user worse off than a file to remove by
/// hand.
#[test]
fn a_refused_delete_is_counted_and_the_rest_still_go() {
    struct RefusesOne;
    impl DeviceSource for RefusesOne {
        fn enumerate(&self) -> Result<Vec<DeviceDto>, AppError> {
            Ok(Vec::new())
        }
        fn list_media(
            &self,
            _friendly: &str,
            _on_enum: Option<&mut dyn FnMut(&str)>,
            _cancel: Option<&crate::core::scanner::CancelFlag>,
        ) -> Result<MediaListing, AppError> {
            Ok(MediaListing::default())
        }
        fn read_to_abs(
            &self,
            _friendly: &str,
            _object: &RemoteObject,
            _staged: &Path,
        ) -> Result<u64, AppError> {
            Ok(0)
        }
        fn delete(&self, _friendly: &str, object: &RemoteObject) -> Result<(), AppError> {
            if object.name == "STUCK.jpg" {
                Err(AppError::Device("device busy".into()))
            } else {
                Ok(())
            }
        }
    }

    let pulled = vec![
        Pulled {
            object: RemoteObject {
                id: "1".into(),
                name: "STUCK.jpg".into(),
                size: 1,
            },
            relative: "Media/iPhone/a.jpg".into(),
            bytes: 1,
        },
        Pulled {
            object: RemoteObject {
                id: "2".into(),
                name: "OK.jpg".into(),
                size: 1,
            },
            relative: "Media/iPhone/b.jpg".into(),
            bytes: 1,
        },
    ];

    let mut errors = Vec::new();
    let deleted = delete_pulled(&RefusesOne, "iPhone", &pulled, &mut errors);

    assert_eq!(deleted, 1, "the refusal does not stop the second delete");
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("STUCK.jpg"));
}

/// §5.4 is enforced on the name the *device* reports, which is as untrusted as
/// a browser's `filename`.
#[test]
fn a_device_supplied_traversal_name_cannot_escape_the_library() {
    let fx = fixture();
    let device = FakeDevice::new(&[("../../escape.jpg", PHOTO)]);

    let outcome = pull(&device, &fx, false);

    assert_eq!(outcome.pulled.len(), 1, "{:?}", outcome.errors);
    let relative = &outcome.pulled[0].relative;
    assert!(relative.starts_with("Media/iPhone/"), "{relative}");
    assert!(!relative.contains(".."), "{relative}");
    let landed = fx.resolver.rel_to_abs(relative).unwrap();
    assert!(landed.is_file());
    assert!(landed.starts_with(fx.resolver.media_device("iPhone").unwrap()));
}

#[test]
fn the_null_source_reports_no_devices_and_refuses_to_start() {
    let source = NullDeviceSource;
    assert!(source.enumerate().unwrap().is_empty());
    let error = source
        .list_media("iPhone", None, None)
        .expect_err("no transport, no pass");
    assert!(matches!(error, AppError::Unavailable(_)), "{error:?}");
}

/**
 * The transport the app actually talks to follows `usb-ingest`, and the feature
 * is a **default** one (c12 decision, human 2026-10-04). This used to assert only
 * the `not(feature)` arm, which is exactly why it would have gone on passing
 * forever while the shipping build had no USB: the arm that mattered was empty in
 * both the default and the `--all-features` test runs. Now the default build
 * asserts the WPD source, so dropping `usb-ingest` from `default` fails here.
 *
 * `type_name` is the only way to compare an alias with the type it may or may
 * not resolve to — the two arms are different types and would not compile as one
 * match arm otherwise.
 */
#[test]
fn the_active_source_follows_the_feature() {
    let name = std::any::type_name::<crate::core::usb::ActiveDeviceSource>();

    #[cfg(feature = "usb-ingest")]
    assert!(name.ends_with("WpdDeviceSource"), "{name}");

    #[cfg(not(feature = "usb-ingest"))]
    assert!(name.ends_with("NullDeviceSource"), "{name}");
}
