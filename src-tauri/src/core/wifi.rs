//! Wi-Fi ingestion (PLAN §7.8, §12.2): the phone's browser is the ingest UI.
//!
//! There is no app on the phone (PLAN §3.1 decision 8), so the whole flow is a
//! `tiny_http` listener on the LAN: `GET /` serves a one-page uploader,
//! `POST /upload` takes multipart bodies, `POST /done` closes a batch. The QR
//! carries `http://<lan-ip>:8642` and the phone is done.
//!
//! **Where the bytes go.** An upload streams straight to
//! `Database/staging/wifi/<uuid>.part` — never into `Media/`, because a partial
//! file inside `Media/` is a file the gallery and the scanner would find (the
//! human-confirmed decision, 2026-10-02). Only a complete, parseable, supported
//! file is renamed into `Media/<device>/<YYYY>/<MM>/` (or `Sem_Metadados/`, per
//! C7), and the rename is same-volume because staging lives under the same root.
//!
//! **What happens after.** Placement is cheap (one EXIF read); the expensive part
//! is §12.1's pipeline — hash, dedupe, geo, thumbs — which is §7.3's scanner
//! itself. §12.3 says a scan is what runs after a Wi-Fi drop-in, so uploads do
//! not duplicate the scanner: they hand the device to the same pipeline job, and
//! its `ScanSummary` is where the dedupe count comes from (the gate's
//! "dedup counted"). Because that scan walks the whole device tree, a burst of
//! N uploads costs **one** scan, not N.
//!
//! **Auth: none.** §7.8 as written has no token, and the human chose that on
//! 2026-10-02 knowing a shared network means anyone on the LAN can write here
//! while the server is on. The stop button is the mitigation.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use chrono::Datelike;
use tiny_http::{Header, Response, Server, StatusCode};

use crate::core::db::SqlitePool;
use crate::core::exif::{self, RawMetadata};
use crate::core::geocode::Geocoder;
use crate::core::models::{IngestEventDto, ScanSummary, WifiInfoDto};
use crate::core::paths::{self, NO_META_DIR, PathResolver};
use crate::core::scanner::{self, ScanSource};
use crate::error::AppError;
use crate::state::{Job, JobKind, JobRegistry};

/// §7.8's constant port ("pulse 8642"). Not configurable: the QR and the page
/// both hardcode it, and a second app on the same port is the user's problem to
/// see (E_CONFLICT), not ours to paper over.
pub const PORT: u16 = 8642;

/// A LAN upload of a 4K video is minutes and gigabytes; the cap only exists to
/// stop a runaway request from filling the SSD with a `.part` nobody will move.
const MAX_UPLOAD_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// A form field (the device label) is one short line. Anything longer is abuse.
const MAX_FIELD_BYTES: usize = 256;
/// `tiny_http` is one accept loop; each request gets its own thread, and this is
/// how many may run at once before the server answers 503 instead of forking
/// without bound.
const MAX_IN_FLIGHT: usize = 4;
/// Sanity bound on one multipart body's part count (the browser sends one part
/// per file, plus one for the device label).
const MAX_PARTS: usize = 2_000;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// `tiny_http`'s response is generic over its data source; every response here
/// is built from a string, so this alias keeps the signatures readable.
type Reply = Response<std::io::Cursor<Vec<u8>>>;

fn with_type(response: Reply, mime: &str) -> Reply {
    match Header::from_bytes(Vec::new(), format!("Content-Type: {mime}").into_bytes()) {
        Ok(header) => response.with_header(header),
        // A literal ASCII header we just built: this cannot fail in practice.
        Err(_) => response,
    }
}

fn html(body: &'static str) -> Reply {
    with_type(Response::from_string(body), "text/html; charset=utf-8")
}

/// What the live server and the pipeline both need. Cloneable, because the
/// accept loop, each request thread and the pipeline job all hold one.
#[derive(Clone)]
pub struct Shared {
    pub resolver: PathResolver,
    pub pool: SqlitePool,
    pub geocoder: Arc<Geocoder>,
    /// `None` when §5.5 found no `App/bin/ffmpeg.exe`: videos then land in
    /// `Sem_Metadados/` and the scanner re-places them once ffmpeg exists.
    pub ffmpeg: Option<PathBuf>,
    pub jobs: Arc<JobRegistry>,
    counters: Arc<Counters>,
    pipeline: Pipeline,
    in_flight: Arc<AtomicUsize>,
}

#[derive(Default)]
struct Counters {
    uploads: AtomicU64,
    bytes: AtomicU64,
    errors: AtomicU64,
}

/// The pipeline's own bookkeeping. `lock` is what makes "one pipeline job at a
/// time" true across *threads*: an upload takes it to publish its pending work,
/// and the job takes it for the exit check, so a file can never land in the gap
/// between "nothing left" and "slot released".
#[derive(Clone)]
struct Pipeline {
    inner: Arc<PipelineInner>,
}

struct PipelineInner {
    lock: Mutex<()>,
    pending: Mutex<BTreeMap<String, usize>>,
    sink: Mutex<Box<dyn FnMut(IngestEventDto) + Send>>,
}

impl Shared {
    pub fn new(
        resolver: PathResolver,
        pool: SqlitePool,
        geocoder: Arc<Geocoder>,
        ffmpeg: Option<PathBuf>,
        jobs: Arc<JobRegistry>,
    ) -> Self {
        Self {
            resolver,
            pool,
            geocoder,
            ffmpeg,
            jobs,
            counters: Arc::new(Counters::default()),
            pipeline: Pipeline::new(),
            in_flight: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Where background facts reach the UI (§9.2). Tests pass a collector; the
    /// command passes the app handle.
    pub fn with_event_sink(mut self, sink: impl FnMut(IngestEventDto) + Send + 'static) -> Self {
        self.pipeline = Pipeline::with_sink(sink);
        self
    }

    /// §11.3's live counter ("aguardando uploads… n enviados"). c13 polls it by
    /// re-calling `ingest_wifi_start`, which is idempotent.
    pub fn uploads(&self) -> i64 {
        self.counters.uploads.load(Ordering::Relaxed) as i64
    }

    pub fn bytes_received(&self) -> i64 {
        self.counters.bytes.load(Ordering::Relaxed) as i64
    }

    pub fn errors(&self) -> i64 {
        self.counters.errors.load(Ordering::Relaxed) as i64
    }

    /// Hands the device to §12.1's pipeline, starting the job if none is running.
    ///
    /// The lock covers the pending bump *and* the slot check, which is the whole
    /// point: a request thread and the running job's exit check can interleave,
    /// but never both decide "nobody is handling this".
    fn request_pipeline(&self, device: &str) {
        let _guard = self
            .pipeline
            .inner
            .lock
            .lock()
            .expect("Pipeline lock poisoned");
        *self
            .pipeline
            .inner
            .pending
            .lock()
            .expect("Pipeline pending poisoned")
            .entry(device.to_string())
            .or_insert(0) += 1;

        if self.jobs.is_running(JobKind::IngestWifi) {
            return;
        }
        if let Ok(job) = self.jobs.claim(JobKind::IngestWifi) {
            let shared = self.clone();
            std::thread::spawn(move || run_pipeline(shared, job));
        }
    }

    /// One extra scan pass now, without claiming the job slot: what `POST /done`
    /// uses so the phone's last batch is processed on its own instead of waiting
    /// for the next upload to wake the loop. A busy slot is fine — the running
    /// job will re-check the pending map itself.
    fn kick_pipeline(&self) {
        let _guard = self
            .pipeline
            .inner
            .lock
            .lock()
            .expect("Pipeline lock poisoned");
        if self.jobs.is_running(JobKind::IngestWifi) {
            return;
        }
        if let Ok(job) = self.jobs.claim(JobKind::IngestWifi) {
            let shared = self.clone();
            std::thread::spawn(move || run_pipeline(shared, job));
        }
    }

    fn emit(&self, event: IngestEventDto) {
        let mut sink = self
            .pipeline
            .inner
            .sink
            .lock()
            .expect("Pipeline sink poisoned");
        sink(event);
    }

    /// §9.1's in-flight cap. Returns false when the server is already full.
    fn enter(&self) -> bool {
        let mut current = self.in_flight.load(Ordering::Acquire);
        loop {
            if current >= MAX_IN_FLIGHT {
                return false;
            }
            match self.in_flight.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
    }

    fn leave(&self) {
        self.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Pipeline {
    fn new() -> Self {
        Self::with_sink(|_| {})
    }

    fn with_sink(sink: impl FnMut(IngestEventDto) + Send + 'static) -> Self {
        Self {
            inner: Arc::new(PipelineInner {
                lock: Mutex::new(()),
                pending: Mutex::new(BTreeMap::new()),
                sink: Mutex::new(Box::new(sink)),
            }),
        }
    }

    /// Empties the pending map: the devices this pass must scan.
    fn take(&self) -> Vec<(String, usize)> {
        let mut pending = self
            .inner
            .pending
            .lock()
            .expect("Pipeline pending poisoned");
        std::mem::take(&mut *pending).into_iter().collect()
    }

    fn is_pending(&self) -> bool {
        !self
            .inner
            .pending
            .lock()
            .expect("Pipeline pending poisoned")
            .is_empty()
    }
}

/// §12.1's pipeline, driven by the scanner. One device per pass, looping while
/// more uploads arrive; the summary it emits is the dedupe count.
fn run_pipeline(shared: Shared, job: Job) {
    loop {
        let work = shared.pipeline.take();
        if work.is_empty() {
            // "Last one out turns off the lights": with the lock held, an upload
            // either already published its pending entry (so this loops again) or
            // will see the released slot and start a new job itself.
            let _guard = shared
                .pipeline
                .inner
                .lock
                .lock()
                .expect("Pipeline lock poisoned");
            if !shared.pipeline.is_pending() {
                shared.jobs.release(JobKind::IngestWifi, job.id());
                return;
            }
            continue;
        }

        let mut totals = ScanSummary::default();
        for (device, _) in work {
            let summary = match scanner::scan(
                &device,
                &shared.resolver,
                &shared.pool,
                &shared.geocoder,
                shared.ffmpeg.as_deref(),
                ScanSource::Device,
                &job.flag,
                |progress| shared.emit(IngestEventDto::Progress(progress.clone())),
            ) {
                Ok(summary) => summary,
                Err(AppError::Cancelled) => {
                    shared.jobs.release(JobKind::IngestWifi, job.id());
                    return;
                }
                Err(error) => {
                    // One bad device must not kill the pass: the phone is already
                    // uploading the next batch.
                    shared.counters.errors.fetch_add(1, Ordering::Relaxed);
                    shared.emit(IngestEventDto::Failed {
                        device: device.clone(),
                        message: error.to_string(),
                    });
                    continue;
                }
            };
            accumulate(&mut totals, &summary);
            shared.emit(IngestEventDto::Summary(summary));
        }
        if totals.inserted > 0 {
            shared.emit(IngestEventDto::DbChanged);
        }
    }
}

fn accumulate(totals: &mut ScanSummary, summary: &ScanSummary) {
    totals.found += summary.found;
    totals.inserted += summary.inserted;
    totals.duplicates += summary.duplicates;
    totals.repaired += summary.repaired;
    totals.organized += summary.organized;
    totals.no_metadata += summary.no_metadata;
    totals.located += summary.located;
    totals.located_none += summary.located_none;
    totals.skipped += summary.skipped;
}

// ---------------------------------------------------------------------------
// The server
// ---------------------------------------------------------------------------

/// A running listener. `stop()` closes it; everything else is read-only.
pub struct WifiServer {
    server: Mutex<Option<Arc<Server>>>,
    cancel: Arc<AtomicBool>,
    join: Mutex<Option<JoinHandle<()>>>,
    shared: Shared,
    info: WifiInfoDto,
    lan_ip: Ipv4Addr,
    port: u16,
    payload: String,
}

impl WifiServer {
    pub fn info(&self) -> WifiInfoDto {
        WifiInfoDto {
            uploads: self.shared.uploads(),
            ..self.info.clone()
        }
    }

    pub fn lan_ip(&self) -> Ipv4Addr {
        self.lan_ip
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn url(&self) -> &str {
        &self.payload
    }

    /// §12.2's `[C-3k Stop]`: the accept loop ends, in-flight uploads finish
    /// (they are already inside a request thread with their own socket), and the
    /// pipeline job is left alone — §12.2 says it keeps running.
    pub fn stop(&self) -> Result<(), AppError> {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(server) = self
            .server
            .lock()
            .expect("WifiServer server poisoned")
            .as_ref()
        {
            server.unblock();
        }
        let handle = self.join.lock().expect("WifiServer join poisoned").take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
        // The accept loop is done and its own `Arc` is gone with the thread,
        // so this is the last reference: dropping it closes the listener. A
        // `stop()` that left the port open would let a phone keep a
        // connection that nobody answers.
        drop(
            self.server
                .lock()
                .expect("WifiServer server poisoned")
                .take(),
        );
        // tiny_http's accept thread owns the socket and exits asynchronously
        // after the `Server` is dropped, so the port can stay open for a
        // moment. Wait for it to actually close: `stop()` must not return
        // while a phone could still connect.
        wait_for_port_close(self.port);
        // A half-written upload from this session is ours to clean up.
        if let Ok(staging) = staging_dir(&self.shared.resolver) {
            let _ = purge_staging(&staging);
        }
        Ok(())
    }
}

/// Blocks until nothing listens on `port` (or a deadline passes). Used by
/// [`WifiServer::stop`] so the port is provably closed when `stop` returns.
fn wait_for_port_close(port: u16) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
        if std::time::Instant::now() > deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Binds and starts accepting. Idempotency is the caller's business (the command
/// returns the live server's info instead of binding twice).
pub fn start(shared: Shared) -> Result<WifiServer, AppError> {
    start_on(shared, PORT)
}

/// `port` is a parameter only so the tests can bind an ephemeral port instead of
/// fighting a dev instance for 8642.
pub fn start_on(shared: Shared, port: u16) -> Result<WifiServer, AppError> {
    let staging = staging_dir(&shared.resolver)?;
    // Only this module writes here, so a leftover `.part` is always a dead
    // upload from a previous run (or a crash) and is never the user's data.
    purge_staging(&staging)?;

    let server = Server::http(("0.0.0.0", port))
        .map_err(|error| AppError::Device(format!("não consegui abrir a porta {port}: {error}")))?;
    let bound: SocketAddr = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| AppError::Device("a porta não tem endereço local".into()))?;

    // The QR is useless without a reachable address, so this is where a
    // loopback-only machine fails — loudly, at start, instead of showing a QR
    // the phone cannot open.
    let lan_ip =
        lan_ip().ok_or_else(|| AppError::Device("nenhum endereço IPv4 de rede local".into()))?;
    let bound_port = if bound.port() == 0 {
        port
    } else {
        bound.port()
    };

    let cancel = Arc::new(AtomicBool::new(false));
    let accept_server = Arc::new(server);
    let thread_server = Arc::clone(&accept_server);
    let thread_shared = shared.clone();
    let thread_cancel = Arc::clone(&cancel);
    let join = std::thread::spawn(move || accept_loop(thread_server, thread_shared, thread_cancel));

    let payload = format!("http://{lan_ip}:{bound_port}");
    let qr_matrix = qr_matrix(&payload)?;
    let info = WifiInfoDto {
        lan_ip: lan_ip.to_string(),
        qr_matrix,
        port: bound_port,
        uploads: shared.uploads(),
    };

    Ok(WifiServer {
        server: Mutex::new(Some(accept_server)),
        cancel,
        join: Mutex::new(Some(join)),
        shared,
        info,
        lan_ip,
        port: bound_port,
        payload,
    })
}

fn accept_loop(server: Arc<Server>, shared: Shared, cancel: Arc<AtomicBool>) {
    while !cancel.load(Ordering::Relaxed) {
        let request = match server.recv() {
            Ok(request) => request,
            Err(_) => break, // `unblock()` from `stop()`
        };
        if !shared.enter() {
            let _ = request.respond(json(
                503,
                r#"{"ok":false,"message":"servidor ocupado"}"#.to_string(),
            ));
            continue;
        }
        let request_shared = shared.clone();
        std::thread::spawn(move || {
            request_shared.leave();
            handle(request_shared, request);
        });
    }
}

fn handle(shared: Shared, mut request: tiny_http::Request) {
    let path = request.url().split('?').next().unwrap_or("/").to_string();

    let response = match (request.method().as_str(), path.as_str()) {
        ("GET", "/") => html(page::PAGE_PT),
        ("GET", "/en") => html(page::PAGE_EN),
        ("GET", "/done") => json(200, done_payload(&shared)),
        ("POST", "/upload") => match handle_upload(&shared, &mut request) {
            Ok(payload) => json(200, payload),
            Err(response) => response,
        },
        ("POST", "/done") => {
            // §12.2's `/done`: the phone is finished, so the batch is processed
            // now instead of waiting for the next upload to wake the loop.
            shared.kick_pipeline();
            json(200, done_payload(&shared))
        }
        _ => json(
            404,
            r#"{"ok":false,"message":"rota desconhecida"}"#.to_string(),
        ),
    };
    let _ = request.respond(response);
}

fn json(status: u16, body: String) -> Reply {
    with_type(
        Response::from_string(body).with_status_code(StatusCode(status)),
        "application/json",
    )
}

fn done_payload(shared: &Shared) -> String {
    format!(
        r#"{{"ok":true,"uploads":{},"bytes":{},"errors":{}}}"#,
        shared.uploads(),
        shared.bytes_received(),
        shared.errors()
    )
}

// ---------------------------------------------------------------------------
// POST /upload
// ---------------------------------------------------------------------------

/// `POST /upload` — one multipart body, possibly many parts.
///
/// Each file part gets **its own** staging file: the parts are streamed one after
/// another, and a single shared `.part` would have the second file appended to
/// the first one's bytes after the first had already been renamed into `Media/`.
fn handle_upload(shared: &Shared, request: &mut tiny_http::Request) -> Result<String, Reply> {
    let boundary = request
        .headers()
        .iter()
        .find_map(|header| {
            if header.field.equiv("Content-Type") {
                parse_boundary(header.value.as_str())
            } else {
                None
            }
        })
        .ok_or_else(|| bad_request("Content-Type sem boundary"))?;

    let staging = staging_dir(&shared.resolver).map_err(server_error)?;
    let staged = stage_parts(shared, request.as_reader(), &boundary, &staging)?;

    if !staged.placed.is_empty() {
        shared.request_pipeline(&staged.device);
    }
    Ok(payload_json(&staged))
}

/// What one body produced. `device` is the label the pipeline must scan.
struct Staged {
    device: String,
    placed: Vec<String>,
    rejected: Vec<String>,
}

fn stage_parts(
    shared: &Shared,
    reader: &mut dyn Read,
    boundary: &str,
    staging: &Path,
) -> Result<Staged, Reply> {
    let mut parts = multipart::Reader::new(reader, boundary);
    let mut device = String::new();
    let mut placed = Vec::new();
    let mut rejected = Vec::new();
    let mut seen = 0usize;

    while let Some(part) = parts
        .next_part()
        .map_err(|error| server_error(error.to_string()))?
    {
        seen += 1;
        if seen > MAX_PARTS {
            return Err(bad_request("excesso de partes no formulário"));
        }
        match part.filename.as_deref() {
            None => {
                let value = parts
                    .text_body(MAX_FIELD_BYTES)
                    .map_err(|error| server_error(error.to_string()))?;
                if part.name == "device" {
                    device = sanitize_device(&value);
                }
            }
            Some(raw_name) => {
                let part_path = staging.join(format!("{}.part", uuid::Uuid::new_v4()));
                let outcome = stage_one(&mut parts, &part_path)
                    .and_then(|()| place(shared, &device, raw_name, &part_path));
                match outcome {
                    Ok(relative) => placed.push(relative),
                    // A rejected file is reported, not fatal: the phone may have
                    // picked a format this build cannot read, and the rest of the
                    // batch is still the user's media.
                    Err(message) => {
                        let _ = fs::remove_file(&part_path);
                        shared.counters.errors.fetch_add(1, Ordering::Relaxed);
                        rejected.push(message);
                    }
                }
            }
        }
    }

    if placed.is_empty() && rejected.is_empty() {
        return Err(bad_request("corpo sem arquivos"));
    }
    Ok(Staged {
        device: if device.is_empty() {
            paths::sanitize_label("")
        } else {
            device
        },
        placed,
        rejected,
    })
}

/// One file part, from the socket to a closed, synced `.part`.
fn stage_one<R: Read>(parts: &mut multipart::Reader<R>, staged: &Path) -> Result<(), String> {
    let file = File::create(staged).map_err(|error| error.to_string())?;
    let mut sink = BufWriter::new(file);
    parts
        .stream_body(&mut sink, MAX_UPLOAD_BYTES)
        .map_err(|error| error.to_string())?;
    sink.flush().map_err(|error| error.to_string())?;
    // §14: the bytes are nowhere else until this rename, so the sync is the
    // difference between "uploaded" and "on the SSD".
    let _ = sink.get_ref().sync_all();
    drop(sink);
    Ok(())
}

fn payload_json(staged: &Staged) -> String {
    let placed = staged
        .placed
        .iter()
        .map(|relative| format!("\"{relative}\""))
        .collect::<Vec<_>>()
        .join(",");
    let rejected = staged
        .rejected
        .iter()
        .map(|message| format!("\"{}\"", message.replace('"', "'")))
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"ok":true,"placed":[{placed}],"rejected":[{rejected}]}}"#)
}

fn sanitize_device(raw: &str) -> String {
    paths::sanitize_device_label(raw)
        .or_else(|_| Ok::<String, AppError>(paths::sanitize_label(raw)))
        .unwrap_or_default()
}

fn parse_boundary(content_type: &str) -> Option<String> {
    let lowered = content_type.to_ascii_lowercase();
    if !lowered.starts_with("multipart/form-data") {
        return None;
    }
    let index = lowered.find("boundary=")?;
    let raw = content_type[index + "boundary=".len()..].trim();
    let raw = raw.split(';').next().unwrap_or(raw).trim();
    let raw = raw.trim_matches('"').trim();
    if raw.is_empty() {
        None
    } else {
        Some(raw.to_string())
    }
}

/// The moment of truth: the staged bytes become a library file.
///
/// Order matters and follows §7.8: reject an unsupported type **before** any
/// metadata read, sanitize both the label and the name (§5.4), then choose the
/// folder by C7 and rename. A failure here deletes the staging file — the phone
/// still has the original, so nothing is lost (§14).
fn place(
    shared: &Shared,
    device_raw: &str,
    raw_name: &str,
    staged: &Path,
) -> Result<String, String> {
    let label = paths::sanitize_device_label(device_raw).map_err(|error| error.to_string())?;
    let name = safe_file_name(raw_name).map_err(|error| error.to_string())?;

    let extension = extension_of(&name);
    if !exif::IMAGE_EXTS.contains(&extension.as_str())
        && !exif::VIDEO_EXTS.contains(&extension.as_str())
    {
        return Err(format!("tipo não suportado: {name}"));
    }

    let metadata = exif::extract(staged, shared.ffmpeg.as_deref()).ok();
    let directory = target_dir(&shared.resolver, &label, metadata.as_ref())
        .map_err(|error| error.to_string())?;
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;

    let target =
        scanner::free_destination(&directory.join(&name)).map_err(|error| error.to_string())?;
    fs::rename(staged, &target).map_err(|error| error.to_string())?;

    let relative = shared
        .resolver
        .abs_to_rel(&target)
        .map_err(|error| error.to_string())?;

    shared.counters.uploads.fetch_add(1, Ordering::Relaxed);
    shared.counters.bytes.fetch_add(
        fs::metadata(&target).map(|meta| meta.len()).unwrap_or(0),
        Ordering::Relaxed,
    );
    Ok(relative)
}

/// §7.8: `<YYYY>/<MM>/` for a file with metadata of its own, `Sem_Metadados/`
/// otherwise — C7's single definition, the same one the scanner uses. A file
/// placed in the wrong folder self-heals: §7.3's organize step renames it on the
/// next pass.
fn target_dir(
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

/// A browser's `filename` is attacker-controlled and historically arrived as
/// `C:\fakepath\IMG_0001.jpg`. Only the last component is ever used, and §5.4's
/// Windows rules still have the last word.
fn safe_file_name(raw: &str) -> Result<String, AppError> {
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

fn extension_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn staging_dir(resolver: &PathResolver) -> Result<PathBuf, AppError> {
    let dir = resolver.db_dir().join("staging").join("wifi");
    fs::create_dir_all(&dir).map_err(|error| {
        AppError::Root(paths::PathError::RootNotADirectory(format!(
            "{}: {error}",
            dir.display()
        )))
    })?;
    Ok(dir)
}

fn purge_staging(dir: &Path) -> Result<(), AppError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        if entry.path().is_file() {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

fn bad_request(message: &str) -> Reply {
    json(
        400,
        format!(
            r#"{{"ok":false,"message":"{}"}}"#,
            message.replace('"', "'")
        ),
    )
}

fn server_error(message: impl ToString) -> Reply {
    json(
        500,
        format!(
            r#"{{"ok":false,"message":"{}"}}"#,
            message.to_string().replace('"', "'")
        ),
    )
}

// ---------------------------------------------------------------------------
// LAN address + QR
// ---------------------------------------------------------------------------

/// The address the phone can actually reach.
///
/// `if-addrs` cannot see the routing table, so §7.8's "the adapter that has the
/// default route" is approximated (human-confirmed 2026-10-02) in two steps: an
/// adapter whose **name** says it is a virtual bridge is demoted below every real
/// one, and what is left is ordered by range — home-router `192.168/16` first,
/// then `10/8`, then `172.16/12`, and public addresses last. A dev box with
/// VirtualBox or WSL would otherwise advertise an address no phone shares.
pub fn lan_ip() -> Option<Ipv4Addr> {
    let interfaces = if_addrs::get_if_addrs().ok()?;
    let mut candidates: Vec<(String, Ipv4Addr)> = Vec::new();
    for interface in interfaces {
        if let if_addrs::IfAddr::V4(v4) = interface.addr
            && usable(v4.ip)
        {
            candidates.push((interface.name, v4.ip));
        }
    }
    pick(&candidates)
}

fn pick(candidates: &[(String, Ipv4Addr)]) -> Option<Ipv4Addr> {
    candidates
        .iter()
        .min_by_key(|(name, ip)| (rank(name, *ip), *ip))
        .map(|(_, ip)| *ip)
}

fn usable(ip: Ipv4Addr) -> bool {
    !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_unspecified()
        && !ip.is_multicast()
        && !ip.is_broadcast()
}

/// Adapter names Windows gives the bridges a phone cannot reach.
const VIRTUAL_HINTS: &[&str] = &[
    "virtual",
    "vethernet",
    "vmware",
    "vbox",
    "hyper-v",
    "loopback",
    "bluetooth",
    "tailscale",
    "zerotier",
    "docker",
    "wsl",
];

fn rank(name: &str, ip: Ipv4Addr) -> u8 {
    if VIRTUAL_HINTS
        .iter()
        .any(|hint| name.to_ascii_lowercase().contains(hint))
    {
        return 9;
    }
    let [a, b, ..] = ip.octets();
    match (a, b) {
        (192, 168) => {
            if in_cidr(ip, "192.168.56.0", 21) {
                9
            } else {
                0
            }
        }
        (10, _) => 1,
        // Docker/WSL bridges are 172.17-31; a real router in 172.16/12 is rare
        // enough that demoting the bridge is the better trade.
        (172, 16..=31) => 1,
        _ => 4,
    }
}

fn in_cidr(ip: Ipv4Addr, network: &str, prefix: u8) -> bool {
    let Ok(network) = network.parse::<Ipv4Addr>() else {
        return false;
    };
    let mask = u32::MAX << (32 - prefix as u32);
    u32::from(ip) & mask == u32::from(network) & mask
}

/// §7.8: the matrix the frontend paints on a `<canvas>`. `Vec<Vec<bool>>` is
/// row-major, `true` = dark module.
pub fn qr_matrix(payload: &str) -> Result<Vec<Vec<bool>>, AppError> {
    let code = qrcodegen::QrCode::encode_text(payload, qrcodegen::QrCodeEcc::Medium)
        .map_err(|error| AppError::Ingest(format!("não consegui gerar o QR: {error}")))?;
    let size = code.size() as usize;
    let mut rows = Vec::with_capacity(size);
    for y in 0..size {
        let mut row = Vec::with_capacity(size);
        for x in 0..size {
            row.push(code.get_module(x as i32, y as i32));
        }
        rows.push(row);
    }
    Ok(rows)
}

// ---------------------------------------------------------------------------
// multipart/form-data, streamed
// ---------------------------------------------------------------------------

/// A minimal, streaming `multipart/form-data` reader — the piece `tiny_http`
/// does not ship.
///
/// It never buffers a whole file: bytes go from the socket to the sink
/// (`File`, via `BufWriter`) as they arrive, so a 2 GB video costs 8 KB of
/// memory. The one thing kept in memory is the unconsumed tail, which must be
/// at least `boundary.len() - 1` bytes because a boundary can straddle two
/// reads.
mod multipart {
    use std::io::{Read, Write};

    const CHUNK: usize = 8 * 1024;

    #[derive(Debug)]
    pub enum Error {
        Io(std::io::Error),
        Truncated,
        TooLarge,
        Malformed(&'static str),
    }

    impl std::fmt::Display for Error {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Error::Io(error) => write!(formatter, "leitura: {error}"),
                Error::Truncated => write!(formatter, "corpo multipart incompleto"),
                Error::TooLarge => write!(formatter, "arquivo maior que o limite"),
                Error::Malformed(what) => write!(formatter, "multipart inválido: {what}"),
            }
        }
    }

    impl From<std::io::Error> for Error {
        fn from(error: std::io::Error) -> Self {
            Error::Io(error)
        }
    }

    pub struct Part {
        pub name: String,
        /// `None` for a plain field, `Some` for a file part. The **extension** is
        /// what decides whether the media is supported, never the part's
        /// `Content-Type`: a phone sends `application/octet-stream` for a HEIC
        /// often enough that trusting it would reject real photos.
        pub filename: Option<String>,
    }

    pub struct Reader<R: Read> {
        reader: R,
        /// `--<boundary>`, the delimiter as it appears on the wire.
        marker: Vec<u8>,
        buffer: Vec<u8>,
        scratch: [u8; CHUNK],
        /// True once the previous part's body has been consumed, so the next
        /// `--boundary` is already behind us and only the `--`/`CRLF` suffix is
        /// left to read.
        after_body: bool,
        finished: bool,
        parts: usize,
    }

    impl<R: Read> Reader<R> {
        pub fn new(reader: R, boundary: &str) -> Self {
            let mut marker = Vec::with_capacity(boundary.len() + 2);
            marker.extend_from_slice(b"--");
            marker.extend_from_slice(boundary.as_bytes());
            Self {
                reader,
                marker,
                buffer: Vec::new(),
                scratch: [0; CHUNK],
                after_body: false,
                finished: false,
                parts: 0,
            }
        }

        /// The next part's headers, or `None` at the end of the body.
        pub fn next_part(&mut self) -> Result<Option<Part>, Error> {
            if self.finished {
                return Ok(None);
            }
            if self.after_body {
                self.after_body = false;
                if self.read_line_marker()? {
                    self.finished = true;
                    return Ok(None);
                }
            } else if !self.skip_to_marker()? {
                self.finished = true;
                return Ok(None);
            }

            let headers = self.read_headers()?;
            self.parts += 1;
            let disposition = header_value(&headers, "content-disposition");
            let Some(disposition) = disposition else {
                return Err(Error::Malformed("parte sem Content-Disposition"));
            };
            Ok(Some(Part {
                name: quoted(&disposition, "name").unwrap_or_default(),
                filename: quoted(&disposition, "filename"),
            }))
        }

        /// The current part's bytes, straight into `sink`.
        pub fn stream_body<W: Write>(&mut self, sink: &mut W, limit: u64) -> Result<u64, Error> {
            let mut written = 0u64;
            loop {
                if let Some(position) = find(&self.buffer, &self.marker) {
                    // The CRLF before a delimiter belongs to the delimiter, not
                    // to the body (RFC 2046), so it must not reach the file. The
                    // CRLF *after* the delimiter is left in the buffer for
                    // `read_line_marker` to consume.
                    let mut end = position;
                    if end >= 2 && &self.buffer[end - 2..end] == b"\r\n" {
                        end -= 2;
                    }
                    sink.write_all(&self.buffer[..end])?;
                    written += end as u64;
                    self.buffer.drain(..position + self.marker.len());
                    self.after_body = true;
                    return Ok(written);
                }

                // Hold back the CRLF that precedes a delimiter plus what
                // could still be the head of a boundary. The CRLF belongs to
                // the delimiter (RFC 2046), so it must never reach the file —
                // and with a 1-byte reader it would be flushed long before
                // the marker that follows it is seen.
                let keep = self.marker.len() + 1;
                if self.buffer.len() > keep {
                    let flush = self.buffer.len() - keep;
                    sink.write_all(&self.buffer[..flush])?;
                    written += flush as u64;
                    self.buffer.drain(..flush);
                }
                if written > limit {
                    return Err(Error::TooLarge);
                }

                let read = self.reader.read(&mut self.scratch)?;
                if read == 0 {
                    // EOF with a partial body: the tail is still the user's
                    // bytes, so they are written before the error. A trailing
                    // CRLF is the head of the delimiter that never arrived, so
                    // it is not the user's content.
                    let mut end = self.buffer.len();
                    if end >= 2 && &self.buffer[end - 2..end] == b"\r\n" {
                        end -= 2;
                    }
                    sink.write_all(&self.buffer[..end])?;
                    return Err(Error::Truncated);
                }
                self.buffer.extend_from_slice(&self.scratch[..read]);
            }
        }

        /// The current part's bytes as text, for the short form fields.
        pub fn text_body(&mut self, limit: usize) -> Result<String, Error> {
            let mut raw = Vec::new();
            self.stream_body(&mut raw, limit as u64)?;
            Ok(String::from_utf8_lossy(&raw).into_owned())
        }

        /// Finds the next `--<boundary>`, discarding the preamble. False at EOF.
        fn skip_to_marker(&mut self) -> Result<bool, Error> {
            loop {
                if let Some(position) = find(&self.buffer, &self.marker) {
                    self.buffer.drain(..position + self.marker.len());
                    return Ok(true);
                }
                // The preamble is not a file: only enough to hold a split marker
                // needs to survive the flush.
                let keep = self.marker.len().saturating_sub(1);
                if self.buffer.len() > keep {
                    let flush = self.buffer.len() - keep;
                    self.buffer.drain(..flush);
                }
                let read = self.reader.read(&mut self.scratch)?;
                if read == 0 {
                    return Ok(false);
                }
                self.buffer.extend_from_slice(&self.scratch[..read]);
            }
        }

        /// After a body: the CRLF after the boundary line means another part
        /// follows, `--` ends the request. Returns true when the body is over.
        fn read_line_marker(&mut self) -> Result<bool, Error> {
            while self.buffer.len() < 2 {
                let read = self.reader.read(&mut self.scratch)?;
                if read == 0 {
                    return Err(Error::Truncated);
                }
                self.buffer.extend_from_slice(&self.scratch[..read]);
            }
            match &self.buffer[..2] {
                b"--" => {
                    self.buffer.drain(..2);
                    Ok(true)
                }
                b"\r\n" => {
                    self.buffer.drain(..2);
                    Ok(false)
                }
                _ => Err(Error::Malformed("delimitador inesperado")),
            }
        }

        fn read_headers(&mut self) -> Result<Vec<u8>, Error> {
            let end = [b"\r\n\r\n".as_slice()];
            loop {
                if let Some(position) = find_any(&self.buffer, &end) {
                    let headers = self.buffer[..position].to_vec();
                    self.buffer.drain(..position + 4);
                    return Ok(headers);
                }
                let read = self.reader.read(&mut self.scratch)?;
                if read == 0 {
                    return Err(Error::Truncated);
                }
                self.buffer.extend_from_slice(&self.scratch[..read]);
            }
        }
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        find_any(haystack, &[needle])
    }

    fn find_any(haystack: &[u8], needles: &[&[u8]]) -> Option<usize> {
        let mut best: Option<usize> = None;
        for needle in needles {
            if let Some(position) = haystack
                .windows(needle.len())
                .position(|window| window == *needle)
            {
                best = Some(best.map_or(position, |current: usize| current.min(position)));
            }
        }
        best
    }

    fn header_value(headers: &[u8], name: &str) -> Option<String> {
        let text = String::from_utf8_lossy(headers);
        for line in text.split("\r\n") {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            if key.trim().eq_ignore_ascii_case(name) {
                return Some(value.trim().to_string());
            }
        }
        None
    }

    /// `name="value"` out of a `Content-Disposition` header. Browsers escape a
    /// quote inside a filename as `\"`, which is undone here.
    fn quoted(header: &str, key: &str) -> Option<String> {
        let pattern = format!("{key}=\"");
        let start = header.find(&pattern)? + pattern.len();
        let rest = &header[start..];
        let mut value = String::new();
        let mut characters = rest.chars();
        while let Some(character) = characters.next() {
            match character {
                '\\' => value.push(characters.next().unwrap_or('\\')),
                '"' => return Some(value),
                other => value.push(other),
            }
        }
        Some(value)
    }
}

// ---------------------------------------------------------------------------
// The page the phone opens
// ---------------------------------------------------------------------------

/// Served verbatim; there is no template engine in the app and no third-party
/// JS on a phone page. pt-BR is the default (`/`) and English is `/en`; the
/// device field is prefilled from the user agent because §7.8's
/// `Media/<selected device>/` is chosen here, on the phone.
mod page {
    pub const PAGE_PT: &str = include_str!("wifi_page.html");

    pub const PAGE_EN: &str = include_str!("wifi_page_en.html");
}

#[cfg(test)]
mod tests;
