//! PLAN §7.7 — the WPD/MTP transport, the only module allowed to name `winmtp`.
//!
//! Everything the app needs from a phone is here: enumerate, walk the media tree,
//! read one object onto the SSD, delete one object. The policy that decides *when*
//! to call any of it lives in the parent module and knows nothing about COM.
//!
//! ## Three facts about this API that shaped the code
//!
//! 1. **COM belongs to a thread.** `Provider::new()` calls `CoInitializeEx` with
//!    `COINIT_MULTITHREADED`, and initialization belongs to the thread that did
//!    it. So each method here opens its own provider inside a thread this module
//!    spawns for it ([`com_thread`]) — never on the caller's, because the caller is
//!    usually the UI thread, and WebView2 has already put *that* one in an STA
//!    where `CoInitializeEx(MTA)` fails with `RPC_E_CHANGED_MODE`. That is also why
//!    the trait takes a friendly name instead of a device handle: re-resolving by
//!    name is how a long job survives a reconnect.
//!
//! 2. **`winmtp` panics where the plan expected an error.** `BasicDevice::open`,
//!    `Object::delete` and `Object::move_to` each `.unwrap()` the underlying COM
//!    call, so a phone that is busy, half-unplugged, or already claimed by
//!    another program takes the thread down instead of returning `Err`. Every COM
//!    entry point below is wrapped in [`com`], which turns a driver bug into a
//!    normal [`AppError`] so the pass reports the failure and carries on (§12.1).
//!
//! 3. **Enumeration can silently truncate.** `winmtp::object::ObjectIterator` is
//!    `Next().ok().ok()?` followed by `object_by_id(..).ok()`: a COM error *or* a
//!    single child it cannot resolve ends the iterator, and the `for` loop simply
//!    stops. A phone with 4 000 photos whose cable wobbles can therefore report a
//!    tidy "1 200 arquivos" that is a lie. Since the user is about to trust that
//!    number — and to delete from the phone because of it — [`list_media`] walks
//!    **twice** and refuses to continue unless both walks agree.
//!
//! ## The two ways a walk lies, and what is done about each
//!
//! Truncation (fact 3) is only half of it. A walk can also fail *completely* and
//! look empty: WPD refuses to enumerate a locked iPhone, and `Object::children`
//! then returns an error. The first version of this file treated that error as "no
//! children", so the two cases became one value and the pane said a phone full of
//! photos had nothing on it. So [`walk_media`] records what it could not read
//! ([`MediaListing::unreadable`]) instead of swallowing it, and [`list_media`]
//! treats "nothing found" and "nothing could be read" as two different answers.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use widestring::{U16CStr, U16CString};
use windows::Win32::Devices::PortableDevices::{
    WPD_OBJECT_CONTENT_TYPE, WPD_OBJECT_NAME, WPD_OBJECT_ORIGINAL_FILE_NAME,
};
use winmtp::Provider;
use winmtp::device::Content;
use winmtp::device::device_values::AppIdentifiers;
use winmtp::object::{Object, ObjectType};

use super::{DeviceSource, MediaListing, RemoteObject};
use crate::core::models::DeviceDto;
use crate::core::placement;
use crate::error::AppError;

/// Folder names a phone keeps photos and videos in, probed in order.
///
/// A deliberate whitelist rather than a walk of the whole device: pulling
/// `Android/data` is slow and worthless, and enumerating every object on a modern
/// phone over USB 2 is tens of thousands of round trips. Albums live in one of
/// these. `DCIM/Camera` is covered because the walk recurses into `DCIM`.
const MEDIA_ROOTS: &[&str] = &[
    "Internal shared storage/DCIM",
    "Internal Storage/DCIM",
    "DCIM",
    "Internal shared storage/Pictures",
    "Internal shared storage/Movies",
    "Internal shared storage/Camera",
];

/// Recursion limit for [`walk_media`]. Guards against a driver that reports a
/// cycle or a pathological layout; a phone is never 12 albums deep.
const MAX_DEPTH: usize = 12;

/// Ceiling for the no-album-folder fallback, which walks the storage roots. This
/// path only runs on a layout we do not recognise, and an unbounded walk there
/// would look like a hang rather than a backup.
const FALLBACK_LIMIT: usize = 200_000;

/// How many ids one `IEnumPortableDeviceObjectIDs::Next` call asks for. The API
/// takes a caller-sized buffer and writes how many it filled, so this is a batch
/// size, not a limit on how many children a folder may have.
const ENUM_BATCH: usize = 64;

/// The WPD **software** device interface, `IPortableDeviceSoftwareDevice`
/// (`{65a1c7ff-4417-11f1-945c-806e6f6e6963}`) — spelled out because `winmtp`
/// re-exports the `PortableDevices` module but not this constant.
///
/// Windows raises one of these to mirror something that is not a portable device,
/// and on a machine with any MTP stack installed it lists a *software* device
/// named `Disco Local ` for the machine's own disk (its id starts `\\?\swd#`).
/// It has a storage volume and no camera, and offering it as an import source is
/// worse than offering nothing: the user picks a device that is their hard drive.
/// No real phone or camera carries this interface, so matching the id is exact
/// rather than a heuristic on the name — which matters, because the name is
/// localized by Windows and there is no spelling of it to test for.
const SOFTWARE_DEVICE_INTERFACE: &str = "65a1c7ff-4417-11f1-945c-806e6f6e6963";

/// The concrete WPD transport. Constructing it touches nothing: no COM happens
/// until a method is called, so this can be a `Default` singleton.
#[derive(Debug, Clone, Copy, Default)]
pub struct WpdDeviceSource;

/// The identity Windows shows in its device list. Built explicitly rather than
/// with `winmtp::make_current_app_identifiers!`, whose own docs say it must be
/// called from a binary — it reads `CARGO_PKG_NAME`, which from inside this lib
/// would advertise the crate name, not the product.
fn app_identifiers() -> AppIdentifiers {
    let (major, minor, patch) = parse_version(env!("CARGO_PKG_VERSION"));
    AppIdentifiers {
        app_name: "BackupManager".to_string(),
        app_major: major,
        app_minor: minor,
        app_patch: patch,
    }
}

fn parse_version(version: &str) -> (u32, u32, u32) {
    let mut parts = version
        .split(['.', '-'])
        .map(|part| part.parse::<u32>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

/// Runs a COM call that is known to be able to panic, and reports a driver-level
/// failure instead of unwinding out of the job thread.
///
/// The panic payload is dropped on purpose: a COM panic message is noise to a
/// user, and every caller supplies its own `context`.
fn com<T>(context: &str, body: impl FnOnce() -> T) -> Result<T, AppError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(value) => Ok(value),
        Err(_) => Err(AppError::Device(context.to_string())),
    }
}

/// Renders a driver failure, with the reason text **already made safe**.
///
/// `detail` is a `String` on purpose. `windows::core::Error`'s `Display` — which
/// every `winmtp` error enum reaches through `#[error("… ({0})")] Windows(#[from]
/// crate::WindowsError)` — calls `HRESULT::message()`, and that calls
/// `FormatMessageW` and then builds a slice from whatever pointer came back
/// (`windows-core-0.52.0/src/hresult.rs:90`). When the system has no message for
/// the code, `FormatMessageW` returns 0 with a null pointer, the slice is built
/// from null, and Rust's precondition check aborts the process. It is a
/// **non-unwinding** panic, so [`com`]'s `catch_unwind` and [`com_thread`]'s
/// `join` cannot contain it: the whole app dies with `STATUS_STACK_BUFFER_OVERRUN`
/// the first time a WPD call fails with a code Windows cannot describe. WPD
/// raises such codes routinely.
///
/// Taking a `String` makes the safe path the only one that compiles.
fn mtp(context: &str, detail: String) -> AppError {
    AppError::Device(format!("{context}: {detail}"))
}

/// The one place allowed to turn a `windows::core::Error` into text.
///
/// `FormatMessageW` is called directly, with the null check `HRESULT::message()`
/// omits: on failure it returns 0 and the caller must use no text at all. The
/// numeric code is always present, so an un describable code still reaches the
/// user as something searchable rather than as an empty sentence.
///
/// The text is whatever the machine's locale says — this app is pt-BR and the
/// system answers in the system language, which is the correct trade for a
/// diagnostic the user is asked to report.
fn hresult_text(error: &windows::core::Error) -> String {
    let code = error.code();
    let mut buffer = [0u16; 512];
    let written = unsafe {
        windows::Win32::System::Diagnostics::Debug::FormatMessageW(
            windows::Win32::System::Diagnostics::Debug::FORMAT_MESSAGE_FROM_SYSTEM
                | windows::Win32::System::Diagnostics::Debug::FORMAT_MESSAGE_IGNORE_INSERTS,
            None,
            windows::core::HRESULT::from_win32(code.0 as u32).0 as u32,
            0,
            windows::core::PWSTR(buffer.as_mut_ptr()),
            buffer.len() as u32,
            None,
        )
    };
    // 0 means "no text for this code" — and a zero-length buffer is exactly the
    // null slice that killed the app.
    let text = if written == 0 {
        String::new()
    } else {
        let end = buffer[..written as usize]
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(written as usize);
        String::from_utf16_lossy(&buffer[..end])
    };
    let text = text.trim();
    if text.is_empty() {
        format!("HRESULT 0x{:08X}", code.0 as u32)
    } else {
        format!("{text} (0x{:08X})", code.0 as u32)
    }
}

/// `MtpError`'s `Windows` variant formats the same `windows::core::Error`, so the
/// enum cannot be `Display`ed either; its other variants are fixed strings.
fn mtp_error_text(error: &winmtp::error::MtpError) -> String {
    match error {
        winmtp::error::MtpError::Windows(inner) => hresult_text(inner),
        other => other.to_string(),
    }
}

/// [`mtp_error_text`] for the stream-opening failure. Same reason, same shape.
fn open_stream_error_text(error: &winmtp::error::OpenStreamError) -> String {
    match error {
        winmtp::error::OpenStreamError::Windows(inner) => hresult_text(inner),
        other => other.to_string(),
    }
}

/// Opens the one `Provider` a call needs. Fact 1: it must stay on this thread.
fn provider() -> Result<Provider, AppError> {
    Provider::new().map_err(|error| mtp("MTP indisponível neste sistema", hresult_text(&error)))
}

/// Runs one COM call on a thread whose apartment this module owns.
///
/// Fact 1 said COM belongs to a thread, and `commands/usb.rs` already honours
/// that — but `devices_list` is called straight from the UI, and Tauri runs a
/// **synchronous** command on the main thread, which WebView2 has already put in
/// an STA. `CoInitializeEx(COINIT_MULTITHREADED)` there fails with
/// `RPC_E_CHANGED_MODE`, so every enumeration from the app failed while the very
/// same call from an ordinary thread worked — a bug this file's own doc comment
/// describes and its code did not implement.
///
/// The fix is not an apartment flag this crate does not expose; it is to make COM
/// unreachable from the caller's thread entirely. Each call gets a fresh thread,
/// whose apartment is initialized once and dies with it — which also side-steps
/// `Provider::new` never calling `CoUninitialize`, since the thread is never
/// reused.
fn com_thread<T>(body: impl FnOnce() -> Result<T, AppError> + Send + 'static) -> Result<T, AppError>
where
    T: Send + 'static,
{
    std::thread::Builder::new()
        .name("wolfs-mtp".to_string())
        .spawn(body)
        .map_err(|error| {
            AppError::Device(format!("não foi possível iniciar a thread do MTP: {error}"))
        })?
        .join()
        .unwrap_or_else(|_| {
            Err(AppError::Device(
                "a thread do MTP terminou sem resposta".to_string(),
            ))
        })
}

impl WpdDeviceSource {
    /// Opens a device by friendly name and returns its content root.
    ///
    /// The provider must already have been created on this thread (fact 1).
    fn content(provider: &Provider, friendly: &str) -> Result<winmtp::device::Content, AppError> {
        let basic = provider
            .enumerate_devices()
            .map_err(|error| mtp("falha ao procurar aparelhos", mtp_error_text(&error)))?
            .into_iter()
            .find(|device| device.friendly_name() == friendly)
            .ok_or_else(|| {
                AppError::Device(format!(
                    "aparelho não encontrado ou desconectado: {friendly}"
                ))
            })?;

        // Phones present FAT/exFAT volumes, which are case-insensitive; the
        // wrapper's docs note that guessing this wrong makes path lookups fail.
        let opened = com(&format!("não foi possível abrir {friendly}"), || {
            basic.open(&app_identifiers(), false)
        })
        .map_err(|_| AppError::Device(format!("não foi possível abrir {friendly}")))?
        .map_err(|error| {
            mtp(
                &format!("não foi possível abrir {friendly}"),
                hresult_text(&error),
            )
        })?;

        opened.content().map_err(|error| {
            mtp(
                "não foi possível ler o conteúdo do aparelho",
                hresult_text(&error),
            )
        })
    }

    /// Resolves an MTP object id back to an `Object` inside the current session.
    ///
    /// Built by [`object_at`] rather than `Content::object_by_id`, so a file the
    /// device does not name is still reachable for reading **and** for the delete
    /// that follows a proven copy. An object that cannot be resolved here would be
    /// media the pipeline copied and then could never remove from the phone.
    fn object(content: &Content, id: &str) -> Result<Object, AppError> {
        object_at(content, &U16CString::from_str_truncate(id))
            .map_err(|_| AppError::Device(format!("objeto do aparelho não existe mais: {id}")))
    }

    /// One full metadata walk. Called twice by [`DeviceSource::list_media`].
    fn walk_once(
        &self,
        friendly: &str,
        mut on_enum: Option<&mut (dyn FnMut(&str) + '_)>,
        cancel: Option<&crate::core::scanner::CancelFlag>,
    ) -> Result<MediaListing, AppError> {
        let provider = provider()?;
        let content = Self::content(&provider, friendly)?;
        let root = content.root().map_err(|error| {
            mtp(
                "não foi possível ler a raiz do aparelho",
                hresult_text(&error),
            )
        })?;

        let mut listing = MediaListing::default();
        let mut folders: Vec<Object> = Vec::new();

        for candidate in MEDIA_ROOTS {
            // A miss here is expected for most entries, so it is not a failure —
            // but a *read error* on the way is, and it is recorded rather than
            // skipped, or a phone that refuses every enumeration would look exactly
            // like a phone whose folders are somewhere else.
            match object_by_path(&content, &root, Path::new(candidate)) {
                Ok(Some(folder)) if folder.object_type() != ObjectType::FunctionalObject => {
                    folders.push(folder)
                }
                Ok(_) => {}
                Err(error) => listing.unreadable.push(format!("{candidate}: {error}")),
            }
        }

        if folders.is_empty() {
            // Unfamiliar layout: walk the storage roots rather than reporting an
            // empty device, which the user would read as "nothing to back up".
            let children = match child_objects(&content, root.id()) {
                Ok(children) => children,
                Err(error) => {
                    listing.unreadable.push(format!("armazenamentos: {error}"));
                    Vec::new()
                }
            };
            folders.extend(
                children
                    .into_iter()
                    .filter(|child| child.object_type() == ObjectType::FunctionalObject),
            );
        }

        for folder in folders {
            walk_media(
                &content,
                &folder,
                0,
                &mut listing,
                on_enum.as_deref_mut(),
                cancel,
                0,
            )?;
        }

        // Sorted so the two walks compare equal whenever they *saw* the same files.
        // MTP hands objects back in whatever order the driver feels like, so an
        // unsorted list would make fact 3's comparison fail on a stable device —
        // training the user to dismiss a real safety check as flaky.
        listing
            .objects
            .sort_by(|left, right| left.id.cmp(&right.id));
        Ok(listing)
    }
}

/// Windows raises a software device to mirror something that is not a portable
/// device — on any machine with an MTP stack it lists `Disco Local ` for the
/// machine's own disk. See [`SOFTWARE_DEVICE_INTERFACE`].
fn is_software_device(id: &str) -> bool {
    id.to_ascii_lowercase().contains(SOFTWARE_DEVICE_INTERFACE)
}

impl DeviceSource for WpdDeviceSource {
    fn enumerate(&self) -> Result<Vec<DeviceDto>, AppError> {
        // Fact 1, enforced: nothing below this line may run on the caller's thread.
        com_thread(|| {
            let devices = provider()?
                .enumerate_devices()
                .map_err(|error| mtp("falha ao enumerar aparelhos", mtp_error_text(&error)))?;

            Ok(devices
                .into_iter()
                // Windows' own software devices (see `SOFTWARE_DEVICE_INTERFACE`)
                // are not import sources, so they are dropped here rather than
                // offered and then failing when read.
                .filter(|device| !is_software_device(&device.device_id()))
                .map(|device| {
                    let friendly = device.friendly_name().to_string();
                    let name = placement::sanitize_device_label(&friendly).unwrap_or_else(|_| {
                        // A device whose name is entirely unusable characters still
                        // has to be selectable, so fall back to a stable slug rather
                        // than dropping it from the list.
                        format!("Aparelho {}", short_id(&device.device_id()))
                    });
                    DeviceDto {
                        id: device.device_id(),
                        name,
                        friendly_name: friendly,
                    }
                })
                .collect())
        })
    }

    /// Lists every media file on the device, verified (fact 3).
    fn list_media(
        &self,
        friendly: &str,
        _on_enum: Option<&mut dyn FnMut(&str)>,
        cancel: Option<&crate::core::scanner::CancelFlag>,
    ) -> Result<MediaListing, AppError> {
        let friendly = friendly.to_string();
        // The callback cannot cross into the spawned thread: `com_thread`'s
        // closure must be `Send + 'static`, and a `&mut dyn FnMut` is neither.
        // Nothing real depends on it — the policy layer always calls with `None`
        // (usb.rs `ingest_from`), and the pane's enumeration coverage comes from
        // the indeterminate `Pull {current: 0, total: 0}` the command emits
        // before this call (§11.3).
        let cancel = cancel.cloned();
        com_thread(move || {
            // Both walks on the *same* thread: fact 1 again — re-initializing COM
            // per walk would risk a different apartment answering a different view
            // of the phone, which is the one thing a comparison like this cannot
            // survive.
            let first = WpdDeviceSource.walk_once(&friendly, None, cancel.as_ref())?;
            let second = WpdDeviceSource.walk_once(&friendly, None, cancel.as_ref())?;

            if first.objects != second.objects {
                // Not a partial backup with a warning: an unknown set of files is
                // precisely the situation where deleting from the phone would destroy
                // media the user believes is safe.
                return Err(AppError::Device(format!(
                    "a leitura de {friendly} foi interrompida: dois levantamentos \
                     divergiram ({} e {} arquivos). Reconecte o cabo e repita a \
                     importação. Nada foi apagado do aparelho.",
                    first.objects.len(),
                    second.objects.len()
                )));
            }

            // Both walks agreed and found nothing. Which of the two "empty" cases is
            // it? They are not the same answer and must not be reported as one.
            if first.objects.is_empty() {
                // (a) Nothing could be read at all. The driver's own words go to the
                // user, because "MTP device not available" and "the phone is locked"
                // are one HRESULT to a human reading a list of photos.
                if let Some(reason) = first.unreadable.first() {
                    return Err(AppError::Device(format!(
                        "nenhum arquivo de {friendly} pôde ser lido: {reason}. \
                         Desbloqueie o aparelho, aceite a confiança do computador \
                         e repita. Nada foi apagado."
                    )));
                }
                // (b) Everything was readable, and there was nothing in it.
                return Err(AppError::Device(format!(
                    "o aparelho {friendly} não expôs nenhum arquivo de mídia. \
                     Se ele tem fotos, reconecte o cabo e confirme que a câmera \
                     está nas Configurações do telefone, depois repita."
                )));
            }

            // Files were found, so the pass is real. Anything the walk could not
            // read travels with it as a warning rather than a failure (§12.1: one
            // unreadable album must not abandon the other 4 000 photos).
            Ok(first)
        })
    }

    fn read_to_abs(
        &self,
        friendly: &str,
        object: &RemoteObject,
        staged: &Path,
    ) -> Result<u64, AppError> {
        let friendly = friendly.to_string();
        let object = object.clone();
        let staged = staged.to_path_buf();
        com_thread(move || {
            let provider = provider()?;
            let content = WpdDeviceSource::content(&provider, &friendly)?;
            let target = WpdDeviceSource::object(&content, &object.id)?;

            let mut opened = com(&format!("não foi possível ler {}", object.name), || {
                target.open_read_stream()
            })
            .map_err(|_| AppError::Device(format!("não foi possível ler {}", object.name)))?
            .map_err(|error| {
                mtp(
                    &format!("nao foi possivel ler {}", object.name),
                    open_stream_error_text(&error),
                )
            })?;

            // The file is created here and nowhere else: bytes go from the COM stream
            // straight into a `.part` under `Database/staging/usb/`, so an
            // interrupted pull cannot leave a half-written file in `Media/`.
            let output =
                File::create(&staged).map_err(|error| AppError::Ingest(error.to_string()))?;
            let mut sink = BufWriter::new(output);
            let written = std::io::copy(&mut opened, &mut sink).map_err(|error| {
                AppError::Device(format!("falha ao ler {}: {error}", object.name))
            })?;
            sink.flush()
                .map_err(|error| AppError::Ingest(error.to_string()))?;

            Ok(written)
        })
    }

    fn delete(&self, friendly: &str, object: &RemoteObject) -> Result<(), AppError> {
        let friendly = friendly.to_string();
        let object = object.clone();
        com_thread(move || {
            let provider = provider()?;
            let content = WpdDeviceSource::content(&provider, &friendly)?;
            let mut target = WpdDeviceSource::object(&content, &object.id)?;

            // The one call in this codebase that can destroy data on a device, and
            // the one most likely to panic on a busy phone — hence the guard.
            com(&format!("falha ao apagar {}", object.name), || {
                target.delete(false)
            })
            .map_err(|_| AppError::Device(format!("falha ao apagar {}", object.name)))?
            .map_err(|error| {
                mtp(
                    &format!("falha ao apagar {}", object.name),
                    hresult_text(&error),
                )
            })
        })
    }
}

/// Recursively collects media-extension files under `folder`, bounded by
/// [`MAX_DEPTH`].
///
/// Media-ness is decided by extension here, not by the device's content type:
/// drivers lie about it (a Kindle reports an `.m3u` playlist as `Unspecified`)
/// and §7.3's pipeline filters on extension anyway, so one rule lives in
/// `placement::is_media` and both transports use it.
///
/// A folder whose children cannot be enumerated is recorded and the walk carries
/// on — one unreadable album must not abandon the other 4 000 photos — but it is
/// **recorded**, which the first version of this function did not do: it returned
/// `Ok(())` and the unreadable folder became indistinguishable from an empty one.
/// That is what let a phone WPD refused to enumerate report "0 arquivos".
fn walk_media(
    content: &Content,
    folder: &Object,
    depth: usize,
    listing: &mut MediaListing,
    mut on_enum: Option<&mut (dyn FnMut(&str) + '_)>,
    cancel: Option<&crate::core::scanner::CancelFlag>,
    mut emit_count: u32,
) -> Result<(), AppError> {
    if depth > MAX_DEPTH {
        return Ok(());
    }

    if cancel.is_some_and(|flag| flag.is_cancelled()) {
        return Err(AppError::Cancelled);
    }

    let children = match child_objects(content, folder.id()) {
        Ok(children) => children,
        Err(error) => {
            listing
                .unreadable
                .push(format!("{}: {error}", folder.name().to_string_lossy()));
            return Ok(());
        }
    };

    for child in children {
        if cancel.is_some_and(|flag| flag.is_cancelled()) {
            return Err(AppError::Cancelled);
        }
        if child.object_type().is_file_like() {
            let name = child.name().to_string_lossy();
            if !placement::is_media(&name) {
                continue;
            }
            let size = child
                .properties(&[winmtp::PortableDevices::WPD_OBJECT_SIZE])
                .ok()
                .and_then(|values| {
                    values
                        .get_u64(&winmtp::PortableDevices::WPD_OBJECT_SIZE)
                        .ok()
                })
                .unwrap_or(0);
            listing.objects.push(RemoteObject {
                id: child.id().to_string_lossy(),
                name,
                size,
            });
            if let Some(on_enum) = on_enum.as_deref_mut() {
                if emit_count.is_multiple_of(64) {
                    on_enum(&child.name().to_string_lossy());
                }
                emit_count += 1;
            }
        } else {
            walk_media(
                content,
                &child,
                depth + 1,
                listing,
                on_enum.as_deref_mut(),
                cancel,
                emit_count,
            )?;
        }
    }

    // The fallback walk is the only unbounded one; see [`FALLBACK_LIMIT`].
    if listing.objects.len() > FALLBACK_LIMIT {
        return Err(AppError::Device(format!(
            "a pasta do aparelho tem mais de {FALLBACK_LIMIT} arquivos e o \
             layout não foi reconhecido. Importação interrompida para não \
             percorrer o aparelho inteiro."
        )));
    }

    Ok(())
}

/// The direct children of `parent`, read from the driver.
///
/// `winmtp`'s own `Object::children` cannot be used on a real iPhone, and the way
/// it fails is the reason this function exists. It walks
/// `IEnumPortableDeviceObjectIDs` and turns each id into an `Object` with
/// `Content::object_by_id`, which reads three properties in one `GetValues` and
/// then **requires** `WPD_OBJECT_NAME` to be a readable string
/// (`winmtp-0.3.2/src/device/content.rs:56`). iOS 18 stores the camera roll in
/// date-named folders that expose only `WPD_OBJECT_ORIGINAL_FILE_NAME`; the read
/// fails with `0x80070490`, and the iterator's `.ok()` cannot tell that apart from
/// "no more objects", so it simply stops. The caller then sees **zero children and
/// no error** on a phone holding thousands of photos.
///
/// Two of those walks fail identically, which is why the two-walk check in
/// [`list_media`] could not catch it: an invariant that holds for a broken reader
/// looks exactly like an invariant that holds.
///
/// So the ids are taken straight from the enumerator, where a failure is a
/// failure, and each object is built by [`object_at`], which reads only what the
/// object actually has.
fn child_objects(content: &Content, parent: &U16CStr) -> Result<Vec<Object>, AppError> {
    let enumerator = unsafe {
        content
            .com_object()
            .EnumObjects(0, windows::core::PCWSTR::from_raw(parent.as_ptr()), None)
    }
    .map_err(|error| {
        mtp(
            &format!(
                "não foi possível ler o conteúdo de {}",
                parent.to_string_lossy()
            ),
            hresult_text(&error),
        )
    })?;

    let mut ids: Vec<U16CString> = Vec::new();
    let mut buffer = [windows::core::PWSTR::null(); ENUM_BATCH];
    loop {
        let mut fetched: u32 = 0;
        // A failed `Next` ends the walk *loudly*. This is the whole point of the
        // function: the crate's iterator drops this HRESULT on the floor.
        unsafe { enumerator.Next(&mut buffer, &mut fetched as *mut u32) }
            .ok()
            .map_err(|error| {
                mtp(
                    &format!(
                        "a leitura de {} foi interrompida pelo aparelho",
                        parent.to_string_lossy()
                    ),
                    hresult_text(&error),
                )
            })?;

        for id in buffer.iter().take(fetched as usize) {
            let text = unsafe { id.to_string() }.map_err(|error| {
                AppError::Device(format!(
                    "identificador de objeto ilegível no aparelho: {error}"
                ))
            })?;
            ids.push(U16CString::from_str_truncate(&text));
        }

        if fetched == 0 {
            break;
        }
    }

    ids.iter()
        .map(|id| object_at(content, id))
        .collect::<Result<Vec<Object>, AppError>>()
}

/// Builds one `Object` from an id, reading only the properties it can read.
///
/// The difference from `Content::object_by_id` is the name. `WPD_OBJECT_NAME` is
/// asked for on its own and is allowed to be absent, because some real folders do
/// not have it; `WPD_OBJECT_ORIGINAL_FILE_NAME` is the second choice and the id is
/// the last, so an object with no readable name at all is still listed (and so
/// still deletable) instead of vanishing. `WPD_OBJECT_CONTENT_TYPE` stays
/// mandatory: without it there is no way to tell a folder from a file, which is the
/// one decision the walk cannot make up.
///
/// [`Object::new`] is public in `winmtp` 0.3.2, and both transfer and delete
/// address an object by id alone, so an object built here behaves exactly like one
/// the crate built.
fn object_at(content: &Content, id: &U16CString) -> Result<Object, AppError> {
    let content_type = content
        .properties(id, &[WPD_OBJECT_CONTENT_TYPE])
        .and_then(|values| values.get_guid(&WPD_OBJECT_CONTENT_TYPE))
        .map_err(|error| {
            mtp(
                &format!("objeto {id:?} do aparelho não pôde ser lido"),
                hresult_text(&error),
            )
        })?;

    let original_file_name = read_name(content, id, WPD_OBJECT_ORIGINAL_FILE_NAME);
    let name = read_name(content, id, WPD_OBJECT_NAME)
        .or_else(|| original_file_name.clone())
        .unwrap_or_else(|| U16CString::from_str_truncate(id.to_string_lossy()));

    Ok(Object::new(
        content.clone(),
        id.clone(),
        name,
        original_file_name,
        ObjectType::from_guid(content_type),
    ))
}

/// One optional string property. `Ok(None)` means "the object does not have it",
/// which for [`WPD_OBJECT_NAME`] is ordinary rather than broken.
fn read_name(content: &Content, id: &U16CString, key: winmtp::PROPERTYKEY) -> Option<U16CString> {
    content
        .properties(id, &[key])
        .ok()
        .and_then(|values| values.get_string(&key).ok())
}

/// Resolves a `/`-separated path below `parent`, one segment at a time.
///
/// `Object::object_by_path` is unusable here for the same reason
/// `Object::children` is: it resolves every segment through `object_by_id`. On the
/// iPhone that made every entry in [`MEDIA_ROOTS`] answer `NotFound`, so the
/// whitelist silently stopped working and every device took the slow fallback walk.
///
/// `Ok(None)` is a genuine miss (this folder has no child by that name), which is
/// the expected outcome for most entries of [`MEDIA_ROOTS`] on most phones.
/// `Err` is a real read failure and is never reported as a miss.
fn object_by_path(
    content: &Content,
    parent: &Object,
    path: &Path,
) -> Result<Option<Object>, AppError> {
    let mut current = parent.clone();
    for segment in path.components() {
        let Some(name) = segment.as_os_str().to_str() else {
            return Ok(None);
        };
        let found = child_objects(content, current.id())?
            .into_iter()
            .find(|child| child.name().to_string_lossy().eq_ignore_ascii_case(name));
        match found {
            Some(child) => current = child,
            None => return Ok(None),
        }
    }
    Ok(Some(current))
}

/// A short, filesystem-safe stand-in for a device id, used only when a device's
/// friendly name is entirely unusable characters.
fn short_id(id: &str) -> String {
    id.chars()
        .filter(char::is_ascii_alphanumeric)
        .take(6)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this pins is an **abort**, not a wrong string: formatting a
    /// `windows::core::Error` whose code Windows cannot describe builds a slice
    /// from a null pointer (`windows-core-0.52.0/src/hresult.rs:90`), which is a
    /// non-unwinding panic that `catch_unwind` cannot contain and that killed the
    /// whole app on the first WPD failure.
    ///
    /// So this asserts the *code always survives*, for a code the system knows and
    /// for codes it does not. If someone routes these back through `Display` the
    /// test binary aborts instead of failing, which is the honest outcome: the
    /// failure mode being guarded is a process that cannot report anything.
    #[test]
    fn driver_errors_are_rendered_without_aborting() {
        let known = windows::core::Error::from(windows::core::HRESULT(0x8007_0490u32 as i32));
        assert!(
            hresult_text(&known).contains("0x80070490"),
            "a readable Win32 code must still name itself"
        );

        // WPD-specific codes, which Windows generally has no message for. Whether
        // this machine can describe any of them is beside the point: asking must
        // never abort.
        for raw in [0x8004_2000u32, 0x8004_2101, 0x8000_0001] {
            let error = windows::core::Error::from(windows::core::HRESULT(raw as i32));
            let text = hresult_text(&error);
            assert!(
                text.contains(&format!("0x{raw:08X}")),
                "an undescribable code must still reach the user: {text}"
            );
        }
    }

    /// The crate's three tree-reading calls cannot be used on a real phone, and
    /// the way they fail is invisible: an iPhone full of photos enumerates as
    /// *zero children and no error*, because a folder that does not expose
    /// `WPD_OBJECT_NAME` ends the iterator instead of failing the walk. Two such
    /// walks agree with each other, so every check above still passed.
    ///
    /// There is no unit test for that — it needs the phone — so the invariant is
    /// pinned against this file's own source instead: [`child_objects`] and
    /// [`object_at`] replace them, and this test fails if one of the originals
    /// comes back. `Provider::enumerate_devices`, `Content::root` and the transfer
    /// and delete calls stay: they address the device or one object by id and do
    /// not walk a tree.
    #[test]
    fn the_crate_tree_walkers_are_not_used_again() {
        // Only the production half: this test names every banned call, so scanning
        // the whole file would always find them.
        let production = include_str!("wpd.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("the test module is always last");
        for banned in [
            ".children()",
            ".object_by_id(",
            ".functional_objects()",
            ".object_by_path(",
        ] {
            assert!(
                !production.contains(banned),
                "`{banned}` is back in the transport: it truncates silently on a \
                 phone whose folders have no WPD_OBJECT_NAME. Use `child_objects` / \
                 `object_at` instead."
            );
        }
    }

    #[test]
    fn the_app_identity_is_built_not_borrowed_from_the_crate_name() {
        let identifiers = app_identifiers();
        assert_eq!(identifiers.app_name, "BackupManager");
        // Must come from `CARGO_PKG_VERSION`, not from zeroes: Windows shows this
        // in the device list, and a 0.0.0 app is indistinguishable from a driver
        // that failed to identify itself.
        assert_eq!(
            (
                identifiers.app_major,
                identifiers.app_minor,
                identifiers.app_patch
            ),
            parse_version(env!("CARGO_PKG_VERSION"))
        );
        assert_ne!(identifiers.app_name, env!("CARGO_PKG_NAME"));
    }

    #[test]
    fn version_parsing_tolerates_a_prerelease_suffix() {
        assert_eq!(parse_version("0.1.0"), (0, 1, 0));
        assert_eq!(parse_version("1.2.3-beta.1"), (1, 2, 3));
        assert_eq!(parse_version("nonsense"), (0, 0, 0));
    }

    /// Both ids are verbatim from the 2026-10-04 probe on the human's machine, so
    /// this pins the filter against what the device manager actually returned
    /// rather than against a shape I assumed. Windows listed `Disco Local ` — its
    /// own software device for the local disk — next to the iPhone, and offering
    /// the user's hard drive as an import source is worse than offering nothing.
    #[test]
    fn windows_software_devices_are_not_import_sources() {
        assert!(is_software_device(
            "\\\\?\\swd#wpdbusenum#{65a1c7ff-4417-11f1-945c-806e6f6e6963}#0000000001000000\
             #{6ac27878-a6fa-4155-ba85-f98f491d4f33}"
        ));
        // Case-insensitive: the GUID's spelling is not guaranteed.
        assert!(is_software_device(
            "\\\\?\\SWD#WPDBUSENUM#{65A1C7FF-4417-11F1-945C-806E6F6E6963}"
        ));
        // The iPhone, same probe, same minute.
        assert!(!is_software_device(
            "\\\\?\\usb#vid_05ac&pid_12a8&mi_00#7&7af9127&0&0000\
             #{6ac27878-a6fa-4155-ba85-f98f491d4f33}"
        ));
    }

    #[test]
    fn a_device_id_survives_being_filtered_into_a_filename() {
        // The property that matters: punctuation dropped, length bounded, and two
        // devices that differ within the first six characters keep different
        // slugs. Asserting the exact string instead would just be a hand-count.
        let first = short_id("o2C::o1ABC-123");
        let second = short_id("o2C::o9XYZ-999");

        assert_eq!(first.chars().count(), 6);
        assert_eq!(second.chars().count(), 6);
        assert!(
            first
                .chars()
                .chain(second.chars())
                .all(|ch| ch.is_ascii_alphanumeric())
        );
        assert_ne!(first, second);
        assert_eq!(short_id("..."), "");
    }

    /// The fix for the hardware bug (2026-10-04: every `devices_list` from the app
    /// failed while the same call from a test thread succeeded) is that the COM body
    /// never runs on the caller's thread. This pins the mechanism, and needs no
    /// phone — which is the point: the failure it prevents cannot be reproduced any
    /// other way.
    #[test]
    fn a_com_body_never_runs_on_the_calling_thread() {
        let caller = std::thread::current().id();
        let inner = com_thread(|| Ok(std::thread::current().id())).expect("body ran");
        assert_ne!(inner, caller);
    }

    /// `join()` on a panicking body must not unwind the caller: a panic inside a
    /// driver's COM call is the failure mode `com()` exists to contain, and the
    /// thread wrapper is the outer wall around it.
    #[test]
    fn a_panicking_com_body_becomes_an_error() {
        let result: Result<(), AppError> = com_thread(|| panic!("driver exploded"));
        assert!(matches!(result, Err(AppError::Device(_))), "{result:?}");
        let Err(AppError::Device(message)) = result else {
            panic!("expected a device error")
        };
        assert!(message.contains("MTP"), "{message}");
    }

    /// The panic guard is the whole reason `delete` is survivable, so it is
    /// pinned without a phone: `winmtp`'s internal `.unwrap()` is unreachable
    /// here, but our wrapper's behaviour is what the safety argument rests on.
    #[test]
    fn a_com_panic_becomes_an_error_instead_of_unwinding() {
        let result = com("boom", || panic!("simulated driver panic"));
        assert!(matches!(result, Err(AppError::Device(message)) if message == "boom"));
    }
}
