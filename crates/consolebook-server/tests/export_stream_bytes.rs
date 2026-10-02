//! The streamed container is byte-identical to the buffered one (#47).
#![allow(clippy::cast_possible_truncation)]
//!
//! The streaming sink holds each entry — its local header and its payload —
//! and applies the writer's CRC-32 and size patch to those held bytes
//! before releasing the entry. If that bookkeeping were wrong the two
//! archives would differ — in the local header, in a data descriptor, or in
//! the central directory — so the comparison is the contract.

use std::io::Write;

use consolebook_server::export_stream::EntryBuffer;

fn options() -> zip::write::SimpleFileOptions {
    zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .last_modified_time(
            zip::DateTime::from_date_and_time(2026, 9, 1, 19, 0, 0).expect("valid instant"),
        )
        .unix_permissions(0o644)
}

fn buffered(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        writer.start_file(name.clone(), options()).expect("start");
        writer.write_all(bytes).expect("write");
    }
    writer.finish().expect("finish").into_inner()
}

fn streamed(entries: &[(String, Vec<u8>)]) -> (Vec<u8>, usize) {
    let sink = EntryBuffer::new(std::io::Cursor::new(Vec::new()));
    let mut writer = zip::ZipWriter::new(sink);
    for (name, bytes) in entries {
        writer.start_file(name.clone(), options()).expect("start");
        writer.write_all(bytes).expect("write");
    }
    let sink = writer.finish().expect("finish");
    let peak = sink.peak_entry_bytes();
    let bytes = sink.into_inner().expect("release").into_inner();
    (bytes, peak)
}

fn entries_of(sizes: &[usize]) -> Vec<(String, Vec<u8>)> {
    sizes
        .iter()
        .enumerate()
        .map(|(index, size)| {
            (
                format!("records/{index}/v1/record.json"),
                (0..*size).map(|at| (at % 251) as u8).collect(),
            )
        })
        .collect()
}

#[test]
fn a_streamed_archive_is_byte_identical_to_a_buffered_one() {
    // Sizes chosen to cross the local-header boundary, a write boundary,
    // and a realistic record.
    for sizes in [
        vec![0],
        vec![1],
        vec![13],
        vec![14],
        vec![15],
        vec![1024],
        vec![0, 1, 13, 14, 15, 4096],
        vec![65_536, 3, 300_000],
    ] {
        let entries = entries_of(&sizes);
        let reference = buffered(&entries);
        let (produced, peak) = streamed(&entries);
        assert_eq!(
            produced, reference,
            "sizes {sizes:?} produced different container bytes"
        );
        // The sink really held an entry (so the patches had somewhere to
        // land) and held no more than the largest one plus its header.
        let largest = sizes.iter().copied().max().expect("a size");
        assert!(
            peak >= largest,
            "sizes {sizes:?} retained only {peak} bytes, less than the largest entry"
        );
        assert!(
            peak <= largest + 1024,
            "sizes {sizes:?} retained {peak} bytes for a {largest}-byte entry"
        );
    }
}

#[test]
fn a_streamed_archive_carries_no_data_descriptors() {
    let entries = entries_of(&[2048, 7, 65_536]);
    let (produced, _) = streamed(&entries);
    let reference = buffered(&entries);
    // Every local header carries flag bit 3 only when the writer had to
    // defer CRC-32 and sizes to a trailing descriptor, and walking the
    // archive by its own size fields only works when they were patched.
    let mut at = 0usize;
    let mut local_headers = 0;
    while at + 30 <= produced.len() && produced[at..at + 4] == [0x50, 0x4b, 0x03, 0x04] {
        let flags = u16::from_le_bytes([produced[at + 6], produced[at + 7]]);
        assert_eq!(flags & 0x0008, 0, "local header at {at} uses a descriptor");
        let crc = u32::from_le_bytes([
            produced[at + 14],
            produced[at + 15],
            produced[at + 16],
            produced[at + 17],
        ]);
        let size = u32::from_le_bytes([
            produced[at + 18],
            produced[at + 19],
            produced[at + 20],
            produced[at + 21],
        ]);
        let uncompressed = u32::from_le_bytes([
            produced[at + 22],
            produced[at + 23],
            produced[at + 24],
            produced[at + 25],
        ]);
        assert_ne!(crc, 0, "local header at {at} was never patched");
        assert_eq!(
            size as usize,
            entries[local_headers].1.len(),
            "local header at {at} carries the wrong size"
        );
        assert_eq!(uncompressed, size, "stored entry at {at} disagrees on size");
        // The whole header, patch included, is the buffered writer's.
        assert_eq!(
            &produced[at..at + 30],
            &reference[at..at + 30],
            "local header at {at} differs from the buffered archive"
        );
        let name_len = usize::from(u16::from_le_bytes([produced[at + 26], produced[at + 27]]));
        let extra_len = usize::from(u16::from_le_bytes([produced[at + 28], produced[at + 29]]));
        at += 30 + name_len + extra_len + size as usize;
        local_headers += 1;
    }
    assert_eq!(local_headers, entries.len(), "not every entry was walked");
    assert_eq!(produced, reference);
}

// ---------------------------------------------------------------- #47 proof
//
// The streamed export's memory is bounded by its entry metadata, one
// record, and the container's directory rather than by the corpus. The
// corpora below are far larger than the bound, the sink discards what it
// receives, and the peak is read from a process that holds nothing else.

use std::io::Seek;

use consolebook_server::capabilities::RoleBundle;
use consolebook_server::data_dir::DataDir;
use consolebook_server::programs::{
    self, AnchorDef, CompetencyDef, FormCompetencyDef, FormDef, NarrativeDef, PolicyDef,
    RecordType, ScaleDef, ScaleKind, TaskDef, VersionContent,
};
use consolebook_server::record_export::{self, Scope};
use consolebook_server::{assignments, enrollments, setup, storage};
use sqlx::SqlitePool;

const PAYLOAD: usize = 16 * 1024;
const EXPORTED_AT: i64 = 1_788_289_200;

fn content() -> VersionContent {
    VersionContent {
        name: "Example County CTO Program".to_owned(),
        label: "2026 rev A".to_owned(),
        description: "Invented program for the streaming proof.".to_owned(),
        phases: Vec::new(),
        phase_transitions: Vec::new(),
        competencies: vec![CompetencyDef {
            category: "Call processing".to_owned(),
            name: "Emergency Call Interrogation".to_owned(),
            description: "Obtains and verifies location, callback, and nature.".to_owned(),
            tasks: vec![TaskDef {
                prompt: "Processes an invented structure-fire call.".to_owned(),
                citations: Vec::new(),
            }],
            citations: Vec::new(),
        }],
        rating_scales: vec![ScaleDef {
            name: "Standard 1-7".to_owned(),
            kind: ScaleKind::AnchoredNumeric,
            min_value: Some(1),
            max_value: Some(7),
            anchors: vec![AnchorDef {
                value: 4,
                label: "Meets standards".to_owned(),
                definition: "To the invented standard.".to_owned(),
            }],
        }],
        rating_modifiers: Vec::new(),
        evaluation_forms: vec![FormDef {
            record_type: RecordType::DailyReport,
            name: "Daily Observation Report".to_owned(),
            instructions: "Rate observed performance.".to_owned(),
            competencies: vec![FormCompetencyDef {
                competency: "Emergency Call Interrogation".to_owned(),
                rating_scale: "Standard 1-7".to_owned(),
            }],
            narratives: vec![NarrativeDef {
                prompt: "Most acceptable performance.".to_owned(),
                required: false,
            }],
        }],
        citations: Vec::new(),
        finalization_policy: PolicyDef {
            review_approved: false,
            required_narratives: false,
            ratings_complete: false,
        },
    }
}

/// The peak resident set this process has reached, from its own status.
/// `None` where the platform does not report one, so the measurement is
/// skipped rather than failing the suite.
fn peak_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse().ok())
}

/// Runs `measure` in a fresh process against the database the fixture just
/// wrote, so the peak it reports belongs to the export rather than to the
/// corpus the fixture allocated. Returns `(units, archive_bytes,
/// peak_growth_kib)`.
fn measure_in_fresh_process(database: &std::path::Path, actor: i64) -> (i64, u64, u64) {
    let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--ignored",
            "--exact",
            "export_memory_probe_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(PROBE_DB, database)
        .env(PROBE_ACTOR, actor.to_string())
        .output()
        .expect("run the probe");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "the probe failed: {text}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // The child is a test binary too, so the measurement shares its line
    // with the harness's own report of the test.
    let line = text
        .lines()
        .find_map(|line| line.find("probe units=").map(|at| &line[at..]))
        .unwrap_or_else(|| panic!("no measurement in {text}"));
    let field = |name: &str| -> u64 {
        line.split_whitespace()
            .find_map(|part| part.strip_prefix(name))
            .unwrap_or_else(|| panic!("{name} missing from {line:?}"))
            .parse()
            .expect("a number")
    };
    (
        field("units=").cast_signed(),
        field("archive_bytes="),
        field("peak_growth_kib="),
    )
}

const PROBE_DB: &str = "CONSOLEBOOK_EXPORT_PROBE_DB";
const PROBE_ACTOR: &str = "CONSOLEBOOK_EXPORT_PROBE_ACTOR";

/// The measured side of the memory proof: one installation export, in a
/// process that holds nothing but the database.
#[test]
#[ignore = "spawned by the memory proof with a seeded database"]
fn export_memory_probe_child() {
    let Some(database) = std::env::var_os(PROBE_DB) else {
        return;
    };
    let actor: i64 = std::env::var(PROBE_ACTOR)
        .expect("the probe actor")
        .parse()
        .expect("a number");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async move {
        let pool = storage::open(std::path::Path::new(&database))
            .await
            .expect("open");
        let Some(before) = peak_kib() else {
            println!("probe unavailable");
            return;
        };
        let mut sink = Discard {
            bytes: 0,
            position: 0,
        };
        let (produced, returned) = record_export::export_to(
            &pool,
            actor,
            Scope::Installation,
            EXPORTED_AT,
            &mut sink,
            |_| {},
        )
        .await
        .expect("call");
        let produced = produced.expect("exported");
        let after = peak_kib().expect("a reported peak");
        println!(
            "probe units={} archive_bytes={} peak_growth_kib={}",
            produced.unit_count,
            returned.bytes,
            after.saturating_sub(before)
        );
        pool.close().await;
    });
}

/// A scratch installation with a published program, an enrolled trainee,
/// an assigned trainer, one daily record, and `count` finalized versions
/// of `size` bytes each, exported in a fresh process.
async fn measured(count: i64, size: usize) -> Option<(i64, u64, u64)> {
    if peak_kib().is_none() {
        eprintln!("this platform reports no process peak: the memory bound is skipped");
        return None;
    }
    let (tmp, pool, admin_id, record_id) = installed().await;
    seed_versions(&pool, admin_id, record_id, count, size).await;
    let database = pool.connect_options().get_filename().to_owned();
    pool.close().await;
    let measured = measure_in_fresh_process(std::path::Path::new(&database), admin_id);
    drop(tmp);
    Some(measured)
}

/// The installation export's memory is bounded by the entry metadata, one
/// record, and the container's directory — never by the corpus — and every
/// byte reaches the sink. The three exports below separate the two terms:
/// `small` and `heavy` share a unit count and differ fourfold in payload,
/// and `wide` shares `small`'s payload and differs fourfold in units.
#[tokio::test(flavor = "multi_thread")]
async fn an_installation_export_is_bounded_by_its_entries_not_by_the_corpus() {
    let small = measured(500, PAYLOAD).await.expect("a measured peak");
    let heavy = measured(500, 4 * PAYLOAD).await.expect("a measured peak");
    let wide = measured(4_000, PAYLOAD).await.expect("a measured peak");
    eprintln!("small: {small:?}\nheavy: {heavy:?}\nwide: {wide:?}");
    // The measurement is real, and the corpora really differ.
    assert_eq!(small.0, 500);
    assert_eq!(heavy.0, 500);
    assert_eq!(wide.0, 4_000);
    assert!(
        heavy.1 > 3 * small.1,
        "the heavy corpus is only {} bytes against {}",
        heavy.1,
        small.1
    );
    assert!(
        wide.1 > 3 * small.1,
        "the wide corpus is only {} bytes against {}",
        wide.1,
        small.1
    );
    // An absolute bound: buffering even the smallest corpus would exceed
    // it, and the sink discards what it receives.
    assert!(
        small.2 < 16 * 1024,
        "the export grew by {} KiB for a {} byte archive",
        small.2,
        small.1
    );
    // Four times the payload at the same unit count: memory does not
    // follow the corpus.
    assert!(
        heavy.2 < small.2 + 4 * 1024,
        "four times the payload grew the peak from {} KiB to {} KiB",
        small.2,
        heavy.2
    );
    // Eight times the units: the entries' metadata, their JSON manifest,
    // and the container's directory grow — a per-unit constant of a few
    // kilobytes, which ADR 0014 names as the format's one linear term —
    // while the corpus, which is eight times larger again, does not.
    assert!(
        (wide.2 - small.2) * 1024 < 4 * 1024 * (wide.0 - small.0).cast_unsigned(),
        "the peak grew by {} KiB for {} more units",
        wide.2 - small.2,
        wide.0 - small.0
    );
    assert!(
        wide.2 * 4 < wide.1 / 1024,
        "the export held {} KiB of a {} byte archive",
        wide.2,
        wide.1
    );
}

/// A scratch installation with a published program, an enrolled trainee, an
/// assigned trainer, and one daily record. Returns the pool and the ids the
/// corpus builders need.
async fn installed() -> (tempfile::TempDir, SqlitePool, i64, i64) {
    let tmp = tempfile::tempdir().expect("temp dir");
    let data_dir = DataDir::new(tmp.path().join("data"));
    data_dir.ensure_layout().expect("layout");
    let pool = storage::open(&data_dir.database()).await.expect("open");
    let code = setup::issue_setup_code(&pool)
        .await
        .expect("issue")
        .expect("uninitialized")
        .0;
    let admin_id = setup::initialize(
        &pool,
        &code.raw,
        "Example County Communications",
        "avery.admin",
        "Avery Admin",
        "invented-passphrase-1",
    )
    .await
    .expect("initialize")
    .expect("accepted");
    let program_id = programs::create_program(&pool, admin_id, "Example County CTO Program")
        .await
        .expect("create")
        .expect("accepted");
    let version_id = programs::create_version(&pool, admin_id, program_id, &content())
        .await
        .expect("create")
        .expect("accepted");
    programs::publish_version(&pool, admin_id, version_id)
        .await
        .expect("publish")
        .expect("accepted");
    let trainee = consolebook_server::users::create_with_reset_code(
        &pool,
        admin_id,
        "taylor.trainee",
        "Taylor Trainee",
        "",
        "",
        RoleBundle::Trainee,
    )
    .await
    .expect("create")
    .expect("accepted");
    let author = consolebook_server::users::create_with_reset_code(
        &pool,
        admin_id,
        "jordan.trainer",
        "Jordan Trainer",
        "",
        "",
        RoleBundle::Trainer,
    )
    .await
    .expect("create")
    .expect("accepted");
    let enrollment_id = enrollments::enroll(&pool, admin_id, version_id, trainee.id)
        .await
        .expect("enroll")
        .expect("enrolled");
    assignments::create(&pool, admin_id, enrollment_id, author.id)
        .await
        .expect("assign")
        .expect("assigned");
    let record_id: i64 = sqlx::query_scalar(
        "INSERT INTO evaluation_record
             (enrollment_id, program_version_id, evaluation_form_id, owner_user_id,
              revision, created_at, created_by)
         SELECT ?1, ?2, f.id, ?3, 0, ?4, ?3
         FROM evaluation_form f
         WHERE f.program_version_id = ?2 AND f.record_type = 'daily_report'
         RETURNING id",
    )
    .bind(enrollment_id)
    .bind(version_id)
    .bind(author.id)
    .bind(1_788_289_200_i64)
    .fetch_one(&pool)
    .await
    .expect("insert record");
    (tmp, pool, admin_id, record_id)
}

/// A corpus of `count` chained finalized versions, each `size` bytes.
async fn seed_versions(pool: &SqlitePool, admin_id: i64, record_id: i64, count: i64, size: usize) {
    let payload = vec![b'x'; size];
    let mut tx = storage::write_tx(pool).await.expect("write tx");
    let mut predecessor: Option<i64> = None;
    for number in 1..=count {
        if number > 1 {
            sqlx::query(
                "INSERT INTO amendment
                     (evaluation_record_id, predecessor_version_id, reason,
                      opened_by, opened_by_display_name, opened_at,
                      opened_after_event_id, opened_after_decision_id)
                 VALUES (?1, ?2, 'Invented correction.', ?3, 'Avery Admin', ?4, 0, 0)",
            )
            .bind(record_id)
            .bind(predecessor)
            .bind(admin_id)
            .bind(1_788_289_200_i64)
            .execute(&mut *tx)
            .await
            .expect("insert amendment");
        }
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO evaluation_version
                 (evaluation_record_id, version_number, record_schema, canonical_bytes,
                  content_hash, chain_hash, predecessor_id, finalized_by, finalized_at)
             VALUES (?1, ?2, 2, ?3, ?4, ?4, ?5, ?6, ?7)
             RETURNING id",
        )
        .bind(record_id)
        .bind(number)
        .bind(&payload)
        .bind("a".repeat(64))
        .bind(predecessor)
        .bind(admin_id)
        .bind(1_788_289_200_i64)
        .fetch_one(&mut *tx)
        .await
        .expect("insert version");
        predecessor = Some(id);
    }
    tx.commit().await.expect("commit");
}

/// A sink that holds nothing and counts what it is given. Seeking is
/// accepted because the container writer patches each entry's local
/// header; a sink that keeps no bytes has nothing to rewrite.
struct Discard {
    bytes: u64,
    position: u64,
}

impl Write for Discard {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.position == self.bytes {
            self.bytes += buf.len() as u64;
        }
        self.position += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Seek for Discard {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.position = match pos {
            std::io::SeekFrom::Start(at) => at,
            std::io::SeekFrom::End(0) => self.bytes,
            std::io::SeekFrom::Current(0) => self.position,
            other => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    format!("discarding sink ({other:?})"),
                ));
            }
        };
        Ok(self.position)
    }
}

/// A destination that holds the first entry it is given until the test
/// releases it, so an export can be observed while it is mid-stream.
struct Gate {
    position: u64,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
    held: bool,
}

impl Write for Gate {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if !self.held {
            self.held = true;
            let _ = self.entered.send(());
            let _ = self
                .release
                .recv_timeout(std::time::Duration::from_secs(60));
        }
        self.position += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Seek for Gate {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.position = match pos {
            std::io::SeekFrom::Start(at) => at,
            std::io::SeekFrom::End(0) | std::io::SeekFrom::Current(0) => self.position,
            other => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    format!("gated sink ({other:?})"),
                ));
            }
        };
        Ok(self.position)
    }
}

/// An export that is mid-stream reserves no writer: the installation's
/// other writers keep working while a download is in flight, because the
/// export holds a read snapshot and its audit is its own committed
/// statement rather than part of that transaction (ADR 0019; #47).
#[tokio::test(flavor = "multi_thread")]
async fn a_streaming_export_holds_no_write_reservation() {
    let (_tmp, pool, admin_id, record_id) = installed().await;
    seed_versions(&pool, admin_id, record_id, 20, PAYLOAD).await;

    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let export_pool = pool.clone();
    let exporting = tokio::task::spawn_blocking(move || {
        let sink = Gate {
            position: 0,
            entered: entered_tx,
            release: release_rx,
            held: false,
        };
        tokio::runtime::Handle::current().block_on(async move {
            record_export::export_to(
                &export_pool,
                admin_id,
                Scope::Installation,
                EXPORTED_AT,
                sink,
                |_| {},
            )
            .await
        })
    });
    // The export is now past its audit and mid-payload.
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the export reached its first entry");

    // A writer takes the installation's write reservation and commits.
    let started = std::time::Instant::now();
    consolebook_server::audit::record(
        &pool,
        consolebook_server::audit::EventKind::RecordExported,
        Some(admin_id),
        None,
    )
    .await
    .expect("a write while an export streams");
    let waited = started.elapsed();
    assert!(
        waited < std::time::Duration::from_secs(1),
        "the writer waited {waited:?} for an export that holds no reservation"
    );

    release_tx.send(()).expect("release the export");
    let (produced, _sink) = exporting.await.expect("join").expect("call");
    assert!(produced.is_ok(), "{produced:?}");
    pool.close().await;
}

/// A pool over the seeded database with exactly `connections` connections,
/// so the acquisition boundary is the one under test.
async fn pool_of(database: &std::path::Path, connections: u32) -> SqlitePool {
    sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(connections)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(database)
                .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
                .foreign_keys(true)
                .busy_timeout(std::time::Duration::from_secs(1)),
        )
        .await
        .expect("pool")
}

/// Closes the fixture pool and hands back a pool of exactly `connections`
/// over the same database.
async fn reopened(database: &std::path::Path, connections: u32) -> SqlitePool {
    pool_of(database, connections).await
}

/// An export never needs two connections at once, so a pool of one serves
/// it: the audit's own short write transaction ends before the read
/// snapshot begins. An export that held a read transaction while waiting
/// for a second connection would time out here.
#[tokio::test(flavor = "multi_thread")]
async fn a_one_connection_pool_serves_a_whole_export() {
    let (_tmp, pool, admin_id, record_id) = installed().await;
    seed_versions(&pool, admin_id, record_id, 8, PAYLOAD).await;
    let database = pool.connect_options().get_filename().to_owned();
    pool.close().await;
    let single = reopened(std::path::Path::new(&database), 1).await;

    let mut sink = Discard {
        bytes: 0,
        position: 0,
    };
    let (produced, returned) = record_export::export_to(
        &single,
        admin_id,
        Scope::Installation,
        EXPORTED_AT,
        &mut sink,
        |_| {},
    )
    .await
    .expect("call");
    let produced = produced.expect("exported");
    assert_eq!(produced.unit_count, 8);
    assert!(returned.bytes > 8 * PAYLOAD as u64);
    single.close().await;
}

/// Two exports overlapping on a two-connection pool both reach their
/// stream and finish: each holds one connection at a time, so neither waits
/// for a connection the other holds. An export that held its read
/// transaction while acquiring the audit's connection would leave both
/// waiting until their acquisition timeouts.
#[tokio::test(flavor = "multi_thread")]
async fn overlapping_exports_on_a_two_connection_pool_both_complete() {
    let (_tmp, pool, admin_id, record_id) = installed().await;
    seed_versions(&pool, admin_id, record_id, 8, PAYLOAD).await;
    let database = pool.connect_options().get_filename().to_owned();
    pool.close().await;
    let small = reopened(std::path::Path::new(&database), 2).await;

    let first = gate_of();
    let second = gate_of();
    let running: Vec<_> = [first.gate, second.gate]
        .into_iter()
        .map(|gate| {
            let pool = small.clone();
            tokio::task::spawn_blocking(move || {
                tokio::runtime::Handle::current().block_on(async move {
                    record_export::export_to(
                        &pool,
                        admin_id,
                        Scope::Installation,
                        EXPORTED_AT,
                        gate,
                        |_| {},
                    )
                    .await
                })
            })
        })
        .collect();
    // Both exports reach the stream while the other is still open, which is
    // the state that needs two connections.
    first
        .arrived_rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("first export streams");
    second
        .arrived_rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("second export streams");
    first.release.send(()).expect("release the first");
    second.release.send(()).expect("release the second");
    for export in running {
        let (produced, _sink) = export.await.expect("join").expect("call");
        assert_eq!(produced.expect("exported").unit_count, 8);
    }
    small.close().await;
}

/// A destination that holds its first entry until the test releases it.
struct ExportGate {
    position: u64,
    released: std::sync::mpsc::Receiver<()>,
    entered: std::sync::mpsc::Sender<()>,
    held: bool,
}

impl Write for ExportGate {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if !self.held {
            self.held = true;
            let _ = self.entered.send(());
            let _ = self
                .released
                .recv_timeout(std::time::Duration::from_secs(60));
        }
        self.position += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Seek for ExportGate {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.position = match pos {
            std::io::SeekFrom::Start(at) => at,
            std::io::SeekFrom::End(0) | std::io::SeekFrom::Current(0) => self.position,
            other => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    format!("gated sink ({other:?})"),
                ));
            }
        };
        Ok(self.position)
    }
}

/// One held export: its sink, the signal that it reached the stream, and
/// the release it is waiting for.
struct GateHandle {
    gate: ExportGate,
    arrived_rx: std::sync::mpsc::Receiver<()>,
    release: std::sync::mpsc::Sender<()>,
}

fn gate_of() -> GateHandle {
    let (entered_tx, arrived_rx) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    GateHandle {
        gate: ExportGate {
            position: 0,
            released,
            entered: entered_tx,
            held: false,
        },
        arrived_rx,
        release,
    }
}

/// An export mid-stream holds one connection, and the installation's other
/// work keeps running on the rest: the audit is already committed, so the
/// download reserves no writer and no second connection.
#[tokio::test(flavor = "multi_thread")]
async fn ordinary_work_progresses_while_an_export_streams() {
    let (_tmp, pool, admin_id, record_id) = installed().await;
    seed_versions(&pool, admin_id, record_id, 20, PAYLOAD).await;
    let database = pool.connect_options().get_filename().to_owned();
    pool.close().await;
    let small = reopened(std::path::Path::new(&database), 2).await;

    let held = gate_of();
    let export_pool = small.clone();
    let running = tokio::task::spawn_blocking(move || {
        tokio::runtime::Handle::current().block_on(async move {
            record_export::export_to(
                &export_pool,
                admin_id,
                Scope::Installation,
                EXPORTED_AT,
                held.gate,
                |_| {},
            )
            .await
        })
    });
    // The export is on the wire, holding its one connection for the read
    // snapshot; everything below runs on the other.
    held.arrived_rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the export reached its stream");

    let started = std::time::Instant::now();
    consolebook_server::audit::record(
        &small,
        consolebook_server::audit::EventKind::RecordExported,
        Some(admin_id),
        None,
    )
    .await
    .expect("an ordinary write while an export streams");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM evaluation_version")
        .fetch_one(&small)
        .await
        .expect("an ordinary read while an export streams");
    assert_eq!(count, 20);
    let waited = started.elapsed();
    assert!(
        waited < std::time::Duration::from_secs(1),
        "ordinary work waited {waited:?} for a streaming export"
    );

    held.release.send(()).expect("release the export");
    let (produced, _sink) = running.await.expect("join").expect("call");
    assert_eq!(produced.expect("exported").unit_count, 20);
    small.close().await;
}

/// A destination that refuses to accept past `cap`, the way a client that
/// stops reading does once the response's queue is full.
struct Capped {
    cap: u64,
    bytes: u64,
    position: u64,
}

impl Write for Capped {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.position >= self.cap {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "the client left",
            ));
        }
        if self.position == self.bytes {
            self.bytes += buf.len() as u64;
        }
        self.position += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Seek for Capped {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.position = match pos {
            std::io::SeekFrom::Start(at) => at,
            std::io::SeekFrom::End(0) => self.bytes,
            std::io::SeekFrom::Current(0) => self.position,
            other => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    format!("capped sink ({other:?})"),
                ));
            }
        };
        Ok(self.position)
    }
}

/// A destination that stops accepting, the way the response body does once
/// its bounded queue is full and the client is not reading.
#[tokio::test(flavor = "multi_thread")]
async fn a_destination_that_stops_accepting_ends_the_export_instead_of_buffering_it() {
    let (_tmp, pool, admin_id, record_id) = installed().await;
    seed_versions(&pool, admin_id, record_id, 20, PAYLOAD).await;

    // The client stops reading after a couple of records.
    let mut sink = Capped {
        cap: 2 * PAYLOAD as u64,
        bytes: 0,
        position: 0,
    };
    let outcome = record_export::export_to(
        &pool,
        admin_id,
        Scope::Installation,
        EXPORTED_AT,
        &mut sink,
        |_| {},
    )
    .await;
    let err = match outcome {
        Ok((produced, _returned)) => {
            produced.expect_err("a stopped destination is not a complete export");
            panic!("the export completed into a destination that stopped accepting");
        }
        Err(err) => err,
    };
    // The failure is the client's departure, surfaced rather than hidden.
    assert!(
        err.chain()
            .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
            .any(|io| io.kind() == std::io::ErrorKind::BrokenPipe),
        "{err:?}"
    );
    // The producer stopped where the client stopped: it did not run on,
    // holding the rest of the corpus for a reader that had left.
    assert!(
        sink.bytes <= 4 * PAYLOAD as u64,
        "the sink accepted {} bytes after its cap",
        sink.bytes
    );
    pool.close().await;
}

/// The response carries the documented bytes: a client reading from the
/// socket receives the same archive the buffered exporter produces for the
/// same scope and instant, verified from the wire alone.
#[tokio::test(flavor = "multi_thread")]
async fn the_export_response_carries_the_buffered_archive_over_the_wire() {
    let (_tmp, pool, admin_id, record_id) = installed().await;
    seed_versions(&pool, admin_id, record_id, 8, 256 * 1024).await;
    let seeded: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM evaluation_version")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(seeded, 8, "the corpus is present before the request");

    // A live listener, so the response travels over a real socket.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let app =
        consolebook_server::http::router(consolebook_server::http::AppState { pool: pool.clone() });
    let serving = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let login = live_login(&addr, "avery.admin").await;
    let mut client = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let request = format!(
        "GET /api/exports/records HTTP/1.1\r\nHost: {addr}\r\nCookie: {}={}\r\nConnection: close\r\n\r\n",
        consolebook_server::http::SESSION_COOKIE,
        login
    );
    tokio::io::AsyncWriteExt::write_all(&mut client, request.as_bytes())
        .await
        .expect("write request");
    let mut response = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut client, &mut response)
        .await
        .expect("read response");
    serving.abort();

    let (headers, body) = split_response(&response);
    let detail = String::from_utf8_lossy(body).into_owned();
    assert!(headers.starts_with("http/1.1 200"), "{headers} {detail}");
    assert!(headers.contains("application/zip"), "{headers}");
    assert!(
        headers.contains("content-disposition: attachment"),
        "{headers}"
    );
    // The body is chunked and has no length: it is produced as it is
    // written, not assembled and measured first.
    assert!(headers.contains("transfer-encoding: chunked"), "{headers}");
    assert!(!headers.contains("content-length"), "{headers}");
    assert!(chunked_is_complete(body), "the transfer did not finish");
    let body = dechunk(body);
    // The container is complete and holds every unit. (The corpus here is
    // synthetic bytes, so the record-level checks are the fixtures' job;
    // this proof is that the transfer carried the whole container.)
    let report = consolebook_server::export_verify::verify_archive(&body);
    assert_eq!(report.units.len(), 8, "{report:?}");
    assert!(report.findings.is_empty(), "{report:?}");
    assert_eq!(report.scope, Some(Scope::Installation));
    for unit in &report.units {
        assert!(
            !unit.findings.iter().any(|finding| matches!(
                finding,
                consolebook_server::export_verify::Finding::MissingEntry { .. }
            )),
            "unit {} is incomplete: {:?}",
            unit.path,
            unit.findings
        );
    }

    // The delivered bytes are the buffered archive's, for the instant the
    // archive itself states: the streamed path is not merely verifiable, it
    // is byte-for-byte the shipped exporter's output.
    let exported_at = archive_instant(&body);
    let again = record_export::export_at(&pool, admin_id, Scope::Installation, exported_at)
        .await
        .expect("call")
        .expect("exported");
    assert_eq!(
        body,
        again.bytes,
        "the response carried {} bytes; the buffered export for the same instant is {}",
        body.len(),
        again.bytes.len()
    );
    pool.close().await;
}

/// A client that stops reading long enough to lose the failure detail must
/// still lose the transfer. The queue stays full past the data send's
/// deadline **and** past the failure marker's own deadline, so the marker
/// is never queued; the client then resumes reading, and what it receives
/// must be an incomplete transfer rather than a clean end of body carrying
/// a truncated prefix.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_outlasts_every_send_window_never_sees_a_complete_transfer() {
    // Larger than the response queue, the socket buffers, and the runtime's
    // own buffering together, so a client that reads nothing cannot be
    // handed the whole archive from memory.
    let (_tmp, pool, admin_id, record_id) = installed().await;
    seed_versions(&pool, admin_id, record_id, 40, 1024 * 1024).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let app =
        consolebook_server::http::router(consolebook_server::http::AppState { pool: pool.clone() });
    let serving = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let login = live_login(&addr, "avery.admin").await;
    let mut client = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let request = format!(
        "GET /api/exports/records HTTP/1.1\r\nHost: {addr}\r\nCookie: {}={}\r\nConnection: close\r\n\r\n",
        consolebook_server::http::SESSION_COOKIE,
        login
    );
    tokio::io::AsyncWriteExt::write_all(&mut client, request.as_bytes())
        .await
        .expect("write request");

    // Nothing is read while both windows expire: the data send gives up,
    // and the failure marker has no room to be queued either.
    let both_windows = 2 * consolebook_server::exports_http::EXPORT_STALL_LIMIT
        + std::time::Duration::from_secs(5);
    tokio::time::sleep(both_windows).await;

    // Resuming now: everything the failure had already queued drains, and
    // then the stream must end in an error, not in a clean end of body.
    let mut response = Vec::new();
    let read = tokio::io::AsyncReadExt::read_to_end(&mut client, &mut response).await;
    serving.abort();
    let (headers, body) = split_response(&response);
    assert!(headers.starts_with("http/1.1 200"), "{headers}");
    assert!(
        !chunked_is_complete(body),
        "the client outlasted both send windows and still saw a complete transfer of {} bytes",
        body.len()
    );
    let delivered = dechunk(body);
    let complete = record_export::export_at(&pool, admin_id, Scope::Installation, EXPORTED_AT)
        .await
        .expect("call")
        .expect("exported");
    assert!(
        delivered.len() < complete.bytes.len(),
        "the stalled client received the whole {} byte archive (read {read:?})",
        complete.bytes.len()
    );
    pool.close().await;
}

/// The instant the delivered archive states for itself, from its manifest.
fn archive_instant(bytes: &[u8]) -> i64 {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).expect("readable archive");
    let mut file = archive
        .by_name("manifest.json")
        .expect("the archive manifest is the first entry");
    let mut text = String::new();
    std::io::Read::read_to_string(&mut file, &mut text).expect("read manifest");
    let manifest: serde_json::Value = serde_json::from_str(&text).expect("manifest json");
    manifest["exported_at"].as_i64().expect("exported_at")
}

/// Signs in over the live listener and returns the session cookie value.
async fn live_login(addr: &std::net::SocketAddr, username: &str) -> String {
    let mut client = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let body = format!("{{\"username\":\"{username}\",\"password\":\"invented-passphrase-1\"}}");
    let request = format!(
        "POST /api/auth/login HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    tokio::io::AsyncWriteExt::write_all(&mut client, request.as_bytes())
        .await
        .expect("write login");
    let mut response = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut client, &mut response)
        .await
        .expect("read login");
    let text = String::from_utf8_lossy(&response).into_owned();
    let (headers, _) = split_response(&response);
    assert!(headers.starts_with("http/1.1 200"), "{text}");
    let value = headers
        .lines()
        .find_map(|line| line.strip_prefix("set-cookie: "))
        .and_then(|cookie| cookie.split(';').next())
        .and_then(|pair| pair.split_once('='))
        .map(|(_, value)| value.to_owned());
    value.unwrap_or_else(|| panic!("no session cookie in {headers}"))
}

/// A chunked response body as the bytes it carries. A body that was cut
/// short yields the bytes it did carry, so a truncated transfer can be
/// inspected rather than panicking.
fn dechunk(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(end) = body[at..].windows(2).position(|window| window == b"\r\n") {
        let size_line = String::from_utf8_lossy(&body[at..at + end]).into_owned();
        let Ok(size) = usize::from_str_radix(size_line.split(';').next().unwrap_or(""), 16) else {
            break;
        };
        at += end + 2;
        if size == 0 || at + size > body.len() {
            break;
        }
        out.extend_from_slice(&body[at..at + size]);
        at += size + 2;
    }
    out
}

/// Whether a chunked body carries its terminal zero-length chunk. A
/// transfer that ends without it was cut short, which is how a client
/// distinguishes an incomplete download from a complete one.
fn chunked_is_complete(body: &[u8]) -> bool {
    let mut at = 0;
    while let Some(end) = body[at..].windows(2).position(|window| window == b"\r\n") {
        let size_line = String::from_utf8_lossy(&body[at..at + end]).into_owned();
        let Ok(size) = usize::from_str_radix(size_line.split(';').next().unwrap_or(""), 16) else {
            return false;
        };
        at += end + 2;
        if size == 0 {
            return true;
        }
        if at + size + 2 > body.len() {
            return false;
        }
        at += size + 2;
    }
    false
}

/// The response's header block and its body.
fn split_response(response: &[u8]) -> (String, &[u8]) {
    let at = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("header end");
    (
        String::from_utf8_lossy(&response[..at]).to_lowercase(),
        &response[at + 4..],
    )
}
