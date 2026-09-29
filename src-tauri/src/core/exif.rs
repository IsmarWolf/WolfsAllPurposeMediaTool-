//! Capture date + GPS extraction (PLAN §7.4).
//!
//! [`extract`] is the single entry point: it classifies by extension, reads the
//! EXIF block of an image (JPEG, HEIC/HEIF, AVIF, PNG, WebP, TIFF — whatever
//! `exif::Reader::read_from_container` recognizes), asks ffmpeg for the
//! `creation_time` of a video, and falls back to the file mtime.
//!
//! The degradation rule of §7.4 governs everything here: a file that cannot be
//! parsed loses its metadata, it never fails the scan. Only two conditions are
//! reported as errors, because both belong to the *host* and not to the file:
//! a file that cannot be read at all, and an ffmpeg that cannot be launched.
//!
//! ## Decisions taken in c6 (recorded in PLAN §7.4.1)
//!
//! 1. **Images keep the local wall clock.** `DateTimeOriginal` is written
//!    without a zone, and the `Media/<device>/<YYYY>/<MM>` tree of §7.3.5 has
//!    to land on the month the human remembers. `OffsetTimeOriginal` is
//!    deliberately **not** applied — converting would need a zone the file does
//!    not carry, and §6.1 stores a naive `DATETIME`.
//! 2. **Videos are converted to a wall clock, so photos and videos share one
//!    calendar.** ffprobe reports the MOV/MP4 `mvhd` clock in UTC and the file
//!    names no zone, so the value is read as UTC and converted with the
//!    machine's zone at that instant. When a container *does* name its zone
//!    (`+HH:MM`) that one wins: a wall clock the file states beats one we guess.
//!    The result is a naive local time like an image's, so §7.3.5's
//!    `Media/<device>/<YYYY>/<MM>` tree and §11's date facets put a clip recorded
//!    at 23:50 on the 31st in the month it belongs to. It is computed once, at
//!    scan time, and stored — nothing downstream re-derives it.
//! 3. **mtime is the last resort and is always flagged.** A copy carries the
//!    copy time, so an mtime date never counts as metadata (C7 — see
//!    [`RawMetadata::has_metadata`]).
//! 4. **`DateTimeDigitized` (0x9004) is the "CreateDate" of §7.4** — same bytes,
//!    the other name every camera writes.
//! 5. **Sub-seconds are dropped** (`SubSecTimeOriginal` is not read): §6.1 stores
//!    a second-resolution `DATETIME`.
//!
//! ffmpeg is *not* discovered here: the caller passes `App/bin/ffmpeg.exe`
//! (§4.1) as `Option<&Path>`, so this module never has to know the root (C2/C5).

use std::fs::{self, File};
use std::io::BufReader;
use std::path::Path;
use std::process::Command;

use chrono::{DateTime, FixedOffset, Local, NaiveDate, NaiveDateTime, TimeZone};
use exif::{Exif, In, Reader, Tag, Value};
use thiserror::Error;

use crate::error::SpawnError;

/// The extensions §7.3 routes to the EXIF reader. TIFF/RAW share the path
/// because they *are* TIFF, which the reader parses natively.
pub const IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "heic", "heif", "avif", "tif", "tiff", "webp",
];

/// The extensions §7.3 routes to ffmpeg for the `creation_time`.
pub const VIDEO_EXTS: &[&str] = &["mov", "mp4", "m4v", "3gp", "avi", "mkv"];

/// What a file says about itself, before the DB type of §6.1 is known.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawMetadata {
    /// Capture date, naive and local — the wall clock of the moment, for a photo
    /// and for a video alike (§7.4.1 decisions 1 and 2 above).
    pub captured_at: Option<NaiveDateTime>,
    /// `(latitude, longitude)` in decimal degrees, negative in the S/W
    /// hemispheres.
    pub gps: Option<(f64, f64)>,
    /// `captured_at` came from the file mtime, not from the file's own metadata.
    pub date_from_fs: bool,
}

impl RawMetadata {
    /// C7 / §7.3.4: a file is "sem metadados" when it has **neither** a date of
    /// its own **nor** a position. An mtime date is not a date of its own, so it
    /// never lifts the file out of `Sem_Metadados/` — only a real capture date
    /// or a GPS fix does.
    pub fn has_metadata(&self) -> bool {
        (self.captured_at.is_some() && !self.date_from_fs) || self.gps.is_some()
    }

    /// Fills the mtime last resort, flagged, and only when nothing better was
    /// read. A failing `stat` is not worth an error: the file was already read
    /// (or was not an image at all) and §7.4 prefers a missing date to a failed
    /// scan.
    fn with_mtime_fallback(mut self, path: &Path) -> Self {
        if self.captured_at.is_none()
            && let Some(mtime) = mtime(path)
        {
            self.captured_at = Some(mtime);
            self.date_from_fs = true;
        }
        self
    }
}

#[derive(Debug, Error)]
pub enum ExifError {
    /// The file exists in the walker's eyes but cannot be read. This is a
    /// permission/locked-file problem, not a metadata problem.
    #[error("could not read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// ffmpeg is missing at the given path or refused to start — E_FFMPEG
    /// upstream, which the caller turns into "disable video dates" (§8.2).
    #[error(transparent)]
    Spawn(#[from] SpawnError),
}

/// Reads the metadata of one media file.
///
/// `ffmpeg` is `App/bin/ffmpeg.exe` when the tree has one (§4.1, §5.5) and
/// `None` when it does not: a video without ffmpeg loses its date and falls
/// back to the mtime instead of failing, because a fresh tree must still scan
/// (§16.2).
pub fn extract(path: &Path, ffmpeg: Option<&Path>) -> Result<RawMetadata, ExifError> {
    let ext = extension(path);
    if VIDEO_EXTS.contains(&ext.as_str()) {
        video_metadata(path, ffmpeg)
    } else if IMAGE_EXTS.contains(&ext.as_str()) {
        image_metadata(path)
    } else {
        // Not a media extension: the §7.3 walker never routes one here, and
        // guessing would parse arbitrary bytes as EXIF.
        Ok(RawMetadata::default().with_mtime_fallback(path))
    }
}

/// The extension of `path`, lowercased — `IMG_0001.HEIC` is `.heic` whichever
/// device wrote it.
fn extension(path: &Path) -> String {
    path.extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase()
}

fn image_metadata(path: &Path) -> Result<RawMetadata, ExifError> {
    let file = File::open(path).map_err(|source| ExifError::Read {
        path: path.display().to_string(),
        source,
    })?;
    let mut reader = BufReader::new(file);
    let parsed = match Reader::new()
        .continue_on_error(true)
        .read_from_container(&mut reader)
    {
        Ok(exif) => Ok(exif),
        // A broken field must not cost the whole file: `continue_on_error`
        // hands back what did parse plus the list of failures.
        Err(err) => err.distill_partial_result(|errors| {
            eprintln!(
                "[wolfsmedia] {} partial EXIF: {} field(s) skipped",
                path.display(),
                errors.len()
            );
        }),
    };
    let meta = match parsed {
        Ok(exif) => from_exif(&exif),
        // No EXIF block, an unsupported container, or a truncated file: §7.4
        // degrades to empty metadata, never to a failed scan.
        Err(_) => RawMetadata::default(),
    };
    Ok(meta.with_mtime_fallback(path))
}

fn video_metadata(path: &Path, ffmpeg: Option<&Path>) -> Result<RawMetadata, ExifError> {
    // GPS is not read from MOV/MP4 in v1 (tagged best-effort, §20): a position
    // there needs per-frame parsing that c6 does not buy.
    let captured_at = match ffmpeg {
        Some(exe) => creation_time(exe, path)?.map(|stamp| stamp.local(machine_offset)),
        None => None,
    };
    Ok(RawMetadata {
        captured_at,
        gps: None,
        date_from_fs: false,
    }
    .with_mtime_fallback(path))
}

fn from_exif(exif: &Exif) -> RawMetadata {
    RawMetadata {
        captured_at: exif_date(exif),
        gps: exif_gps(exif),
        date_from_fs: false,
    }
}

/// §7.4 precedence: `DateTimeOriginal` → `CreateDate` → mtime (decision 4
/// explains the 0x9004 name).
fn exif_date(exif: &Exif) -> Option<NaiveDateTime> {
    date_field(exif, Tag::DateTimeOriginal).or_else(|| date_field(exif, Tag::DateTimeDigitized))
}

/// An EXIF date is 19 ASCII characters in `YYYY:MM:DD HH:MM:SS`, with no zone
/// (decision 1). `exif::DateTime` checks the shape only, so the calendar range
/// is checked here: a `month = 13` must become a missing date, not a panic three
/// layers up.
fn date_field(exif: &Exif, tag: Tag) -> Option<NaiveDateTime> {
    let bytes = ascii_first(&exif.get_field(tag, In::PRIMARY)?.value)?;
    let date = exif::DateTime::from_ascii(bytes).ok()?;
    NaiveDate::from_ymd_opt(
        i32::from(date.year),
        u32::from(date.month),
        u32::from(date.day),
    )?
    .and_hms_opt(
        u32::from(date.hour),
        u32::from(date.minute),
        u32::from(date.second),
    )
}

/// The GPS IFD in decimal degrees with the hemisphere of the `Ref` tags applied.
/// A pair is returned only when both halves are usable: half a position is not a
/// position.
fn exif_gps(exif: &Exif) -> Option<(f64, f64)> {
    let latitude = dms_field(exif, Tag::GPSLatitude, Tag::GPSLatitudeRef)?;
    let longitude = dms_field(exif, Tag::GPSLongitude, Tag::GPSLongitudeRef)?;
    // The range test also rejects the NaN that a zero denominator produces.
    if !(-90.0..=90.0).contains(&latitude) || !(-180.0..=180.0).contains(&longitude) {
        return None;
    }
    Some((latitude, longitude))
}

fn dms_field(exif: &Exif, tag: Tag, reference: Tag) -> Option<f64> {
    let parts = rationals(&exif.get_field(tag, In::PRIMARY)?.value)?;
    // The degrees field is unsigned with the sign in `Ref`; `abs` tolerates the
    // broken files that put the sign in both.
    let degrees = parts.first().copied()?.abs();
    let minutes = parts.get(1).copied().unwrap_or(0.0) / 60.0;
    let seconds = parts.get(2).copied().unwrap_or(0.0) / 3600.0;
    let magnitude = degrees + minutes + seconds;
    // A missing `Ref` means N/E per the specification's default, and a
    // coordinate is negative only when its own tag says so.
    let south = matches!(ref_byte(exif, reference), Some(b'S'));
    let west = matches!(ref_byte(exif, reference), Some(b'W'));
    Some(if south || west { -magnitude } else { magnitude })
}

fn ref_byte(exif: &Exif, reference: Tag) -> Option<u8> {
    ascii_first(&exif.get_field(reference, In::PRIMARY)?.value)
        .and_then(<[u8]>::first)
        .copied()
}

/// The rationals of a field as `f64`. A zero denominator is `NaN` in the crate's
/// arithmetic and is treated as a broken field here, not as a position.
fn rationals(value: &Value) -> Option<Vec<f64>> {
    match value {
        Value::Rational(values) => values
            .iter()
            .map(|r| (r.denom != 0).then(|| r.to_f64()))
            .collect(),
        Value::SRational(values) => values
            .iter()
            .map(|r| (r.denom != 0).then(|| r.to_f64()))
            .collect(),
        _ => None,
    }
}

fn ascii_first(value: &Value) -> Option<&[u8]> {
    match value {
        Value::Ascii(values) => values.first().map(Vec::as_slice),
        _ => None,
    }
}

/// The mtime as local wall-clock time. Best-effort: an unreadable `stat` means no
/// date at all, not a failed file.
fn mtime(path: &Path) -> Option<NaiveDateTime> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    Some(DateTime::<Local>::from(modified).naive_local())
}

/// The machine's zone at `utc`. chrono resolves this from the platform's time
/// zone rules — on Windows out of the registry, DST transitions included — so a
/// clip recorded in a summer month is not read as a winter one. The machine is
/// the only source of a zone for a MOV/MP4: the file does not carry one
/// (decision 2).
fn machine_offset(utc: &NaiveDateTime) -> FixedOffset {
    Local.offset_from_utc_datetime(utc)
}

/// One ffmpeg pass over a video: its `creation_time`, or nothing.
///
/// ffmpeg's exit status is not the answer — it also writes the metadata block
/// for containers it considers partially broken, and §7.4 prefers a missing date
/// over a failed file. Only a failure to *launch* is an error.
fn creation_time(ffmpeg: &Path, path: &Path) -> Result<Option<CreationTime>, ExifError> {
    let output = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-f", "ffmetadata", "-"])
        .output()
        .map_err(|source| SpawnError {
            program: ffmpeg.display().to_string(),
            source,
        })?;
    Ok(parse_creation_time(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

/// A `creation_time` as ffmetadata printed it: the digits, plus the zone the
/// container named — if it named one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreationTime {
    /// The digits are the instant in UTC, *unless* a zone was named, in which
    /// case they are already the wall clock in it.
    digits: NaiveDateTime,
    zone: Option<FixedOffset>,
}

impl CreationTime {
    /// The wall clock of the moment, which is what §6.1 stores.
    ///
    /// A file that states its own zone has already answered, and the machine is
    /// not consulted. Otherwise the digits are read as UTC and `offset_of`
    /// resolves the zone — the machine's, in the app ([`machine_offset`]) —
    /// because a MOV/MP4 carries none. Resolving is a parameter and not a global
    /// read so the conversion is testable without depending on the zone of
    /// whoever runs `cargo test` (decision 2).
    pub fn local(&self, offset_of: impl FnOnce(&NaiveDateTime) -> FixedOffset) -> NaiveDateTime {
        match self.zone {
            Some(_) => self.digits,
            None => offset_of(&self.digits)
                .from_utc_datetime(&self.digits)
                .naive_local(),
        }
    }
}

/// Parses the `creation_time` of an `ffmpeg -f ffmetadata -` block.
///
/// The block is a `;FFMETADATA1` header followed by `key=value` lines whose
/// values escape `\`, `=`, `;`, `#` and newlines. Only one key matters here, but
/// its value still has to be unescaped before it can be read as a date. The
/// fraction is dropped (decision 5); the zone is kept (decision 2).
pub fn parse_creation_time(metadata: &str) -> Option<CreationTime> {
    metadata.lines().find_map(|line| {
        let value = unescape(line.strip_prefix("creation_time=")?)?;
        parse_iso8601(&value)
    })
}

fn unescape(value: &str) -> Option<String> {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            c @ ('\\' | '=' | ';' | '#') => out.push(c),
            'n' => out.push('\n'),
            // A trailing or unknown escape is a broken line, not a date.
            _ => return None,
        }
    }
    Some(out)
}

fn parse_iso8601(value: &str) -> Option<CreationTime> {
    let stamp = value.trim();
    let (head, zone) = stamp.split_at(zone_start(stamp)?);
    let seconds = head.split('.').next()?;
    let digits = NaiveDateTime::parse_from_str(seconds, "%Y-%m-%dT%H:%M:%S").ok()?;
    Some(CreationTime {
        digits,
        zone: parse_zone(zone),
    })
}

/// The zone a ffmetadata stamp named, if it named one.
///
/// `""` and `Z` are the same answer — UTC with no wall clock attached — so both
/// leave `zone` empty and the machine decides (decision 2). `+HH:MM`/`-HH:MM` is
/// the one case where the file states its own wall clock, and that one is kept.
/// Anything else is a stamp this parser does not understand, and an unparsable
/// zone is no reason to drop a date that is otherwise sound.
fn parse_zone(zone: &str) -> Option<FixedOffset> {
    if zone.len() < 6 {
        return None;
    }
    let (sign, digits) = zone.split_at(1);
    let sign = match sign {
        "+" => 1,
        "-" => -1,
        _ => return None,
    };
    let (hours, minutes) = digits.split_once(':')?;
    let seconds = sign * (hours.parse::<i32>().ok()? * 3600 + minutes.parse::<i32>().ok()? * 60);
    FixedOffset::east_opt(seconds)
}

/// Where the zone designator starts, if any. ffprobe writes `Z`; a container
/// that carries a real offset writes `+HH:MM` or `-HH:MM` — and that `-` is only
/// a zone sign after the time, so the dashes of the date are skipped.
fn zone_start(stamp: &str) -> Option<usize> {
    stamp
        .find('+')
        .or_else(|| stamp.find('Z'))
        .or_else(|| stamp.rfind('-').filter(|i| *i > 10))
        .or(Some(stamp.len()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use exif::{Rational, SRational};

    use super::*;

    /// The date every date fixture carries, as EXIF writes it and as the DB
    /// keeps it.
    const DATE: &str = "2024:03:15 22:33:44";
    const DATE_ISO: &str = "2024-03-15T22:33:44";

    /// A real GPS IFD: Dearborn, Michigan, 42° 19' 53.2" N, 83° 02' 58.2" W —
    /// the same numbers as the decimals, written as the rationals a camera
    /// writes (seconds in tenths).
    const LAT_DMS: [(u32, u32); 3] = [(42, 1), (19, 1), (532, 10)];
    const LON_DMS: [(u32, u32); 3] = [(83, 1), (2, 1), (582, 10)];
    const LAT_DEG: f64 = 42.0 + 19.0 / 60.0 + 53.2 / 3600.0;
    const LON_DEG: f64 = 83.0 + 2.0 / 60.0 + 58.2 / 3600.0;

    /// The same coordinates as `SRATIONAL`, the shape some cameras write instead.
    const SIGNED_LAT_DMS: [(i32, i32); 3] = [(42, 1), (19, 1), (532, 10)];
    const SIGNED_LON_DMS: [(i32, i32); 3] = [(83, 1), (2, 1), (582, 10)];

    // ---- the EXIF block every fixture shares ------------------------------

    /// Which tags a fixture writes. Each variant exists to pin one rule, so a
    /// failure names the rule instead of "EXIF broke".
    #[derive(Clone, Copy, PartialEq)]
    enum Fixture {
        /// `DateTimeOriginal` alone — the §7.4 first step.
        Date,
        /// Both date tags, to pin the precedence between them.
        BothDates,
        /// Date + GPS, hemisphere N/E.
        DateGps,
        /// Date + GPS, hemisphere S/W.
        DateGpsSouthWest,
        /// Date + GPS written as *signed* rationals, which some cameras do.
        DateGpsSigned,
        /// GPS with a zero denominator and an unknown hemisphere letter.
        BrokenGps,
        /// A date that is not a calendar date.
        ImpossibleDate,
    }

    fn ascii(value: &str) -> Value {
        Value::Ascii(vec![format!("{value}\0").into_bytes()])
    }

    fn rationals(values: &[(u32, u32)]) -> Value {
        Value::Rational(
            values
                .iter()
                .map(|&(num, denom)| Rational { num, denom })
                .collect(),
        )
    }

    fn signed_rationals(values: &[(i32, i32)]) -> Value {
        Value::SRational(
            values
                .iter()
                .map(|&(num, denom)| SRational { num, denom })
                .collect(),
        )
    }

    /// A big-endian EXIF block with exactly the tags §7.4 reads: the dates in
    /// the Exif IFD, the SDMS pair plus hemisphere refs in the GPS IFD, and the
    /// two IFD pointers in IFD0.
    ///
    /// Layout: header, one data area for every value longer than the 4-byte
    /// entry slot (word-aligned, [EXIF23 4.6.1]), then IFD0, the Exif IFD and
    /// the GPS IFD.
    fn tiff(fixture: Fixture) -> Vec<u8> {
        let (original, digitized) = match fixture {
            Fixture::BothDates => (ascii(DATE), Some(ascii("2023:01:02 03:04:05"))),
            Fixture::ImpossibleDate => (ascii("2024:13:45 99:99:99"), None),
            _ => (ascii(DATE), None),
        };
        let mut exif_ifd = vec![entry(0x9000, Value::Byte(vec![2, 3, 0, 0]))];
        exif_ifd.push(entry(0x9003, original));
        exif_ifd.extend(digitized.map(|d| entry(0x9004, d)));

        let mut gps_ifd: Vec<Entry> = match fixture {
            Fixture::DateGps => vec![
                entry(0x0000, Value::Byte(vec![2, 3, 0, 0])),
                entry(0x0001, Value::Ascii(vec![b"N\0".to_vec()])),
                entry(0x0002, rationals(&LAT_DMS)),
                entry(0x0003, Value::Ascii(vec![b"E\0".to_vec()])),
                entry(0x0004, rationals(&LON_DMS)),
            ],
            Fixture::DateGpsSouthWest => vec![
                entry(0x0001, Value::Ascii(vec![b"S\0".to_vec()])),
                entry(0x0002, rationals(&LAT_DMS)),
                entry(0x0003, Value::Ascii(vec![b"W\0".to_vec()])),
                entry(0x0004, rationals(&LON_DMS)),
            ],
            Fixture::DateGpsSigned => vec![
                entry(0x0001, Value::Ascii(vec![b"N\0".to_vec()])),
                entry(0x0002, signed_rationals(&SIGNED_LAT_DMS)),
                entry(0x0003, Value::Ascii(vec![b"E\0".to_vec()])),
                entry(0x0004, signed_rationals(&SIGNED_LON_DMS)),
            ],
            // A zero denominator is NaN, and a letter that is neither S nor W
            // must not be read as a hemisphere.
            Fixture::BrokenGps => vec![
                entry(0x0001, Value::Ascii(vec![b"x\0".to_vec()])),
                entry(0x0002, rationals(&[(0, 0), (0, 1), (0, 1)])),
                entry(0x0003, Value::Ascii(vec![b"y\0".to_vec()])),
                entry(0x0004, rationals(&LON_DMS)),
            ],
            Fixture::Date | Fixture::BothDates | Fixture::ImpossibleDate => Vec::new(),
        };

        let mut ifd0 = vec![
            entry(0x0110, ascii("WolfsCam")),
            // 0x8769 ExifIFDPointer and 0x8825 GPSInfoIFDPointer; the offsets
            // are patched below, once the layout is known.
            entry(0x8769, Value::Long(vec![0])),
            entry(0x8825, Value::Long(vec![0])),
        ];

        let data_len = [ifd0.as_slice(), exif_ifd.as_slice(), gps_ifd.as_slice()]
            .into_iter()
            .flat_map(|ifd| ifd.iter().map(out_of_line_len))
            .sum::<usize>();
        let ifd0_at = 8 + data_len as u32;
        let exif_at = ifd0_at + ifd_len(&ifd0);
        let gps_at = exif_at + ifd_len(&exif_ifd);
        ifd0[1].value = Value::Long(vec![exif_at]);
        ifd0[2].value = Value::Long(vec![gps_at]);

        let mut out = Vec::new();
        out.extend_from_slice(b"MM\x00\x2a");
        out.extend_from_slice(&ifd0_at.to_be_bytes());
        // The data area follows the header, so the offsets it records are
        // already offsets from the start of the file.
        for ifd in [&mut ifd0, &mut exif_ifd, &mut gps_ifd] {
            write_data(&mut out, ifd);
        }
        for ifd in [ifd0, exif_ifd, gps_ifd] {
            write_ifd(&mut out, &ifd);
        }
        out
    }

    struct Entry {
        tag: u16,
        typ: u16,
        count: u32,
        value: Value,
    }

    /// The tag/type/count triple [EXIF23 4.6.1] asks for each value shape.
    fn entry(tag: u16, value: Value) -> Entry {
        let (typ, count) = match &value {
            Value::Byte(v) => (1, v.len()),
            Value::Ascii(v) => (2, v.first().map_or(0, Vec::len)),
            Value::Short(v) => (3, v.len()),
            Value::Long(v) => (4, v.len()),
            Value::Rational(v) => (5, v.len()),
            Value::Undefined(v, _) => (7, v.len()),
            Value::SRational(v) => (10, v.len()),
            other => panic!("the fixtures do not use {other:?}"),
        };
        Entry {
            tag,
            typ,
            count: count as u32,
            value,
        }
    }

    fn ifd_len(ifd: &[Entry]) -> u32 {
        2 + 12 * ifd.len() as u32 + 4
    }

    /// A value that does not fit the 4-byte slot lives in the data area; the
    /// others are stored inline.
    fn out_of_line_len(entry: &Entry) -> usize {
        let len = match &entry.value {
            Value::Ascii(v) => v.first().map_or(0, Vec::len),
            Value::Rational(v) => v.len() * 8,
            Value::SRational(v) => v.len() * 8,
            _ => 0,
        };
        if len > 4 { len + len % 2 } else { 0 }
    }

    /// Appends the out-of-line values of one IFD, word-aligning each, and
    /// replaces the entry value with its offset.
    fn write_data(data: &mut Vec<u8>, ifd: &mut [Entry]) {
        for entry in ifd.iter_mut() {
            let len = match &entry.value {
                Value::Ascii(v) => v.first().map_or(0, Vec::len),
                Value::Rational(v) => v.len() * 8,
                Value::SRational(v) => v.len() * 8,
                _ => 0,
            };
            if len <= 4 {
                continue;
            }
            while !data.len().is_multiple_of(2) {
                data.push(0);
            }
            let offset = data.len() as u32;
            match &entry.value {
                Value::Ascii(v) => data.extend_from_slice(&v[0]),
                Value::Rational(v) => {
                    for r in v {
                        data.extend_from_slice(&r.num.to_be_bytes());
                        data.extend_from_slice(&r.denom.to_be_bytes());
                    }
                }
                Value::SRational(v) => {
                    for r in v {
                        data.extend_from_slice(&r.num.to_be_bytes());
                        data.extend_from_slice(&r.denom.to_be_bytes());
                    }
                }
                other => panic!("{other:?} has no out-of-line payload"),
            }
            entry.value = Value::Long(vec![offset]);
        }
    }

    fn write_ifd(out: &mut Vec<u8>, ifd: &[Entry]) {
        out.extend_from_slice(&(ifd.len() as u16).to_be_bytes());
        for entry in ifd {
            out.extend_from_slice(&entry.tag.to_be_bytes());
            out.extend_from_slice(&entry.typ.to_be_bytes());
            out.extend_from_slice(&entry.count.to_be_bytes());
            out.extend_from_slice(&inline_slot(entry));
        }
        // No IFD1: the fixtures have no thumbnail.
        out.extend_from_slice(&0u32.to_be_bytes());
    }

    /// The 4-byte value/offset slot. Big-endian, so an inline value shorter than
    /// 4 bytes sits at the front of the slot.
    fn inline_slot(entry: &Entry) -> [u8; 4] {
        let mut slot = [0u8; 4];
        match &entry.value {
            Value::Byte(v) => slot[..v.len()].copy_from_slice(v),
            Value::Ascii(v) => slot[..v[0].len()].copy_from_slice(&v[0]),
            Value::Long(v) => slot.copy_from_slice(&v[0].to_be_bytes()),
            other => panic!("{other:?} does not fit the fixtures' inline slot"),
        }
        slot
    }

    // ---- the containers of §7.4 -------------------------------------------

    /// SOI + one APP1 `Exif\0\0` segment + EOI. No image data: §7.4 reads the
    /// header only, and a fixture that could be opened by a viewer would need a
    /// real picture.
    fn jpeg(exif: &[u8]) -> Vec<u8> {
        let mut out = vec![0xff, 0xd8, 0xff, 0xe1];
        out.extend_from_slice(&((exif.len() + 8) as u16).to_be_bytes());
        out.extend_from_slice(b"Exif\0\0");
        out.extend_from_slice(exif);
        out.extend_from_slice(&[0xff, 0xd9]);
        out
    }

    /// A PNG with an `eXIf` chunk [PNGEXT150 3.7]: signature, IHDR, the EXIF
    /// payload, IEND.
    fn png(exif: Option<&[u8]>) -> Vec<u8> {
        let mut out = b"\x89PNG\x0d\x0a\x1a\x0a".to_vec();
        // 1×1, 8-bit grayscale: the smallest header that is a valid PNG.
        chunk(&mut out, b"IHDR", &[0, 0, 0, 1, 8, 0, 0, 0, 1]);
        if let Some(exif) = exif {
            chunk(&mut out, b"eXIf", exif);
        }
        chunk(&mut out, b"IEND", &[]);
        out
    }

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        // The reader skips the CRC, but a fixture that is not a valid PNG is not
        // a fixture.
        let mut crc_input = kind.to_vec();
        crc_input.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }

    /// The CRC-32 (IEEE, reflected) a PNG chunk carries.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// A structurally valid HEIC carrying the EXIF block as its `Exif` item:
    /// `ftyp` + `meta`(`iinf`/`infe` declaring the item, `iloc` locating it in
    /// `idat` with `construction_method = 1`, and the `idat` payload itself).
    fn heic(exif: &[u8]) -> Vec<u8> {
        /// An arbitrary item id, as a camera would write.
        const ITEM_ID: u16 = 0x1e1d;

        let mut out = Vec::new();
        box_(&mut out, b"ftyp", |b| {
            b.extend_from_slice(b"heic");
            b.extend_from_slice(&0u32.to_be_bytes()); // minor_version
            b.extend_from_slice(b"mif1"); // the compatible brand the reader needs
            b.extend_from_slice(b"heic");
        });
        box_(&mut out, b"meta", |b| {
            b.extend_from_slice(&[0, 0, 0, 0]); // FullBox, version 0
            box_(b, b"iinf", |b| {
                b.extend_from_slice(&[0, 0, 0, 0]);
                b.extend_from_slice(&1u16.to_be_bytes()); // entry_count
                box_(b, b"infe", |b| {
                    b.extend_from_slice(&[2, 0, 0, 0]); // FullBox, version 2
                    b.extend_from_slice(&ITEM_ID.to_be_bytes());
                    b.extend_from_slice(&[0, 0]); // item_protection_index
                    b.extend_from_slice(b"Exif"); // item_type: what makes it the EXIF item
                    b.push(0); // item_name
                });
            });
            box_(b, b"iloc", |b| {
                b.extend_from_slice(&[1, 0, 0, 0]); // FullBox, version 1
                b.extend_from_slice(&0x4400u16.to_be_bytes()); // offset and length are 4 bytes
                b.extend_from_slice(&1u16.to_be_bytes()); // item_count
                b.extend_from_slice(&ITEM_ID.to_be_bytes());
                b.extend_from_slice(&[0, 1]); // construction_method 1 = inside idat
                b.extend_from_slice(&0u16.to_be_bytes()); // data_ref_index
                b.extend_from_slice(&1u16.to_be_bytes()); // extent_count
                // The extent starts at 0 because the item data *is* the 4-byte
                // Exif header offset followed by the TIFF block.
                b.extend_from_slice(&0u32.to_be_bytes());
                b.extend_from_slice(&((exif.len() + 4) as u32).to_be_bytes());
            });
            box_(b, b"idat", |b| {
                b.extend_from_slice(&0u32.to_be_bytes()); // offset to the TIFF header
                b.extend_from_slice(exif);
            });
        });
        out
    }

    /// Appends an ISO base media file box with its size patched to cover the
    /// header. The 64-bit `largesize` form is not needed: fixtures are small.
    fn box_(out: &mut Vec<u8>, kind: &[u8; 4], body: impl FnOnce(&mut Vec<u8>)) {
        let start = out.len();
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(kind);
        body(out);
        let size = (out.len() - start) as u32;
        out[start..start + 4].copy_from_slice(&size.to_be_bytes());
    }

    // ---- the tests --------------------------------------------------------

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn a_date_without_a_gps_ifd_is_still_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "so-data.jpg", &jpeg(&tiff(Fixture::Date)));
        let meta = extract(&path, None).unwrap();
        assert_eq!(meta.captured_at, Some(DATE_ISO.parse().unwrap()));
        assert_eq!(meta.gps, None);
        assert!(!meta.date_from_fs, "the date came from the file itself");
        assert!(meta.has_metadata(), "C7: a date of its own is enough");
    }

    /// The same position written as `SRATIONAL` must read the same: the value
    /// shape is a property of the camera, not of the coordinate.
    #[test]
    fn signed_rationals_read_like_unsigned_ones() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "signed.jpg",
            &jpeg(&tiff(Fixture::DateGpsSigned)),
        );
        let meta = extract(&path, None).unwrap();
        let (lat, lon) = meta.gps.expect("the signed fixture has a GPS IFD");
        assert_eq!(lat, LAT_DEG);
        assert_eq!(lon, LON_DEG);
    }

    /// The §7.4 contract for a photo: the date and the position of the file come
    /// back exactly as written. Every container of §7.4 must agree — that is the
    /// parity the gate asks for, and it is what makes "HEIC identical to source"
    /// a test instead of a hope.
    #[test]
    fn jpeg_png_and_heic_agree_with_the_written_values() {
        let dir = tempfile::tempdir().unwrap();
        let exif = tiff(Fixture::DateGps);
        let jpg = write(dir.path(), "photo.jpg", &jpeg(&exif));
        let png = write(dir.path(), "photo.png", &png(Some(&exif)));
        // Uppercase on purpose: an iPhone writes `.HEIC` and Windows keeps it.
        let heic = write(dir.path(), "photo.HEIC", &heic(&exif));

        let mut seen = Vec::new();
        for path in [&jpg, &png, &heic] {
            let meta = extract(path, None).unwrap();
            assert_eq!(
                meta.captured_at,
                Some(DATE_ISO.parse().unwrap()),
                "{} date",
                path.display()
            );
            let (lat, lon) = meta.gps.expect("the fixture has a GPS IFD");
            assert_eq!(lat, LAT_DEG, "{} latitude", path.display());
            assert_eq!(lon, LON_DEG, "{} longitude", path.display());
            assert!(!meta.date_from_fs, "{} read a real date", path.display());
            assert!(
                meta.has_metadata(),
                "{} is not sem metadados",
                path.display()
            );
            seen.push(meta);
        }
        assert_eq!(seen[0], seen[1], "png must read the same block as jpeg");
        assert_eq!(seen[0], seen[2], "heic must read the same block as jpeg");
    }

    #[test]
    fn southern_and_western_coordinates_are_negative() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "sul.jpg",
            &jpeg(&tiff(Fixture::DateGpsSouthWest)),
        );
        let meta = extract(&path, None).unwrap();
        let (lat, lon) = meta.gps.expect("the S/W fixture has a GPS IFD");
        assert_eq!(lat, -LAT_DEG, "GPSLatitudeRef = S flips the sign");
        assert_eq!(lon, -LON_DEG, "GPSLongitudeRef = W flips the sign");
    }

    #[test]
    fn a_broken_coordinate_is_dropped_not_guessed() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "ruim.jpg", &jpeg(&tiff(Fixture::BrokenGps)));
        let meta = extract(&path, None).unwrap();
        assert_eq!(meta.gps, None, "a zero denominator is not a position");
        // The date of the same file still survives: one bad field must not cost
        // the whole file.
        assert_eq!(meta.captured_at, Some(DATE_ISO.parse().unwrap()));
    }

    #[test]
    fn date_time_original_wins_over_create_date() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "duas.jpg", &jpeg(&tiff(Fixture::BothDates)));
        let meta = extract(&path, None).unwrap();
        assert_eq!(meta.captured_at, Some(DATE_ISO.parse().unwrap()));
        assert_ne!(
            meta.captured_at,
            Some("2023-01-02T03:04:05".parse().unwrap()),
            "the 0x9004 date must not be read while 0x9003 is there"
        );
    }

    #[test]
    fn an_impossible_date_is_no_date() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "ruim.jpg",
            &jpeg(&tiff(Fixture::ImpossibleDate)),
        );
        let meta = extract(&path, None).unwrap();
        assert_eq!(
            meta.captured_at,
            mtime(&path),
            "month 13 is not a date, so the mtime takes its place"
        );
        assert!(meta.date_from_fs, "and it is flagged");
        assert!(!meta.has_metadata(), "and C7 still calls it sem metadados");
    }

    /// The last resort, and its flag. A copied file carries the copy time, so
    /// §7.4/C7 treat an mtime date as *no* date.
    #[test]
    fn a_file_without_metadata_falls_back_to_a_flagged_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "sem-nada.png", &png(None));
        let meta = extract(&path, None).unwrap();

        let expected = mtime(&path).expect("the fixture was just written");
        assert_eq!(meta.captured_at, Some(expected));
        assert!(meta.date_from_fs, "mtime is always flagged");
        assert_eq!(meta.gps, None);
        assert!(
            !meta.has_metadata(),
            "C7: no date of its own and no GPS is sem metadados"
        );
    }

    /// C7 on the other side of the rule: a position alone is metadata, so a
    /// file with a GPS IFD and no date is *not* sem metadados. The date is
    /// cleared here instead of written by a fixture because no fixture can
    /// produce a GPS IFD with no date tags — and the rule under test is the
    /// `has_metadata` one.
    #[test]
    fn a_gps_fix_alone_still_counts_as_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "so-gps.jpg", &jpeg(&tiff(Fixture::DateGps)));
        let mut meta = extract(&path, None).unwrap();
        meta.captured_at = None;
        meta.date_from_fs = false;
        assert!(meta.has_metadata(), "C7 counts a position as metadata");
    }

    /// §7.4: a file that cannot be parsed loses its metadata, not the scan. The
    /// APP1 payload here is not a TIFF at all.
    #[test]
    fn a_broken_exif_block_degrades_to_the_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "quebrado.jpg", &jpeg(b"not a tiff block"));
        let meta = extract(&path, None).unwrap();
        assert_eq!(meta.captured_at, mtime(&path));
        assert!(meta.date_from_fs);
        assert!(!meta.has_metadata());
    }

    #[test]
    fn an_unreadable_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ausente.jpg");
        let err = extract(&path, None).unwrap_err();
        assert!(
            matches!(err, ExifError::Read { .. }),
            "a missing file is the walker's problem, not empty metadata"
        );
    }

    /// A video without ffmpeg still has a date — its mtime, flagged. This is the
    /// §16.2 "fresh tree" case: the app must scan before the binary is dropped.
    #[test]
    fn a_video_without_ffmpeg_falls_back_to_the_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "VID_0001.MOV", b"ftypqt  not a real movie");
        let meta = extract(&path, None).unwrap();
        assert_eq!(meta.captured_at, mtime(&path));
        assert!(meta.date_from_fs);
        assert!(!meta.has_metadata());
    }

    /// The other §16.2 case: the caller hands over a path where no binary is.
    /// That is E_FFMPEG upstream, not an empty date.
    #[test]
    fn a_missing_ffmpeg_binary_is_a_spawn_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "VID_0002.mp4", b"ftypqt  not a real movie");
        let ffmpeg = dir.path().join("bin").join("ffmpeg.exe");
        let err = extract(&path, Some(&ffmpeg)).unwrap_err();
        assert!(matches!(err, ExifError::Spawn(_)), "{err}");
    }

    /// A real `ffmpeg -f ffmetadata -` block, as ffprobe prints it for a MOV.
    #[test]
    fn creation_time_is_parsed_from_a_ffmetadata_block() {
        let block = ";FFMETADATA1\n\
                     major_brand=qt  \n\
                     minor_version=512\n\
                     compatible_brands=qt  \n\
                     creation_time=2024-03-15T22:33:44.000000Z\n\
                     encoder=Lavf60.16.100\n";
        let stamp = parse_creation_time(block).expect("a creation_time");
        // UTC-3: the 22:33:44 the `mvhd` clock holds is 19:33:44 on the wall.
        assert_eq!(stamp.local(sao_paulo), Nai::from_str("2024-03-15T19:33:44"));
    }

    #[test]
    fn an_escaped_value_is_never_read_as_a_date() {
        // `encoder` carries escapes, and an escaped `\=` must not look like the
        // start of a value.
        let block = ";FFMETADATA1\n\
                     encoder=Lavf60\\=16\\;100\n\
                     creation_time=2024-03-15T22:33:44Z\n";
        assert!(parse_creation_time(block).is_some());

        let escaped_only = ";FFMETADATA1\nencoder=Lavf60\\=16\n";
        assert_eq!(parse_creation_time(escaped_only), None);

        let broken_escape = ";FFMETADATA1\ncreation_time=2024-03-15T22:33:44\\\n";
        assert_eq!(parse_creation_time(broken_escape), None);
    }

    /// The zone a file states is kept and used as-is; `Z` and a missing zone mean
    /// "UTC, unresolved", so the machine's zone decides. The fraction is dropped.
    #[test]
    fn a_stated_zone_wins_and_the_fraction_is_dropped() {
        let parse = |stamp: &str| {
            parse_creation_time(&format!("creation_time={stamp}\n")).map(|c| c.local(sao_paulo))
        };
        // São Paulo, UTC-3 the year round since Brazil dropped DST in 2019.
        let expected = Some(Nai::from_str("2024-03-15T19:33:44"));
        // The file states its own wall clock, so the machine is not consulted
        // and the digits are read as they stand.
        let declared = parse_creation_time("creation_time=2024-03-15T22:33:44+02:00\n")
            .expect("a stamp with a zone")
            .local(|_| panic!("the machine must not decide a zone the file states"));
        assert_eq!(declared, Nai::from_str("2024-03-15T22:33:44"));

        assert_eq!(parse("2024-03-15T22:33:44Z"), expected, "ffprobe's form");
        assert_eq!(parse("2024-03-15T22:33:44"), expected, "no zone");
        assert_eq!(parse("2024-03-15T22:33:44.123456Z"), expected, "fraction");
        // A stated zone means the digits already *are* the wall clock, so they
        // survive untouched — at any offset, and without a machine decision.
        assert_eq!(
            parse("2024-03-15T02:33:44+02:00"),
            Some(Nai::from_str("2024-03-15T02:33:44")),
            "positive offset"
        );
        assert_eq!(
            parse("2024-03-15T01:33:44-03:00"),
            Some(Nai::from_str("2024-03-15T01:33:44")),
            "negative offset"
        );
        assert_eq!(
            parse("2024-03-15T22:33:44-0700"),
            expected,
            "an unparsable zone falls back to the machine"
        );

        let raw = |stamp: &str| parse_creation_time(&format!("creation_time={stamp}\n"));
        assert_eq!(raw("2024-03-15 22:33:44"), None, "a space is not a T");
        assert_eq!(raw(""), None);
    }

    /// A clip at 23:50 on the 31st belongs to the month it was shot in, not to
    /// the one its UTC stamp falls in — the reason the conversion exists.
    #[test]
    fn a_late_clip_keeps_its_own_month() {
        let stamp = parse_creation_time("creation_time=2024-03-31T23:50:00Z\n")
            .expect("a stamp")
            .local(sao_paulo);
        assert_eq!(stamp, Nai::from_str("2024-03-31T20:50:00"));
        assert_eq!(stamp.format("%Y/%m").to_string(), "2024/03");
    }

    fn sao_paulo(_utc: &NaiveDateTime) -> FixedOffset {
        FixedOffset::west_opt(3 * 3600).expect("a whole-hour zone")
    }

    /// Shorthand for the expected wall clock: the fixtures are all `DATE`.
    struct Nai;

    impl Nai {
        fn from_str(value: &str) -> NaiveDateTime {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S").expect("a naive stamp")
        }
    }
}
