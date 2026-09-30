//! Offline reverse geocoding over `Database/geonames.bin` (PLAN §7.5).
//!
//! The app never geocodes over the network. A maintainer runs
//! `tools/gen-geonames.rs` once, offline, and the result is a flat binary index
//! that this module reads. The point of the index is that a scan can ask "which
//! city was this photo taken in" for tens of thousands of files without a
//! network call, a SQL query, or a fuzzy name match: it is a grid lookup.
//!
//! ## The file
//!
//! ```text
//! header    32 B    magic, version, reserved, min_pop, counts, string-table length
//! cells      8 B x N  key u16, count u16, first_record u32
//! records   24 B x N  lat i32, lon i32 (deg x 1e7), pop u32, 3 string offsets
//! strings    ? B    UTF-8, NUL-terminated, referenced by byte offset from its start
//! ```
//!
//! The grid is 1°×1°: 180 latitude bands × 360 longitude bands = 64800 cells,
//! which is exactly why a cell key is a `u16` — the §7.5 `HashMap<u16, Range<u32>>`
//! is not an approximation, it is the whole grid. The reader builds that table
//! and never decodes a record it does not need.
//!
//! ## Decisions taken in c7 (recorded in PLAN §7.5.1)
//!
//! 1. **A cell keeps every city inside it, ordered by population
//!    descending.** §7.5 asks for a nearest-by-Haversine winner *and* a
//!    population tie-break, and a single "best" row per cell cannot be compared
//!    against anything: a tie-break needs the runners-up. The build-time order
//!    is not a query-time shortcut (the query reads all 9 cells) — it is what
//!    makes the file byte-reproducible and the tie-break deterministic when two
//!    cities sit the same distance apart.
//! 2. **The search is the query's cell plus the 8 around it, and a candidate
//!    further than 40 km is refused.** Both halves of the §7.5 "0 → ~40 km" are
//!    kept. The ring bound is not a guess: a ring-2 cell is at least a full
//!    degree of latitude away (~111 km), so rings 0 and 1 provably cover
//!    everything the cutoff can accept, and the cutoff can never hide a city the
//!    expansion would have reached. Both facts are pinned by tests.
//! 3. **`GeoDto` carries the *photo's* coordinates, not the city's.** The
//!    `locations` row is keyed by `media_id` (§6.1), so its lat/lon is the
//!    position of the media. A city centroid there would silently move a photo
//!    taken 20 km out into downtown, and a centroid is a property of the city,
//!    not of the file. The index keeps its own coordinates for the distance
//!    computation; the name is what travels.
//! 4. **A missing or unreadable index is `None`, never an error.** §7.5 says the
//!    app never crashes on geo absence, so [`Geocoder::geocode`] has no error
//!    arm at all: no index → no city, the photo keeps its lat/lon, and the
//!    Settings alert of §7.5 is the only place that reports the absence. The
//!    fallible [`GeoIndex::load`] exists for the tool and the tests, which need
//!    to tell "no file" from "broken file".
//! 5. **The reader is mmap-free and loads once into a `Vec<u8>`.** The file is
//!    ~10 MB at a 500-population threshold (§7.5 budgets 16 MB), the resident
//!    cost is the file plus one `u16 → Range<u32>` entry per populated cell,
//!    and a 50k-photo scan is then a bounded hash lookup per photo with no
//!    allocation on the query path.
//!
//! The producer is `tools/gen-geonames.rs`, behind the non-default `tooling`
//! feature, so a normal `cargo build` never compiles any of it into the app.
//! The writer lives *here*, next to the reader, on purpose: the tests build
//! their synthetic index through the real format code, so a change that broke
//! one side of the format breaks the other side of a test instead of passing a
//! hand-rolled blob.

use std::collections::HashMap;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use crate::core::models::GeoDto;

// The writer is an inline module rather than a file: §7.5's module map lists
// `geocode.rs` and nothing under it, and `exif.rs` sets the precedent for a
// slice that owns both sides of its own format.
#[cfg(any(feature = "tooling", test))]
pub use writer::{City, CityWriter, WriteReport};

/// 8-byte magic. A GeoNames dump renamed to `.bin` is rejected with a clear
/// message instead of being parsed as garbage.
pub const MAGIC: [u8; 8] = *b"WOLFSGEO";
/// The only version this build writes and the only one it reads.
pub const VERSION: u16 = 1;

const HEADER_LEN: usize = 32;
const CELL_ENTRY_LEN: usize = 8;
const RECORD_LEN: usize = 24;

const LAT_BANDS: i32 = 180;
const LON_BANDS: i32 = 360;
/// 180 × 360 = 64800, which is what makes the `u16` cell key of §7.5 work.
pub const CELL_COUNT: u32 = (LAT_BANDS * LON_BANDS) as u32;

/// Ring 0 is the query's own cell; ring 1 is the 8 around it.
const MAX_RING: i32 = 1;
/// §7.5's "0 → ~40 km", enforced as a real distance cutoff and not only as a
/// search bound.
pub const MAX_DIST_KM: f64 = 40.0;
/// One degree of latitude. Not used at runtime: it is what the test proving
/// ring 2 cannot hold a city within [`MAX_DIST_KM`] measures against, so the
/// bound is a stated fact rather than a claim in a comment.
#[cfg(test)]
const DEG_LAT_KM: f64 = 111.195;
/// Two distances closer than this are "the same distance" for a tie-break.
const DIST_EPS_KM: f64 = 1e-6;
/// Mean Earth radius (IUGG), km.
const EARTH_RADIUS_KM: f64 = 6371.0088;

/// §7.5's cache budget.
const CACHE_CAPACITY: usize = 512;

/// Coordinates live as degrees × 1e7 in an `i32`: ~1 cm, far finer than a 1°
/// grid needs, and it keeps the comparison exact instead of dragging `f32`
/// rounding into the distance computation.
const COORD_SCALE: f64 = 1e7;

/// §7.5's 16 MB budget for the whole index. Public because it is a fact about
/// the *format*, not about the reader: the tool checks a build against it and
/// the reader does not care.
pub const BYTE_BUDGET: usize = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum GeoError {
    #[error("could not read the cities index: {0}")]
    Io(#[from] std::io::Error),
    #[error("this is not a WolfsMedia cities index (bad magic)")]
    BadMagic,
    #[error("cities index version {found} is not supported (this build reads {VERSION})")]
    BadVersion { found: u16 },
    #[error("cities index is truncated or inconsistent: {0}")]
    Truncated(&'static str),
}

/// The loaded index: the file bytes plus the cell → record-range table.
///
/// Immutable once built, so it is shared behind an [`Arc`] and every scan
/// thread can query it at once.
pub struct GeoIndex {
    data: Vec<u8>,
    cells: HashMap<u16, Range<u32>>,
    records_start: usize,
    strings_start: usize,
    strings_len: usize,
    record_count: u32,
    min_pop: u32,
}

impl GeoIndex {
    /// Reads and validates the index at `path`. The fallible door, for the tool
    /// and the tests — the app calls [`Geocoder`].
    pub fn load(path: &Path) -> Result<Self, GeoError> {
        Self::from_bytes(fs::read(path)?)
    }

    /// Parses an index that is already in memory.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, GeoError> {
        // Magic first, length second: a short file of the wrong kind is a
        // clearer "this is not a cities index" than a bare "truncated", and the
        // 8 magic bytes are readable as soon as there are 8 bytes at all.
        if data.len() < MAGIC.len() {
            return Err(GeoError::Truncated("shorter than the magic"));
        }
        if data[0..8] != MAGIC {
            return Err(GeoError::BadMagic);
        }
        if data.len() < HEADER_LEN {
            return Err(GeoError::Truncated("shorter than the header"));
        }
        let version = u16::from_le_bytes([data[8], data[9]]);
        if version != VERSION {
            return Err(GeoError::BadVersion { found: version });
        }
        let read_u32 =
            |at: usize| u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
        let min_pop = read_u32(12);
        let cell_count = read_u32(16);
        let record_count = read_u32(20);
        let strings_len = read_u32(24) as usize;

        // Every region is bounds-checked *before* it is indexed, with checked
        // arithmetic, so a truncated or hostile file cannot wrap a length
        // computation into a panic or a wild read.
        if cell_count > CELL_COUNT {
            return Err(GeoError::Truncated("more cells than the grid has"));
        }
        let Some(cells_len) = (cell_count as u64).checked_mul(CELL_ENTRY_LEN as u64) else {
            return Err(GeoError::Truncated("cell region overflows"));
        };
        let Some(records_len) = (record_count as u64).checked_mul(RECORD_LEN as u64) else {
            return Err(GeoError::Truncated("record region overflows"));
        };
        let Some(records_start) = HEADER_LEN.checked_add(cells_len as usize) else {
            return Err(GeoError::Truncated("cell region overflows"));
        };
        let Some(strings_start) = records_start.checked_add(records_len as usize) else {
            return Err(GeoError::Truncated("record region overflows"));
        };
        let Some(strings_end) = strings_start.checked_add(strings_len) else {
            return Err(GeoError::Truncated("string table overflows"));
        };
        if strings_end > data.len() {
            return Err(GeoError::Truncated("declared past the end of the file"));
        }

        let mut cells = HashMap::with_capacity(cell_count as usize);
        for cell in 0..cell_count as usize {
            let at = HEADER_LEN + cell * CELL_ENTRY_LEN;
            let key = u16::from_le_bytes([data[at], data[at + 1]]);
            let count = u16::from_le_bytes([data[at + 2], data[at + 3]]) as u32;
            let first = read_u32(at + 4);
            // A cell pointing outside the record array would make every query
            // in it decode foreign bytes as cities.
            if first
                .checked_add(count)
                .is_none_or(|end| end > record_count)
            {
                return Err(GeoError::Truncated("a cell points past the records"));
            }
            cells.insert(key, first..first + count);
        }

        Ok(Self {
            data,
            cells,
            records_start,
            strings_start,
            strings_len,
            record_count,
            min_pop,
        })
    }

    /// The nearest city to `(lat, lon)` within [`MAX_DIST_KM`], or `None` when
    /// the index is empty or nothing is close enough. Decision 4: a failure is
    /// not representable, so a caller cannot forget to handle one.
    pub fn geocode(&self, lat: f64, lon: f64) -> Option<GeoDto> {
        let (lat_band, lon_band) = bands(lat, lon)?;

        let mut best: Option<Candidate> = None;
        for ring in 0..=MAX_RING {
            for dlat in -ring..=ring {
                for dlon in -ring..=ring {
                    // Ring 0 is the cell itself; every later ring is only its
                    // perimeter, so no cell is evaluated twice.
                    if ring > 0 && dlat.abs() != ring && dlon.abs() != ring {
                        continue;
                    }
                    let la = lat_band + dlat;
                    let lo = lon_band + dlon;
                    if !(0..LAT_BANDS).contains(&la) || !(0..LON_BANDS).contains(&lo) {
                        continue;
                    }
                    let Some(range) = self.cells.get(&cell_key(la, lo)) else {
                        continue;
                    };
                    for index in range.clone() {
                        let Some(city) = self.record(index) else {
                            continue;
                        };
                        let distance = haversine_km(lat, lon, city.lat, city.lon);
                        if distance > MAX_DIST_KM {
                            continue;
                        }
                        let candidate = Candidate {
                            index,
                            distance,
                            pop: city.pop,
                        };
                        // The winner is a plain comparison, not
                        // `best.map_or(true, ..)`: the closure would move
                        // `best` while it is still being read.
                        let wins = match &best {
                            None => true,
                            Some(current) => candidate.better_than(current),
                        };
                        if wins {
                            best = Some(candidate);
                        }
                    }
                }
            }
        }

        // Decision 3: the coordinates in the DTO are the queried ones.
        best.map(|winner| {
            let city = self
                .record(winner.index)
                .expect("the winner was decoded already");
            GeoDto {
                city: self.string(city.name).to_owned(),
                state: self.string(city.state).to_owned(),
                country: self.string(city.country).to_owned(),
                latitude: lat,
                longitude: lon,
            }
        })
    }

    /// Populated cells — the memory the §7.5 offset table costs.
    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }

    pub fn record_count(&self) -> u32 {
        self.record_count
    }

    pub fn min_pop(&self) -> u32 {
        self.min_pop
    }

    /// True when the index parsed but holds nothing to look up: the "the data is
    /// here but it is empty" case, which §7.5's Settings alert shows differently
    /// from a missing file.
    pub fn is_empty(&self) -> bool {
        self.record_count == 0
    }

    /// Resident bytes: the file plus one hash entry per populated cell. The
    /// §7.5 budget covers this, not the file alone.
    pub fn resident_bytes(&self) -> usize {
        self.data.len() + self.cells.len() * std::mem::size_of::<(u16, Range<u32>)>() + 64
    }

    fn record(&self, index: u32) -> Option<Record> {
        let at = self
            .records_start
            .checked_add(index as usize * RECORD_LEN)?;
        let bytes = self.data.get(at..at.checked_add(RECORD_LEN)?)?;
        let read =
            |o: usize| i32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        let read_u =
            |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        Some(Record {
            lat: read(0) as f64 / COORD_SCALE,
            lon: read(4) as f64 / COORD_SCALE,
            pop: read_u(8),
            name: read_u(12),
            state: read_u(16),
            country: read_u(20),
        })
    }

    /// A NUL-terminated string at `offset`, counted from the start of the
    /// string table. Offset 0 is always the empty string (the writer guarantees
    /// it), and a malformed offset reads as empty instead of panicking — this is
    /// the one place a corrupt file could otherwise become a crash.
    fn string(&self, offset: u32) -> &str {
        let end = self.strings_start + self.strings_len;
        let Some(at) = self.strings_start.checked_add(offset as usize) else {
            return "";
        };
        if at >= end {
            return "";
        }
        let rest = &self.data[at..end];
        let stop = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        std::str::from_utf8(&rest[..stop]).unwrap_or("")
    }
}

struct Record {
    lat: f64,
    lon: f64,
    pop: u32,
    name: u32,
    state: u32,
    country: u32,
}

/// A winner in progress. `better_than` is the whole §7.5 tie-break in one
/// place: closer wins, and at the same distance the bigger city wins.
struct Candidate {
    index: u32,
    distance: f64,
    pop: u32,
}

impl Candidate {
    fn better_than(&self, other: &Self) -> bool {
        if self.distance < other.distance - DIST_EPS_KM {
            return true;
        }
        (self.distance - other.distance).abs() <= DIST_EPS_KM && self.pop > other.pop
    }
}

/// Great-circle distance in km on a sphere. Haversine, not a geodesic solver:
/// the answer feeds a nearest-city pick over a 40 km budget, where a spherical
/// model's error is metres and the solver's cost is not.
fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dlon / 2.0).sin().powi(2);
    // `asin` is undefined outside [0,1] and float noise can push it there.
    2.0 * EARTH_RADIUS_KM * a.clamp(0.0, 1.0).sqrt().asin()
}

/// Latitude and longitude to their 1° band indices, or `None` for a coordinate
/// that is not a place on Earth. Rejects NaN, ±inf, a latitude past the pole and
/// a longitude that wrapped, so a corrupt EXIF fix is a `None` rather than a
/// panic or a cell that silently wrapped to the other side of the world.
fn bands(lat: f64, lon: f64) -> Option<(i32, i32)> {
    if !lat.is_finite() || !lon.is_finite() || !(-90.0..=90.0).contains(&lat) {
        return None;
    }
    if !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    // `+ 90.0` lands exactly on a band edge at the poles, hence the clamp.
    let lat_band = ((lat + 90.0).floor() as i32).clamp(0, LAT_BANDS - 1);
    let lon_band = ((lon + 180.0).floor() as i32).clamp(0, LON_BANDS - 1);
    Some((lat_band, lon_band))
}

/// The §7.5 `u16` cell key. 64800 cells fit in 16 bits, which is the whole
/// reason this grid is 1°×1°.
fn cell_key(lat_band: i32, lon_band: i32) -> u16 {
    (lat_band * LON_BANDS + lon_band) as u16
}

/// The thread-safe handle the app holds: the index, loaded once and shared,
/// plus the §7.5 LRU.
pub struct Geocoder {
    path: PathBuf,
    /// `None` means "no usable index". A *failed* load is remembered, so a
    /// missing file is not re-stat'ed once per photo.
    index: OnceLock<Option<Arc<GeoIndex>>>,
    cache: Mutex<Lru>,
}

impl Geocoder {
    /// A geocoder over `<root>/Database/geonames.bin` (§5.3). Nothing is read
    /// yet — see [`Geocoder::preload`].
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            index: OnceLock::new(),
            cache: Mutex::new(Lru::new(CACHE_CAPACITY)),
        }
    }

    /// Reads the index now, off any hot path, so the first photo of a scan does
    /// not pay for it. Idempotent, cheap to call more than once, and safe to run
    /// on a worker thread while others already query.
    pub fn preload(&self) {
        let _ = self.get();
    }

    /// Whether a usable index is behind this handle. §7.5's Settings alert and
    /// §8.1's `geo_lookup` both need to tell "loaded" from "absent" without
    /// re-reading the file.
    ///
    /// This **triggers** the load if it has not run, so it answers honestly
    /// even on the first call; the point of [`Geocoder::preload`] is to move
    /// that cost off whatever asks first, not to make this a peek.
    pub fn available(&self) -> bool {
        self.get().is_some()
    }

    /// Nearest city, or `None`. No error arm, by decision 4.
    pub fn geocode(&self, lat: f64, lon: f64) -> Option<GeoDto> {
        // An unusable coordinate can never be cached usefully, and its key
        // would be meaningless anyway.
        let key = CacheKey::new(lat, lon)?;
        if let Some(hit) = self.lock_cache().get(&key) {
            return Some(hit);
        }
        let found = self.get()?.geocode(lat, lon)?;
        self.lock_cache().put(key, found.clone());
        Some(found)
    }

    fn get(&self) -> Option<Arc<GeoIndex>> {
        self.index
            .get_or_init(|| match GeoIndex::load(&self.path) {
                Ok(index) => Some(Arc::new(index)),
                // Remembered as absent on purpose: without this, a tree with no
                // cities would re-read the file for every photo of every scan.
                Err(error) => {
                    eprintln!("[wolfsmedia] cities index unavailable: {error}");
                    None
                }
            })
            .clone()
    }

    /// Never poisoned in practice: a panic while holding the cache would only
    /// lose cached answers, and the alternative is a `Result` on every query.
    fn lock_cache(&self) -> MutexGuard<'_, Lru> {
        self.cache.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Cache key: the queried point quantized exactly like a stored record, so the
/// key is reproducible and two queries for the same place share an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CacheKey {
    lat: i32,
    lon: i32,
}

impl CacheKey {
    fn new(lat: f64, lon: f64) -> Option<Self> {
        bands(lat, lon)?;
        Some(Self {
            lat: (lat * COORD_SCALE) as i32,
            lon: (lon * COORD_SCALE) as i32,
        })
    }
}

/// The §7.5 LRU, 512 entries.
///
/// A `HashMap` of value + use-counter, and the victim is the lowest counter.
/// O(1) insert, O(n) eviction with n = 512 and one eviction per *new* point.
/// `geocode` runs once per photo during a scan, so this is a rounding error
/// next to the distance computation it serves — and it needs no dependency and
/// no clock.
struct Lru {
    entries: HashMap<CacheKey, (GeoDto, u64)>,
    tick: u64,
    capacity: usize,
}

impl Lru {
    fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::with_capacity(capacity),
            tick: 0,
            capacity,
        }
    }

    fn get(&mut self, key: &CacheKey) -> Option<GeoDto> {
        // `tick` starts at 0 and the counter is bumped *before* the read, so 0
        // is never a live counter and a wrapped tick cannot evict the entry
        // that was just used.
        let tick = self.tick;
        self.tick = self.tick.wrapping_add(1);
        let (value, previous) = self.entries.get_mut(key)?;
        *previous = tick;
        Some(value.clone())
    }

    fn put(&mut self, key: CacheKey, value: GeoDto) {
        let tick = self.tick;
        self.tick = self.tick.wrapping_add(1);
        if self.entries.len() >= self.capacity
            && let Some(victim) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(key, _)| *key)
        {
            self.entries.remove(&victim);
        }
        self.entries.insert(key, (value, tick));
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// The *producer* side of `geonames.bin`.
///
/// Gated behind the non-default `tooling` feature (or `test`), so a normal
/// `cargo build` of the app never compiles it. The `gen-geonames` binary is the
/// only shipped user.
#[cfg(any(feature = "tooling", test))]
pub mod writer {
    use super::{
        BYTE_BUDGET, CELL_COUNT, CELL_ENTRY_LEN, COORD_SCALE, HEADER_LEN, LAT_BANDS, LON_BANDS,
        MAGIC, RECORD_LEN, VERSION, bands, cell_key,
    };
    use std::collections::HashMap;

    /// One city, as the GeoNames dump names it. `PartialEq` and not `Eq`:
    /// `lat`/`lon` are `f64`, and a test that compared cities for equality does
    /// not need the reflexive guarantee of a total order.
    #[derive(Debug, Clone, PartialEq)]
    pub struct City {
        pub lat: f64,
        pub lon: f64,
        pub population: u32,
        pub name: String,
        pub state: String,
        pub country: String,
    }

    impl City {
        pub fn new(
            lat: f64,
            lon: f64,
            population: u32,
            name: &str,
            state: &str,
            country: &str,
        ) -> Self {
            Self {
                lat,
                lon,
                population,
                name: name.to_owned(),
                state: state.to_owned(),
                country: country.to_owned(),
            }
        }
    }

    /// What a build produced, for the tool's report and the budget test.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct WriteReport {
        pub cities: u32,
        pub cells: u32,
        pub bytes: usize,
        pub min_pop: u32,
    }

    impl WriteReport {
        /// §7.5 budgets the index at 16 MB.
        pub fn within_budget(&self) -> bool {
            self.bytes <= BYTE_BUDGET
        }

        /// What the app will actually hold resident: the file plus one hash
        /// entry per populated cell. The report says this in KB, because the
        /// 16 MB is the budget the §7.5 table is really about.
        pub fn resident_kb(&self) -> usize {
            let table = self.cells as usize * std::mem::size_of::<(u16, std::ops::Range<u32>)>();
            (self.bytes + table + 64).div_ceil(1024)
        }
    }

    /// Accumulates cities and serializes them into the index.
    ///
    /// The order is fixed so the same input always produces the same bytes: by
    /// cell, then population descending (the §7.5.1 decision 1 ordering, and
    /// what makes the tie-break deterministic), then by name and state so two
    /// equally populous cities in one cell do not swap between builds.
    pub struct CityWriter {
        min_pop: u32,
        cities: Vec<City>,
        /// The same city arriving twice is one city: a GeoNames dump is
        /// country-scoped and overlapping extracts are a normal way to build one.
        seen: HashMap<(i32, i32, String), ()>,
    }

    impl CityWriter {
        pub fn new(min_pop: u32) -> Self {
            Self {
                min_pop,
                cities: Vec::new(),
                seen: HashMap::new(),
            }
        }

        /// Adds a city. A city below the population floor, without a name, or
        /// with coordinates that are not a place on Earth is dropped:
        /// `GeoDto.city` is what the UI shows, and an unindexable row would
        /// never be found anyway.
        pub fn push(&mut self, city: City) {
            if city.population < self.min_pop || city.name.trim().is_empty() {
                return;
            }
            if bands(city.lat, city.lon).is_none() {
                return;
            }
            let key = (
                (city.lat * COORD_SCALE) as i32,
                (city.lon * COORD_SCALE) as i32,
                city.name.clone(),
            );
            if self.seen.insert(key, ()).is_none() {
                self.cities.push(city);
            }
        }

        pub fn len(&self) -> usize {
            self.cities.len()
        }

        pub fn is_empty(&self) -> bool {
            self.cities.is_empty()
        }

        pub fn min_pop(&self) -> u32 {
            self.min_pop
        }

        /// Serializes the index to bytes. Every caller wants the bytes: the tool
        /// writes them to the SSD, the tests parse them in place.
        pub fn finish(&self) -> Result<Vec<u8>, super::GeoError> {
            let mut order: Vec<&City> = self.cities.iter().collect();
            order.sort_by(|a, b| {
                cell_of(a.lat, a.lon)
                    .cmp(&cell_of(b.lat, b.lon))
                    // Descending population: the biggest city in a cell is the
                    // one a tie-break must keep.
                    .then(b.population.cmp(&a.population))
                    .then(a.name.cmp(&b.name))
                    .then(a.state.cmp(&b.state))
            });

            // Group into cells so each writes exactly one directory entry.
            let mut cells: Vec<(u16, std::ops::Range<usize>)> = Vec::new();
            for (position, city) in order.iter().enumerate() {
                let key = cell_of(city.lat, city.lon);
                match cells.last_mut() {
                    Some((last, range)) if *last == key => range.end = position + 1,
                    _ => cells.push((key, position..position + 1)),
                }
            }

            // The string table, one copy per distinct string. A country code
            // repeats once per city, so this is where the file gets its size.
            let mut strings: Vec<u8> = vec![0];
            let mut offsets: HashMap<String, u32> = HashMap::new();

            let record_count = order.len() as u32;
            let cell_count = cells.len() as u32;
            debug_assert!(cell_count <= CELL_COUNT, "more cells than the grid has");
            let records_start = HEADER_LEN + cells.len() * CELL_ENTRY_LEN;
            let strings_start = records_start + record_count as usize * RECORD_LEN;
            let strings_len_at = 24;

            let mut out = Vec::with_capacity(strings_start + 64);
            out.extend_from_slice(&MAGIC);
            out.extend_from_slice(&VERSION.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // reserved
            out.extend_from_slice(&self.min_pop.to_le_bytes());
            out.extend_from_slice(&cell_count.to_le_bytes());
            out.extend_from_slice(&record_count.to_le_bytes());
            // Patched once the table is complete; the placeholder keeps the
            // header the right width while the body is written.
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes()); // reserved
            debug_assert_eq!(out.len(), HEADER_LEN);

            for (key, range) in &cells {
                out.extend_from_slice(&key.to_le_bytes());
                out.extend_from_slice(&(range.len() as u16).to_le_bytes());
                out.extend_from_slice(&(range.start as u32).to_le_bytes());
            }
            debug_assert_eq!(out.len(), records_start);

            for city in &order {
                out.extend_from_slice(&quantize(city.lat).to_le_bytes());
                out.extend_from_slice(&quantize(city.lon).to_le_bytes());
                out.extend_from_slice(&city.population.to_le_bytes());
                out.extend_from_slice(
                    &intern(&city.name, &mut strings, &mut offsets).to_le_bytes(),
                );
                out.extend_from_slice(
                    &intern(&city.state, &mut strings, &mut offsets).to_le_bytes(),
                );
                out.extend_from_slice(
                    &intern(&city.country, &mut strings, &mut offsets).to_le_bytes(),
                );
            }
            debug_assert_eq!(out.len(), strings_start);

            out[strings_len_at..strings_len_at + 4]
                .copy_from_slice(&(strings.len() as u32).to_le_bytes());
            out.extend_from_slice(&strings);

            Ok(out)
        }

        /// What the previous [`CityWriter::finish`] produced, for the tool.
        pub fn report(&self, bytes: usize) -> WriteReport {
            WriteReport {
                cities: self.cities.len() as u32,
                cells: self.populated_cells(),
                bytes,
                min_pop: self.min_pop,
            }
        }

        fn populated_cells(&self) -> u32 {
            let mut keys: HashMap<u16, ()> = HashMap::new();
            for city in &self.cities {
                keys.insert(cell_of(city.lat, city.lon), ());
            }
            keys.len() as u32
        }
    }

    fn intern(value: &str, strings: &mut Vec<u8>, offsets: &mut HashMap<String, u32>) -> u32 {
        if let Some(offset) = offsets.get(value) {
            return *offset;
        }
        let offset = strings.len() as u32;
        strings.extend_from_slice(value.as_bytes());
        strings.push(0);
        offsets.insert(value.to_owned(), offset);
        offset
    }

    /// Degrees to fixed-point `i32`. The caller has already rejected a
    /// coordinate outside the world, so the clamp is a belt-and-braces guard
    /// against a `f64` that rounds past `i32`.
    fn quantize(degrees: f64) -> i32 {
        (degrees * COORD_SCALE)
            .round()
            .clamp(i32::MIN as f64, i32::MAX as f64) as i32
    }

    /// The 1° grid cell, as a `u16` key. The writer derives it before the
    /// reader exists; the reader re-derives it on every query, so the two must
    /// agree or every city becomes unfindable — pinned by a test.
    fn cell_of(lat: f64, lon: f64) -> u16 {
        match bands(lat, lon) {
            Some((la, lo)) => cell_key(la, lo),
            // `push` refuses these, so nothing reaches here.
            None => 0,
        }
    }

    /// The grid, re-exported for the tool's report.
    pub const GRID: (i32, i32) = (LAT_BANDS, LON_BANDS);

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::core::geocode::{DEG_LAT_KM, GeoIndex, MAX_DIST_KM, cell_key};

        fn writer_with(cities: &[(&str, f64, f64, u32)]) -> CityWriter {
            let mut writer = CityWriter::new(500);
            for (name, lat, lon, pop) in cities {
                writer.push(City::new(*lat, *lon, *pop, name, "SP", "BR"));
            }
            writer
        }

        #[test]
        fn writes_an_index_the_reader_understands() {
            let mut writer = writer_with(&[("São Paulo", -23.5505, -46.6333, 12_252_023)]);
            writer.push(City::new(
                -23.1829, -47.1744, 90_664, "Campinas", "SP", "BR",
            ));

            let index = GeoIndex::from_bytes(writer.finish().unwrap()).unwrap();
            assert_eq!(index.record_count(), 2);
            assert_eq!(index.min_pop(), 500);
            assert_eq!(index.geocode(-23.5505, -46.6333).unwrap().city, "São Paulo");
            assert_eq!(index.geocode(-23.1829, -47.1744).unwrap().city, "Campinas");
        }

        #[test]
        fn an_empty_dataset_is_still_a_valid_file() {
            let index = GeoIndex::from_bytes(CityWriter::new(500).finish().unwrap()).unwrap();
            assert!(index.is_empty());
            assert_eq!(index.cell_count(), 0);
            assert_eq!(index.record_count(), 0);
        }

        /// The population floor of §7.5, applied at build time so the query path
        /// never filters.
        #[test]
        fn drops_cities_below_the_floor() {
            let writer = writer_with(&[
                ("Grande", -23.0, -46.0, 100_000),
                ("Pequena", -24.0, -47.0, 499),
                ("No Limite", -25.0, -48.0, 500),
            ]);
            assert_eq!(writer.len(), 2, "499 is below, 500 is on the floor");

            let index = GeoIndex::from_bytes(writer.finish().unwrap()).unwrap();
            assert_eq!(index.record_count(), 2);
            assert_eq!(index.geocode(-25.0, -48.0).unwrap().city, "No Limite");
        }

        #[test]
        fn drops_a_city_with_no_name() {
            let writer = writer_with(&[
                ("", -23.0, -46.0, 100_000),
                ("Com Nome", -24.0, -47.0, 1_000),
            ]);
            assert_eq!(writer.len(), 1);
        }

        /// A coordinate that is not a place on Earth cannot be indexed, so it
        /// is dropped instead of being written to a cell the query can never
        /// reach.
        #[test]
        fn drops_coordinates_that_are_not_places() {
            let writer = writer_with(&[
                ("No Limite", 91.0, 0.0, 1_000),
                ("Longitude", 0.0, 181.0, 1_000),
                ("Valida", 0.0, 0.0, 1_000),
            ]);
            assert_eq!(writer.len(), 1);
        }

        #[test]
        fn the_same_city_twice_is_one_city() {
            let mut writer = CityWriter::new(500);
            for _ in 0..3 {
                writer.push(City::new(
                    -23.5505,
                    -46.6333,
                    12_252_023,
                    "São Paulo",
                    "SP",
                    "BR",
                ));
            }
            assert_eq!(writer.len(), 1);
        }

        /// Two cities may share a name in a state, so the dedupe key includes
        /// the coordinates.
        #[test]
        fn same_name_different_place_is_two_cities() {
            let mut writer = CityWriter::new(500);
            writer.push(City::new(-23.0, -46.0, 1_000, "Centro", "SP", "BR"));
            writer.push(City::new(-24.0, -47.0, 1_000, "Centro", "SP", "BR"));
            assert_eq!(writer.len(), 2);
        }

        /// The same input must give the same bytes, or two builds of the same
        /// dataset are two files and there is no way to tell a rebuild from a
        /// change.
        #[test]
        fn output_is_reproducible_regardless_of_input_order() {
            let a = writer_with(&[
                ("Alpha", -23.0, -46.0, 900),
                ("Beta", -24.0, -47.0, 5_000),
                ("Gamma", -25.0, -48.0, 50),
            ])
            .finish()
            .unwrap();
            let b = writer_with(&[
                ("Gamma", -25.0, -48.0, 50),
                ("Beta", -24.0, -47.0, 5_000),
                ("Alpha", -23.0, -46.0, 900),
            ])
            .finish()
            .unwrap();
            assert_eq!(a, b);
        }

        /// Decision 1: population descending inside a cell, and *all* of them
        /// are kept — otherwise a tie-break has nothing to choose between. The
        /// three share a cell and sit ~11 km apart, so each is the nearest in
        /// turn and the index really is holding all of them.
        #[test]
        fn records_are_ordered_by_population_inside_a_cell() {
            let writer = writer_with(&[
                ("Pequena", -23.0, -46.0, 1_000),
                ("Grande", -22.9, -46.0, 9_000),
                ("Media", -22.8, -46.0, 5_000),
            ]);
            let index = GeoIndex::from_bytes(writer.finish().unwrap()).unwrap();
            assert_eq!(index.record_count(), 3);
            assert_eq!(index.cell_count(), 1, "all three share one cell");
            for (lat, expected) in [(-23.0, "Pequena"), (-22.9, "Grande"), (-22.8, "Media")] {
                assert_eq!(
                    index.geocode(lat, -46.0).unwrap().city,
                    expected,
                    "at {lat}"
                );
            }
        }

        #[test]
        fn non_ascii_names_survive_the_round_trip() {
            let writer = writer_with(&[
                ("São Paulo", -23.5505, -46.6333, 12_252_023),
                ("Brasília", -15.7939, -47.8828, 4_775_000),
                ("Córdoba", -31.4201, -64.1888, 1_400_000),
            ]);
            let index = GeoIndex::from_bytes(writer.finish().unwrap()).unwrap();
            assert_eq!(index.geocode(-23.5505, -46.6333).unwrap().city, "São Paulo");
            assert_eq!(index.geocode(-15.7939, -47.8828).unwrap().city, "Brasília");
            assert_eq!(index.geocode(-31.4201, -64.1888).unwrap().city, "Córdoba");
        }

        /// A state with no name (GeoNames has countries whose admin1 is empty)
        /// must round-trip as an empty string, not as a missing record.
        #[test]
        fn an_empty_state_is_not_a_missing_city() {
            let mut writer = CityWriter::new(500);
            writer.push(City::new(35.6762, 139.6503, 13_960_000, "Tokyo", "", "JP"));
            let index = GeoIndex::from_bytes(writer.finish().unwrap()).unwrap();
            let found = index.geocode(35.6762, 139.6503).unwrap();
            assert_eq!(found.city, "Tokyo");
            assert_eq!(found.state, "");
            assert_eq!(found.country, "JP");
        }

        #[test]
        fn quantize_keeps_a_city_on_its_own_grid_point() {
            assert_eq!(quantize(-23.5505), -235_505_000);
            assert_eq!(quantize(0.0), 0);
            assert_eq!(quantize(90.0), 900_000_000);
        }

        /// §7.5's 16 MB budget, checked against a city count no single country
        /// reaches, rather than asserted in a comment.
        #[test]
        fn a_large_dataset_stays_inside_the_budget() {
            let mut writer = CityWriter::new(500);
            for i in 0..45_000u32 {
                let lat = -33.0 + (i % 500) as f64 * 0.1;
                let lon = -70.0 + (i / 500) as f64 * 0.1;
                writer.push(City::new(
                    lat,
                    lon,
                    1_000 + i,
                    &format!("Cidade Numero {i} do Teste"),
                    "Estado Longo Para ocupar bytes",
                    "BR",
                ));
            }
            let bytes = writer.finish().unwrap();
            let index = GeoIndex::from_bytes(bytes.clone()).unwrap();
            let report = writer.report(bytes.len());
            assert_eq!(report.cities, 45_000);
            assert!(report.within_budget(), "{} bytes", report.bytes);
            // The file plus the offset table, which is what the budget covers.
            assert!(
                index.resident_bytes() <= BYTE_BUDGET,
                "{}",
                index.resident_bytes()
            );
        }

        /// The writer derives the cell key itself and the reader re-derives it
        /// on every query. If they ever disagree, every city is unfindable, so
        /// the grid is pinned against hand-computed keys.
        #[test]
        fn the_writer_and_the_reader_agree_on_the_cell_grid() {
            // São Paulo: lat band floor(66.4495) = 66, lon band floor(133.3667) = 133.
            assert_eq!(cell_of(-23.5505, -46.6333), cell_key(66, 133));
            assert_eq!(cell_of(0.0, 0.0), cell_key(90, 180));
            assert_eq!(cell_of(90.0, 180.0), cell_key(179, 359));
            assert_eq!(cell_of(-90.0, -180.0), cell_key(0, 0));
        }

        /// Decision 2: a ring-2 cell is at least a full degree of latitude away,
        /// which is more than the cutoff. So the 3×3 expansion really does cover
        /// everything the cutoff can accept, and the bound is not a guess — it
        /// is checked by the compiler, so changing either number to break this
        /// is a build error rather than a silently wrong search.
        const _: () = assert!(
            DEG_LAT_KM > MAX_DIST_KM,
            "a ring-2 cell is further than the cutoff allows, so rings 0-1 cover it"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::geocode::writer::City;

    /// A small São Paulo / interior-SP dataset, built through the real writer so
    /// the reader and the format can never drift apart unnoticed.
    fn sao_paulo_index() -> GeoIndex {
        let mut builder = CityWriter::new(500);
        builder.push(City::new(
            -23.5505,
            -46.6333,
            12_252_023,
            "São Paulo",
            "SP",
            "BR",
        ));
        builder.push(City::new(
            -23.1829, -47.1744, 90_664, "Campinas", "SP", "BR",
        ));
        builder.push(City::new(
            -22.9068,
            -43.1729,
            6_748_813,
            "Rio de Janeiro",
            "RJ",
            "BR",
        ));
        let bytes = builder.finish().expect("the dataset is well formed");
        GeoIndex::from_bytes(bytes).unwrap()
    }

    #[test]
    fn finds_the_city_a_photo_was_taken_in() {
        let index = sao_paulo_index();
        let found = index.geocode(-23.5505, -46.6333).expect("São Paulo");
        assert_eq!(found.city, "São Paulo");
        assert_eq!(found.state, "SP");
        assert_eq!(found.country, "BR");
    }

    /// Decision 3: the DTO reports where the photo is. A city centroid here
    /// would quietly move a photo taken 20 km out into downtown.
    #[test]
    fn dto_carries_the_queried_point_not_the_centroid() {
        let index = sao_paulo_index();
        let found = index.geocode(-23.5105, -46.6033).expect("São Paulo");
        assert_eq!(found.city, "São Paulo");
        assert!(
            (found.latitude - -23.5105).abs() < 1e-9,
            "{}",
            found.latitude
        );
        assert!(
            (found.longitude - -46.6033).abs() < 1e-9,
            "{}",
            found.longitude
        );
    }

    /// §7.5's "nearest": a point 5 km from Campinas must not report São Paulo
    /// just because that one has more people.
    #[test]
    fn nearest_wins_over_biggest() {
        let index = sao_paulo_index();
        assert_eq!(
            index.geocode(-23.2029, -47.1944).expect("Campinas").city,
            "Campinas"
        );
    }

    /// The expansion is what makes a city near a cell edge findable. São Paulo
    /// sits at lat -23.55, so its own cell is the -24..-23 band; a point at
    /// -22.9 is in the band above it and still only ~72 km... which the 40 km
    /// cutoff refuses, so the fixture is a pair of cities straddling a band
    /// edge instead, with the query inside the budget.
    #[test]
    fn expands_into_the_neighbouring_cells() {
        let mut builder = CityWriter::new(500);
        // Two cells apart in latitude: -23.0 is band 67, -22.0 is band 68.
        builder.push(City::new(-23.0, -46.0, 900_000, "Noite", "SP", "BR"));
        builder.push(City::new(-22.0, -46.0, 800_000, "La", "SP", "BR"));
        let index = GeoIndex::from_bytes(builder.finish().unwrap()).unwrap();

        // The query is in band 67's neighbour and 39 km from the city above it,
        // so the answer can only come from the ring.
        let found = index
            .geocode(-22.35, -46.0)
            .expect("a city in the ring, inside the budget");
        assert_eq!(found.city, "La", "the nearer of the two");
    }

    #[test]
    fn refuses_a_candidate_beyond_the_radius() {
        let index = sao_paulo_index();
        // Open water: nothing in the index, and it must not fall back to "the
        // biggest city in the file".
        assert_eq!(index.geocode(-40.0, -30.0), None);
    }

    #[test]
    fn an_empty_index_answers_none() {
        let index =
            GeoIndex::from_bytes(CityWriter::new(500).finish().expect("empty is valid")).unwrap();
        assert!(index.is_empty());
        assert_eq!(index.geocode(-23.5505, -46.6333), None);
    }

    /// §7.5's tie-break, on two cities exactly mirrored about the query so the
    /// distance is equal to the last bit and only the population separates them.
    /// Both are 11 km away, inside the 40 km the cutoff allows.
    #[test]
    fn population_breaks_an_exact_distance_tie() {
        let mut builder = CityWriter::new(500);
        builder.push(City::new(0.1, 0.0, 5_000, "Pequena", "SP", "BR"));
        builder.push(City::new(-0.1, 0.0, 9_000, "Grande", "SP", "BR"));
        let index = GeoIndex::from_bytes(builder.finish().unwrap()).unwrap();

        let found = index.geocode(0.0, 0.0).expect("both are ~11 km away");
        assert_eq!(found.city, "Grande", "same distance, bigger city wins");
    }

    #[test]
    fn refuses_coordinates_that_are_not_places() {
        let index = sao_paulo_index();
        for (lat, lon) in [
            (f64::NAN, -46.6333),
            (-23.5505, f64::NAN),
            (f64::INFINITY, 0.0),
            (91.0, 0.0),
            (-91.0, 0.0),
            (0.0, 180.5),
            (0.0, -180.5),
        ] {
            assert_eq!(index.geocode(lat, lon), None, "({lat}, {lon}) must be None");
        }
    }

    #[test]
    fn the_poles_and_the_antimeridian_do_not_panic() {
        let index = sao_paulo_index();
        for (lat, lon) in [(90.0, 180.0), (-90.0, -180.0), (90.0, -180.0), (0.0, 180.0)] {
            let _ = index.geocode(lat, lon);
        }
    }

    #[test]
    fn every_cell_key_fits_the_u16_the_catalog_reserved() {
        assert_eq!(CELL_COUNT, 64800);
        assert!(u16::try_from(CELL_COUNT - 1).is_ok());
        assert_eq!(cell_key(0, 0), 0);
        assert_eq!(
            cell_key(LAT_BANDS - 1, LON_BANDS - 1),
            (CELL_COUNT - 1) as u16
        );
    }

    #[test]
    fn haversine_matches_known_distances() {
        // São Paulo → Rio is ~360 km. 0.1° of longitude there is ~10.2 km,
        // because a degree of longitude is 111.32 x cos(latitude) km and São
        // Paulo is at 23.5° south.
        let whole = haversine_km(-23.5505, -46.6333, -22.9068, -43.1729);
        assert!((whole - 360.0).abs() < 10.0, "{whole} km");
        let tenth = haversine_km(-23.5505, -46.6333, -23.5505, -46.5333);
        assert!((tenth - 10.2).abs() < 0.3, "{tenth} km");
        // A degree of latitude is the one distance that does not shrink.
        let degree = haversine_km(0.0, 0.0, 1.0, 0.0);
        assert!((degree - 111.2).abs() < 0.5, "{degree} km");
        assert_eq!(haversine_km(0.0, 0.0, 0.0, 0.0), 0.0);
    }

    #[test]
    fn rejects_a_file_that_is_not_an_index() {
        assert!(matches!(
            GeoIndex::from_bytes(b"nao sou um indice".to_vec()),
            Err(GeoError::BadMagic)
        ));
        assert!(matches!(
            GeoIndex::from_bytes(Vec::new()),
            Err(GeoError::Truncated(_))
        ));
        assert!(matches!(
            GeoIndex::from_bytes(b"half".to_vec()),
            Err(GeoError::Truncated(_))
        ));
    }

    #[test]
    fn rejects_a_future_version_instead_of_reading_it() {
        let mut data = sao_paulo_index().data;
        data[8..10].copy_from_slice(&2u16.to_le_bytes());
        assert!(matches!(
            GeoIndex::from_bytes(data),
            Err(GeoError::BadVersion { found: 2 })
        ));
    }

    #[test]
    fn rejects_a_truncated_body() {
        let full = sao_paulo_index();
        assert!(matches!(
            GeoIndex::from_bytes(full.data[..full.records_start + 4].to_vec()),
            Err(GeoError::Truncated(_))
        ));
    }

    /// The cell table is a lookup table, so a cell pointing outside the record
    /// array is a memory-safety bug, not a data glitch.
    #[test]
    fn rejects_a_cell_pointing_past_the_records() {
        let mut data = sao_paulo_index().data;
        let first_cell = HEADER_LEN;
        data[first_cell + 4..first_cell + 8].copy_from_slice(&9000u32.to_le_bytes());
        data[first_cell + 2..first_cell + 4].copy_from_slice(&1u16.to_le_bytes());
        assert!(matches!(
            GeoIndex::from_bytes(data),
            Err(GeoError::Truncated(_))
        ));
    }

    /// A hostile length must not wrap the bounds check.
    #[test]
    fn rejects_an_oversized_cell_count() {
        let mut data = sao_paulo_index().data;
        data[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            GeoIndex::from_bytes(data),
            Err(GeoError::Truncated(_))
        ));
    }

    #[test]
    fn a_missing_index_is_none_and_never_panics() {
        let dir = tempfile::tempdir().unwrap();
        let geocoder = Geocoder::new(dir.path().join("geonames.bin"));
        assert!(!geocoder.available());
        assert_eq!(geocoder.geocode(-23.5505, -46.6333), None);
        // And it stays that way without re-reading the file.
        assert_eq!(geocoder.geocode(-22.9068, -43.1729), None);
    }

    #[test]
    fn a_broken_index_is_treated_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("geonames.bin");
        std::fs::write(&path, b"lixo").unwrap();
        let geocoder = Geocoder::new(path);
        assert!(!geocoder.available());
        assert_eq!(geocoder.geocode(-23.5505, -46.6333), None);
    }

    /// The load is lazy *and* memoized: constructing the handle touches
    /// nothing, and a second `preload` is free because `OnceLock` already holds
    /// the index.
    #[test]
    fn preload_makes_the_index_available_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("geonames.bin");
        std::fs::write(&path, sao_paulo_index().data).unwrap();

        // Nothing is read by constructing it: a handle can be built for a root
        // that does not exist yet.
        let geocoder = Geocoder::new(path.clone());
        geocoder.preload();
        assert!(geocoder.available());
        assert_eq!(
            geocoder.geocode(-23.5505, -46.6333).unwrap().city,
            "São Paulo"
        );

        geocoder.preload();
        assert!(geocoder.available());
        assert_eq!(
            geocoder.geocode(-23.5505, -46.6333).unwrap().city,
            "São Paulo"
        );
    }

    /// The cache is bounded at §7.5's 512 and stays there. It needs *distinct*
    /// keys to fill: a thousand repeats of one point is one entry, and a test
    /// that asserted 512 here would be asserting that repeats are cached
    /// separately, which is the opposite of the point.
    #[test]
    fn the_lru_stays_at_capacity_under_many_distinct_points() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("geonames.bin");
        std::fs::write(&path, sao_paulo_index().data).unwrap();
        let geocoder = Geocoder::new(path);

        // A spread of points over the São Paulo cell, all inside the budget so
        // each one is really cached.
        let mut answered = 0;
        for i in 0..(CACHE_CAPACITY * 2) {
            let lat = -23.55 + (i as f64) * 0.0001;
            if geocoder.geocode(lat, -46.6333).is_some() {
                answered += 1;
            }
        }
        assert_eq!(answered, CACHE_CAPACITY * 2, "every point is inside 40 km");
        assert_eq!(
            geocoder.lock_cache().len(),
            CACHE_CAPACITY,
            "and the cache is capped"
        );
    }

    /// A repeated point is served from the cache, so the second call does not
    /// even need the index. Pinned by the fact that the file is deleted after
    /// the first answer: a re-read would fail and the cache would miss.
    #[test]
    fn a_repeat_is_served_from_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("geonames.bin");
        std::fs::write(&path, sao_paulo_index().data).unwrap();
        let geocoder = Geocoder::new(path.clone());

        let first = geocoder.geocode(-23.5505, -46.6333).expect("first read");
        std::fs::remove_file(&path).unwrap();
        let second = geocoder
            .geocode(-23.5505, -46.6333)
            .expect("from the cache");
        assert_eq!(first, second);
    }

    #[test]
    fn the_lru_evicts_the_least_recently_used() {
        let mut lru = Lru::new(2);
        let key = |lat: f64| CacheKey::new(lat, 0.0).unwrap();
        let city = |name: &str| GeoDto {
            city: name.into(),
            state: String::new(),
            country: String::new(),
            latitude: 0.0,
            longitude: 0.0,
        };

        lru.put(key(1.0), city("a"));
        lru.put(key(2.0), city("b"));
        assert_eq!(
            lru.get(&key(1.0)).unwrap().city,
            "a",
            "reading a refreshes it"
        );
        lru.put(key(3.0), city("c"));

        assert_eq!(lru.len(), 2);
        assert!(lru.get(&key(1.0)).is_some(), "a was refreshed, so it stays");
        assert!(
            lru.get(&key(2.0)).is_none(),
            "b was the coldest and had to go"
        );
        assert_eq!(lru.get(&key(3.0)).unwrap().city, "c");
    }

    #[test]
    fn an_unusable_coordinate_is_not_cached() {
        assert!(CacheKey::new(f64::NAN, 0.0).is_none());
        assert!(CacheKey::new(95.0, 0.0).is_none());
    }
}
