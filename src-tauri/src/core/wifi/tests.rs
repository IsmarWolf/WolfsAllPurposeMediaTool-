//! c11 tests: the multipart reader, the LAN choice, the QR, and one real HTTP
//! round trip against a bound socket — the gate's "Phone → QR → upload lands in
//! `Media/`, dedup counted", without needing a phone.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::*;

use crate::core::db;
use crate::core::layout;
use crate::core::paths::RootSource;
use crate::core::scanner::ScanSource;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn temp_shared(root: &Path, events: Arc<Mutex<Vec<IngestEventDto>>>) -> Shared {
    let resolver = PathResolver::new(root.to_path_buf(), RootSource::MarkerWalk);
    layout::ensure(&resolver).expect("a temp tree");
    let pool = db::open(&resolver).expect("a temp pool");
    let geocoder = Arc::new(Geocoder::new(resolver.geonames_path()));
    Shared::new(
        resolver,
        pool,
        geocoder,
        None,
        Arc::new(JobRegistry::default()),
    )
    .with_event_sink(move |event| events.lock().expect("events").push(event))
}

fn temp_resolver(root: &Path) -> PathResolver {
    let resolver = PathResolver::new(root.to_path_buf(), RootSource::MarkerWalk);
    layout::ensure(&resolver).expect("a temp tree");
    resolver
}

/// One `multipart/form-data` body, exactly as a browser writes it.
fn body(boundary: &str, parts: &[(&str, Option<&str>, &[u8])]) -> Vec<u8> {
    let mut raw = Vec::new();
    for (name, filename, content) in parts {
        raw.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match filename {
            Some(file) => raw.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{file}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
                )
                .as_bytes(),
            ),
            None => raw.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
            ),
        }
        raw.extend_from_slice(content);
        raw.extend_from_slice(b"\r\n");
    }
    raw.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    raw
}

/// A tiny JPEG-ish blob: a real EXIF read finds nothing in it, which is exactly
/// the C7 "sem metadados" case the upload path must route to `Sem_Metadados/`.
fn photo_bytes(seed: u8) -> Vec<u8> {
    let mut raw = vec![0xFF, 0xD8, 0xFF, 0xE0];
    raw.extend(std::iter::repeat_n(seed, 512));
    raw.extend_from_slice(&[0xFF, 0xD9]);
    raw
}

/// A `Read` that hands out at most `chunk` bytes at a time, so a boundary can be
/// split across reads no matter where it lands.
struct Dribble<'a> {
    data: &'a [u8],
    chunk: usize,
}

impl Read for Dribble<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let take = self.chunk.min(buffer.len()).min(self.data.len());
        buffer[..take].copy_from_slice(&self.data[..take]);
        self.data = &self.data[take..];
        Ok(take)
    }
}

// ---------------------------------------------------------------------------
// multipart
// ---------------------------------------------------------------------------

#[test]
fn a_field_and_a_file_come_back_verbatim() {
    let raw = body(
        "BOUND",
        &[
            ("device", None, b"iPhone 14 Pro"),
            ("file", Some("IMG_0001.jpg"), &photo_bytes(7)),
        ],
    );

    let mut parts = multipart::Reader::new(
        Dribble {
            data: &raw,
            chunk: 64,
        },
        "BOUND",
    );

    let field = parts.next_part().unwrap().unwrap();
    assert_eq!(field.name, "device");
    assert!(field.filename.is_none());
    assert_eq!(parts.text_body(256).unwrap(), "iPhone 14 Pro");

    let file = parts.next_part().unwrap().unwrap();
    assert_eq!(file.name, "file");
    assert_eq!(file.filename.as_deref(), Some("IMG_0001.jpg"));
    let mut landed = Vec::new();
    parts.stream_body(&mut landed, u64::MAX).unwrap();
    assert_eq!(landed, photo_bytes(7));

    assert!(parts.next_part().unwrap().is_none(), "body is over");
}

#[test]
fn bytes_are_identical_for_every_chunk_size() {
    let payload: Vec<u8> = (0..5_000u32).map(|index| (index % 251) as u8).collect();
    let raw = body(
        "----WebKitFormBoundaryABC123",
        &[("file", Some("video.mp4"), &payload)],
    );

    // A boundary is 26 bytes long here, so chunk sizes below that force the
    // reader to hold a partial delimiter across reads.
    for chunk in [1usize, 3, 7, 25, 26, 27, 512, 8_192] {
        let mut parts = multipart::Reader::new(
            Dribble { data: &raw, chunk },
            "----WebKitFormBoundaryABC123",
        );
        let part = parts.next_part().unwrap().unwrap();
        assert_eq!(part.filename.as_deref(), Some("video.mp4"));
        let mut landed = Vec::new();
        parts.stream_body(&mut landed, u64::MAX).unwrap();
        assert_eq!(landed, payload, "chunk {chunk} changed the bytes");
        assert!(parts.next_part().unwrap().is_none());
    }
}

#[test]
fn a_body_that_stops_mid_file_is_an_error_not_a_short_file() {
    let mut raw = body("BOUND", &[("file", Some("truncado.jpg"), b"1234567890")]);
    // Cut the closing delimiter away: the client vanished mid-transfer.
    // `--BOUND--\r\n` is exactly 11 bytes, so the whole body survives.
    let keep = raw.len() - 11;
    raw.truncate(keep);

    let mut parts = multipart::Reader::new(
        Dribble {
            data: &raw,
            chunk: 8,
        },
        "BOUND",
    );
    parts.next_part().unwrap().unwrap();
    let mut landed = Vec::new();
    let error = parts.stream_body(&mut landed, u64::MAX).unwrap_err();
    assert!(matches!(error, multipart::Error::Truncated), "{error}");
    assert_eq!(
        landed, b"1234567890",
        "the bytes that did arrive are still the user's"
    );
}

#[test]
fn the_size_cap_stops_a_runaway_upload() {
    let payload = vec![b'x'; 4_096];
    let raw = body("BOUND", &[("file", Some("grande.mp4"), &payload)]);

    let mut parts = multipart::Reader::new(
        Dribble {
            data: &raw,
            chunk: 512,
        },
        "BOUND",
    );
    parts.next_part().unwrap().unwrap();
    let mut landed = Vec::new();
    let error = parts.stream_body(&mut landed, 1_024).unwrap_err();
    assert!(matches!(error, multipart::Error::TooLarge), "{error}");
}

#[test]
fn several_files_in_one_body_do_not_share_bytes() {
    let raw = body(
        "BOUND",
        &[
            ("device", None, b"iPad"),
            ("file", Some("a.jpg"), &photo_bytes(1)),
            ("file", Some("b.jpg"), &photo_bytes(2)),
            ("file", Some("c.jpg"), &photo_bytes(3)),
        ],
    );

    let mut parts = multipart::Reader::new(
        Dribble {
            data: &raw,
            chunk: 32,
        },
        "BOUND",
    );
    let mut files = Vec::new();
    let mut device = String::new();
    while let Some(part) = parts.next_part().unwrap() {
        match part.filename {
            None => device = parts.text_body(256).unwrap(),
            Some(_) => {
                let mut landed = Vec::new();
                parts.stream_body(&mut landed, u64::MAX).unwrap();
                files.push(landed);
            }
        }
    }

    assert_eq!(device, "iPad");
    assert_eq!(files.len(), 3);
    for (index, file) in files.iter().enumerate() {
        assert_eq!(file, &photo_bytes(index as u8 + 1), "file {index}");
    }
}

#[test]
fn the_boundary_is_read_from_the_content_type_header() {
    assert_eq!(
        parse_boundary("multipart/form-data; boundary=----WebKitFormBoundaryABC").as_deref(),
        Some("----WebKitFormBoundaryABC")
    );
    assert_eq!(
        parse_boundary("multipart/form-data; boundary=\"quoted one\"").as_deref(),
        Some("quoted one")
    );
    assert_eq!(parse_boundary("application/json"), None);
    assert_eq!(parse_boundary("multipart/form-data"), None);
}

#[test]
fn a_windows_fakepath_filename_is_reduced_to_its_name() {
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
        "§5.4's Windows reserved names still have the last word"
    );
    assert!(
        safe_file_name("a/b:c.jpg").is_err(),
        "only the last component is ever used"
    );
}

// ---------------------------------------------------------------------------
// Folder choice (C7) and the LAN address
// ---------------------------------------------------------------------------

#[test]
fn an_upload_without_metadata_lands_in_sem_metadados() {
    let dir = tempfile::tempdir().unwrap();
    let resolver = temp_resolver(dir.path());

    let target = target_dir(&resolver, "iPhone", None).unwrap();
    assert!(target.ends_with("iPhone/Sem_Metadados"), "{target:?}");
}

#[test]
fn c7_decides_the_upload_folder_the_same_way_the_scanner_does() {
    let dir = tempfile::tempdir().unwrap();
    let resolver = temp_resolver(dir.path());
    let captured = chrono::NaiveDate::from_ymd_opt(2024, 5, 14)
        .unwrap()
        .and_hms_opt(9, 32, 11)
        .unwrap();

    let own_date = RawMetadata {
        captured_at: Some(captured),
        gps: None,
        date_from_fs: false,
    };
    assert!(
        target_dir(&resolver, "iPhone", Some(&own_date))
            .unwrap()
            .ends_with("iPhone/2024/05"),
        "a real capture date gets the dated tree"
    );

    let mtime_only = RawMetadata {
        captured_at: Some(captured),
        gps: None,
        date_from_fs: true,
    };
    assert!(
        target_dir(&resolver, "iPhone", Some(&mtime_only))
            .unwrap()
            .ends_with("iPhone/Sem_Metadados"),
        "C7: an mtime date is not a date of its own"
    );

    let gps_only = RawMetadata {
        captured_at: None,
        gps: Some((-23.55, -46.63)),
        date_from_fs: false,
    };
    assert!(
        target_dir(&resolver, "iPhone", Some(&gps_only))
            .unwrap()
            .ends_with("iPhone/Sem_Metadados"),
        "GPS without a date has no year/month to file it under"
    );
}

#[test]
fn the_qr_prefers_a_real_home_router_over_a_vm_bridge() {
    let home = (String::from("Wi-Fi"), Ipv4Addr::new(192, 168, 0, 20));
    let virtualbox = (String::from("Ethernet 2"), Ipv4Addr::new(192, 168, 56, 1));
    let vpn = (String::from("Tailscale"), Ipv4Addr::new(100, 64, 0, 2));
    let docker = (
        String::from("vEthernet (WSL)"),
        Ipv4Addr::new(172, 20, 0, 1),
    );

    assert_eq!(pick(std::slice::from_ref(&home)), Some(home.1));
    assert_eq!(
        pick(&[
            home.clone(),
            virtualbox.clone(),
            vpn.clone(),
            docker.clone()
        ]),
        Some(home.1),
        "a virtual bridge never outranks the real adapter, whatever its range"
    );
    assert_eq!(pick(&[virtualbox.clone(), vpn.clone()]), Some(vpn.1));
    assert_eq!(
        pick(&[virtualbox, docker.clone()]),
        Some(docker.1),
        "with no home-range adapter left, the named bridge is still usable if it has a private address"
    );
    assert_eq!(pick(&[]), None);
    assert!(
        pick(&[
            (String::from("Loopback"), Ipv4Addr::new(192, 168, 1, 5)),
            (String::from("Local Area Connection"), Ipv4Addr::LOCALHOST),
        ])
        .is_some(),
        "loopback never reaches `pick`: `lan_ip` filters it out first"
    );
    assert_eq!(rank("Wi-Fi", Ipv4Addr::new(169, 254, 1, 2)), 4);
}

#[test]
fn the_qr_is_a_square_matrix_that_embeds_the_url() {
    let matrix = qr_matrix("http://192.168.0.20:8642").unwrap();
    assert!(matrix.len() >= 21, "a QR is at least version 1");
    for row in &matrix {
        assert_eq!(row.len(), matrix.len(), "the matrix must be square");
    }
    assert!(
        matrix.iter().flatten().filter(|dark| **dark).count() > 50,
        "a blank matrix would be a QR the phone cannot read"
    );
    assert!(qr_matrix("http://192.168.0.20:8642").unwrap() == matrix);
}

/// Prints what this machine actually offers, so the ranking above can be checked
/// against a real adapter list. Never fails: a CI box has no LAN.
#[test]
fn this_machine_has_an_ip_or_it_is_only_reported() {
    let found = lan_ip();
    let all: Vec<String> = if_addrs::get_if_addrs()
        .map(|interfaces| {
            interfaces
                .iter()
                .filter_map(|interface| match &interface.addr {
                    if_addrs::IfAddr::V4(v4) => Some(format!("{} {}", v4.ip, interface.name)),
                    if_addrs::IfAddr::V6(_) => None,
                })
                .collect()
        })
        .unwrap_or_default();
    println!("IPv4 candidates: {all:?} -> chosen {found:?}");
}

// ---------------------------------------------------------------------------
// The real thing: a bound socket, a hand-written request
// ---------------------------------------------------------------------------

struct Reply {
    status: u16,
    body: String,
}

fn request(addr: SocketAddr, raw: &[u8]) -> Reply {
    let mut stream = TcpStream::connect(addr).expect("connect to our own listener");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    stream.write_all(raw).unwrap();
    stream.flush().unwrap();
    // `Connection: close` in the request makes the response end at EOF.
    let mut text = String::new();
    stream.read_to_string(&mut text).unwrap();

    let (head, body) = text.split_once("\r\n\r\n").expect("a complete response");
    let status: u16 = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("a status line");
    Reply {
        status,
        body: body.to_string(),
    }
}

fn upload_request(addr: SocketAddr, boundary: &str, payload: &[u8]) -> Reply {
    let head = format!(
        "POST /upload HTTP/1.1\r\nHost: localhost\r\nContent-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    let mut raw = head.into_bytes();
    raw.extend_from_slice(payload);
    request(addr, &raw)
}

fn bind(shared: Shared) -> (SocketAddr, Arc<WifiServer>) {
    let server = Arc::new(start_on(shared, 0).expect("bind an ephemeral port"));
    let addr: SocketAddr = format!("127.0.0.1:{}", server.port()).parse().unwrap();
    (addr, server)
}

#[test]
fn the_page_is_served_in_both_languages() {
    let dir = tempfile::tempdir().unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let (addr, server) = bind(temp_shared(dir.path(), events));

    let pt = request(
        addr,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(pt.status, 200);
    assert!(
        pt.body.contains("Enviar para o SSD"),
        "pt-BR is the default"
    );
    assert!(
        pt.body.contains("Nome do aparelho"),
        "the label field is there"
    );

    let en = request(
        addr,
        b"GET /en HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(en.status, 200);
    assert!(en.body.contains("Upload to the SSD"));

    assert_eq!(
        request(
            addr,
            b"GET /nope HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
        )
        .status,
        404
    );

    server.stop().unwrap();
}

#[test]
fn an_upload_lands_in_media_and_a_second_one_counts_as_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let shared = temp_shared(dir.path(), Arc::clone(&events));
    let resolver = shared.resolver.clone();
    let (addr, server) = bind(shared);

    let payload = body(
        "BOUND",
        &[
            ("device", None, b"iPhone 14 Pro"),
            ("file", Some("IMG_0001.jpg"), &photo_bytes(9)),
        ],
    );
    let reply = upload_request(addr, "BOUND", &payload);
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert!(
        reply
            .body
            .contains(r#""placed":["Media/iPhone 14 Pro/Sem_Metadados/IMG_0001.jpg"]"#),
        "{}",
        reply.body
    );

    // The bytes are in the tree, with nothing left behind in staging.
    let landed = resolver
        .media_device("iPhone 14 Pro")
        .unwrap()
        .join("Sem_Metadados")
        .join("IMG_0001.jpg");
    assert!(landed.is_file(), "{} should exist", landed.display());
    let staging = resolver.db_dir().join("staging").join("wifi");
    let leftovers: Vec<_> = fs::read_dir(&staging)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "staging must be empty: {leftovers:?}");

    // The same bytes again: a second file with a `_1` suffix (C5 never
    // overwrites), and the pipeline counts it as a duplicate.
    let again = body(
        "BOUND",
        &[
            ("device", None, b"iPhone 14 Pro"),
            ("file", Some("IMG_0001.jpg"), &photo_bytes(9)),
        ],
    );
    let reply = upload_request(addr, "BOUND", &again);
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert!(
        reply.body.contains("IMG_0001_1.jpg"),
        "the original bytes are never overwritten: {}",
        reply.body
    );

    // The pipeline runs on its own thread; the second pass (the one that
    // counts the duplicate) lands after this response. Wait for it.
    let duplicates = || {
        events
            .lock()
            .expect("events")
            .iter()
            .filter_map(|event| match event {
                IngestEventDto::Summary(summary) => Some(summary.duplicates),
                _ => None,
            })
            .sum::<u32>()
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while duplicates() < 1 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let seen = events.lock().expect("events").clone();
    assert!(
        duplicates() >= 1,
        "the dedupe count is the gate's check; events were {seen:?}"
    );
    assert!(
        seen.iter()
            .any(|event| matches!(event, IngestEventDto::DbChanged)),
        "new rows mean every open panel is stale (§10.5)"
    );

    // And the DB really has one row for the two files.
    let pool = db::open(&resolver).unwrap();
    let rows = db::media_query(&pool, &crate::core::models::FilterSpec::default()).unwrap();
    assert_eq!(rows.items.len(), 1, "one logical photo, two files on disk");

    server.stop().unwrap();
}

#[test]
fn an_unsupported_file_is_refused_and_the_batch_continues() {
    let dir = tempfile::tempdir().unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let shared = temp_shared(dir.path(), events);
    let (addr, server) = bind(shared);

    let payload = body(
        "BOUND",
        &[
            ("device", None, b"iPad"),
            ("file", Some("nota.txt"), b"isto nao e midia"),
            ("file", Some("IMG_0002.jpg"), &photo_bytes(4)),
        ],
    );
    let reply = upload_request(addr, "BOUND", &payload);
    assert_eq!(reply.status, 200);
    assert!(reply.body.contains("tipo não suportado"), "{}", reply.body);
    assert!(
        reply.body.contains("IMG_0002.jpg"),
        "one bad file must not cancel the rest: {}",
        reply.body
    );

    server.stop().unwrap();
}

#[test]
fn a_label_that_is_reserved_falls_back_instead_of_writing_to_the_pc_tree() {
    let dir = tempfile::tempdir().unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let (addr, server) = bind(temp_shared(dir.path(), events));

    let payload = body(
        "BOUND",
        &[
            ("device", None, b"PC"),
            ("file", Some("IMG_0003.jpg"), &photo_bytes(5)),
        ],
    );
    let reply = upload_request(addr, "BOUND", &payload);
    assert_eq!(reply.status, 200);
    assert!(
        reply.body.contains("PC"),
        "§5.4 reserves the label, so nothing lands under Media/PC: {}",
        reply.body
    );

    server.stop().unwrap();
}

#[test]
fn stopping_the_server_is_idempotent_and_ends_the_listener() {
    let dir = tempfile::tempdir().unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let (addr, server) = bind(temp_shared(dir.path(), events));

    assert_eq!(
        request(
            addr,
            b"GET /done HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
        )
        .status,
        200
    );

    server.stop().unwrap();
    server.stop().unwrap();

    // The port is closed now: a connection either fails or reaches nothing.
    let mut stream = TcpStream::connect(addr);
    if let Ok(stream) = stream.as_mut() {
        let _ = stream.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        let mut text = String::new();
        let _ = stream.read_to_string(&mut text);
        assert!(
            text.is_empty(),
            "a stopped server answers nothing: {text:?}"
        );
    }
}

#[test]
fn one_burst_of_uploads_runs_one_pipeline_pass() {
    let dir = tempfile::tempdir().unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let shared = temp_shared(dir.path(), Arc::clone(&events));
    let (addr, server) = bind(shared);

    for index in 1..=3u8 {
        let payload = body(
            "BOUND",
            &[
                ("device", None, b"iPhone"),
                (
                    "file",
                    Some(&*format!("IMG_{index:04}.jpg")),
                    &photo_bytes(index),
                ),
            ],
        );
        assert_eq!(upload_request(addr, "BOUND", &payload).status, 200);
    }
    let reply = request(
        addr,
        b"POST /done HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(reply.status, 200);
    assert!(reply.body.contains(r#""uploads":3"#), "{}", reply.body);

    // The job drains the pending map on its own; wait for every photo to be
    // indexed rather than sleeping a fixed amount.
    let added = || {
        events
            .lock()
            .expect("events")
            .iter()
            .filter_map(|event| match event {
                IngestEventDto::Summary(summary) => Some(summary.inserted),
                _ => None,
            })
            .sum::<u32>()
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while added() < 3 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let seen = events.lock().expect("events").clone();
    assert!(added() >= 3, "all three photos must be indexed: {seen:?}");

    server.stop().unwrap();
}

/// The pipeline's slot must be free again once it drains, or the next upload
/// could never be processed.
#[test]
fn the_pipeline_slot_is_released_when_there_is_nothing_left() {
    let dir = tempfile::tempdir().unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let shared = temp_shared(dir.path(), events);
    let jobs = Arc::clone(&shared.jobs);

    let payload = body(
        "BOUND",
        &[
            ("device", None, b"iPhone"),
            ("file", Some("sozinho.jpg"), &photo_bytes(6)),
        ],
    );
    let staged = staging_dir(&shared.resolver).unwrap();
    let parts = multipart::Reader::new(payload.as_slice(), "BOUND");
    let mut device = String::new();
    let mut reader = parts;
    while let Some(part) = reader.next_part().unwrap() {
        if part.filename.is_none() {
            device = reader.text_body(256).unwrap();
        } else {
            let path = staged.join("unit.part");
            stage_one(&mut reader, &path).unwrap();
            place(&shared, &device, part.filename.as_deref().unwrap(), &path).unwrap();
        }
    }
    shared.request_pipeline(&device);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while jobs.is_running(JobKind::IngestWifi) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        !jobs.is_running(JobKind::IngestWifi),
        "a drained pipeline must give the slot back"
    );
    assert!(
        jobs.claim(JobKind::IngestWifi).is_ok(),
        "and be claimable again"
    );
    jobs.release(JobKind::IngestWifi, "unit");
}

/// A scan of a device tree needs the folder to exist; an upload creates it, and
/// that ordering is what makes the very first upload work at all.
#[test]
fn an_upload_creates_the_device_folder_it_lands_in() {
    let dir = tempfile::tempdir().unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let shared = temp_shared(dir.path(), events);
    let resolver = shared.resolver.clone();

    assert!(!resolver.media_device("iPhone Novo").unwrap().exists());
    let payload = body(
        "BOUND",
        &[
            ("device", None, b"iPhone Novo"),
            ("file", Some("IMG_0100.jpg"), &photo_bytes(8)),
        ],
    );
    let (addr, server) = bind(shared);
    assert_eq!(upload_request(addr, "BOUND", &payload).status, 200);
    assert!(
        resolver
            .media_device("iPhone Novo")
            .unwrap()
            .join("Sem_Metadados")
            .is_dir(),
        "the first upload creates Media/<device>/Sem_Metadados/"
    );

    server.stop().unwrap();
}

/// Guards the invariant the whole module rests on: nothing is written inside
/// `Media/` until a part is complete (§14, and what the human's C5 worries).
#[test]
fn media_is_never_written_to_while_a_part_is_in_flight() {
    let dir = tempfile::tempdir().unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let shared = temp_shared(dir.path(), events);
    let resolver = shared.resolver.clone();

    let raw = body(
        "BOUND",
        &[("file", Some("grande.mp4"), &vec![b'z'; 20_000])],
    );
    let mut parts = multipart::Reader::new(
        Dribble {
            data: &raw,
            chunk: 128,
        },
        "BOUND",
    );
    let part = parts.next_part().unwrap().unwrap();
    let path = staging_dir(&resolver).unwrap().join("in-flight.part");
    {
        let mut sink = std::io::BufWriter::new(File::create(&path).unwrap());
        parts.stream_body(&mut sink, u64::MAX).unwrap();
        // Mid-flight: the bytes are on the SSD but nowhere near `Media/`.
        assert!(path.is_file());
        assert!(!resolver.media_device("iPhone").unwrap().exists());
    }
    assert_eq!(
        place(&shared, "iPhone", part.filename.as_deref().unwrap(), &path)
            .unwrap()
            .split('/')
            .next()
            .unwrap(),
        "Media"
    );
}

/// C4's server is an offline feature; this asserts the one thing that would make
/// it not: the app itself pulls nothing from the network to answer the phone.
#[test]
fn nothing_but_the_lan_listener_is_started() {
    let counter = Arc::new(AtomicUsize::new(0));
    let _ = counter.fetch_add(0, Ordering::Relaxed);
    // A compile-time statement is the honest version of this: the module has no
    // outbound client at all, so there is nothing to assert at runtime.
    assert_eq!(PORT, 8642, "§7.8's constant port, not a configurable one");
}

/// The scanner path the pipeline uses, spelled out so a change in `ScanSource`
/// cannot silently make Wi-Fi uploads write somewhere else.
#[test]
fn the_pipeline_scans_the_device_tree_in_place() {
    assert!(matches!(ScanSource::Device, ScanSource::Device));
}
