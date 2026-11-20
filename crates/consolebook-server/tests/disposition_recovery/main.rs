//! Disposition-aware backup, copy, restore, and recovery progress tests
//! (ADR 0027; ADR 0030; #213). All agencies, people, schedules, authorities,
//! and records are invented.

#[path = "../support/mod.rs"]
mod support;

mod drill;

use std::path::{Path, PathBuf};
use std::process::Output;

use axum::body::Body;
use axum::http::header::COOKIE;
use axum::http::{Request, StatusCode};
use consolebook_server::backup_copy::{self, BackupSettings};
use consolebook_server::capabilities::{self, Capability, RoleBundle};
use consolebook_server::data_dir::DataDir;
use consolebook_server::disposition_evidence::{
    AUTHENTICITY_SELF_CONSISTENCY_ONLY, DispositionErrorCode, DispositionRunState,
};
use consolebook_server::disposition_recovery::{
    self, CopyArtifactsInventory, CopyLedgerStatus, DISPOSITION_LEDGER_FILENAME,
    DISPOSITION_LEDGER_FORMAT, DISPOSITION_LEDGER_FORMAT_VERSION, DispositionLedger,
    DispositionLedgerEntry, EXTERNAL_COPIES_NON_GUARANTEE, LedgerError, LocalLedgerStatus,
    RecoveryHealthState,
};
use consolebook_server::retention::{
    self, PolicyInput, RecordClass, RetentionAction, RetentionTrigger,
};
use consolebook_server::{
    assignments, backup, canonical, doctor, enrollments, evaluation_drafts, finalization, http,
    programs, secrets, setup, storage, training_sessions, users,
};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::SqlitePool;
use tower::ServiceExt;

/// A separate tmpfs filesystem standing in for a mounted off-server share.
pub(crate) fn other_filesystem(beside: &Path) -> tempfile::TempDir {
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir_in("/dev/shm").expect("these tests need /dev/shm (a tmpfs)");
    let dev = |p: &Path| std::fs::metadata(p).expect("metadata").dev();
    assert_ne!(
        dev(dir.path()),
        dev(beside),
        "/dev/shm must be a different filesystem from the temp directory"
    );
    dir
}

pub(crate) fn run_cli(data_dir: &DataDir, args: &[&str]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_consolebook-server"))
        .env_remove("CONSOLEBOOK_BACKUP_COPY_DIR")
        .env_remove("CONSOLEBOOK_DATA_DIR")
        .arg("--data-dir")
        .arg(data_dir.root())
        .args(args)
        .output()
        .expect("run consolebook-server CLI")
}

/// Complete invented installation with:
/// - `retained_record_id`: finalized record kept across disposition
/// - `disposed_record_id`: finalized record present in `pre_disposition_snapshot`
///   and absent + tombstoned in `live` (`data_dir`) and `post_disposition_snapshot`
/// - `disposed_content_hash`: real SHA-256 content hash of `disposed_record_id`
pub(crate) struct DrillInstallation {
    pub tmp: tempfile::TempDir,
    pub other_fs: tempfile::TempDir,
    pub data_dir: DataDir,
    pub copy_dir: PathBuf,
    pub installation_id: String,
    pub admin_id: i64,
    pub retained_record_id: i64,
    pub disposed_record_id: i64,
    pub disposed_content_hash: String,
    pub disposed_at: i64,
    pub pre_disposition_snapshot: PathBuf,
    pub post_disposition_snapshot: PathBuf,
}

impl DrillInstallation {
    #[allow(clippy::too_many_lines)]
    pub async fn new() -> Self {
        let tmp = tempfile::tempdir().expect("scratch");
        let other_fs = other_filesystem(tmp.path());
        let data_dir = DataDir::new(tmp.path().join("data"));
        data_dir.ensure_layout().expect("layout");
        let copy_dir = other_fs.path().join("offsite");
        std::fs::create_dir_all(&copy_dir).expect("copy dir");

        let pool = storage::open(&data_dir.database()).await.expect("open");
        let installation_id = storage::installation_id(&pool).await.expect("id");

        let code = setup::issue_setup_code(&pool)
            .await
            .expect("issue")
            .expect("uninitialized")
            .0;
        let admin_id = setup::initialize(
            &pool,
            &code.raw,
            "Cedar Ridge Communications",
            "avery.admin",
            "Avery Admin",
            support::password(),
        )
        .await
        .expect("init")
        .expect("accepted");

        let mut tx = storage::write_tx(&pool).await.expect("seed users");
        let hash = secrets::hash_password(support::password()).expect("hash");
        let instructor_id =
            users::create(&mut tx, "jordan.trainer", "Jordan Trainer", "", "", &hash)
                .await
                .expect("trainer");
        capabilities::grant_bundle(
            &mut tx,
            instructor_id,
            RoleBundle::Trainer.capabilities(),
            Some(admin_id),
        )
        .await
        .expect("trainer bundle");
        let trainee_id = users::create(&mut tx, "taylor.trainee", "Taylor Trainee", "", "", &hash)
            .await
            .expect("trainee");
        capabilities::grant_bundle(
            &mut tx,
            trainee_id,
            RoleBundle::Trainee.capabilities(),
            Some(admin_id),
        )
        .await
        .expect("trainee bundle");
        capabilities::grant_bundle(
            &mut tx,
            admin_id,
            &[Capability::ReviewEvaluation],
            Some(admin_id),
        )
        .await
        .expect("review grant");
        tx.commit().await.expect("commit");

        retention::set_authority(
            &pool,
            admin_id,
            admin_id,
            true,
            "Invented retention authority",
        )
        .await
        .expect("call")
        .expect("grant");
        let daily_destroy_policy_id = retention::create_policy(
            &pool,
            admin_id,
            &PolicyInput {
                record_class: RecordClass::DailyReport,
                expected_current_id: None,
                authority: "INVENTED-CEDAR-DAILY-2026".into(),
                retention_trigger: RetentionTrigger::FinalizedAt,
                retention_days: 30,
                action: RetentionAction::Destroy,
                reason: "Invented schedule".into(),
            },
        )
        .await
        .expect("call")
        .expect("policy");

        let content: programs::VersionContent = serde_json::from_value(serde_json::json!({
            "name": "Cedar Ridge CTO Program",
            "label": "2026 A",
            "description": "Invented recovery drill program.",
            "phases": [],
            "phase_transitions": [],
            "competencies": [],
            "rating_scales": [],
            "rating_modifiers": [],
            "citations": [],
            "evaluation_forms": [{
                "record_type": "daily_report",
                "name": "Invented Daily",
                "instructions": "",
                "competencies": [],
                "narratives": [{"prompt": "Invented notes", "required": false}]
            }],
            "finalization_policy": {
                "review_approved": false,
                "required_narratives": false,
                "ratings_complete": false
            }
        }))
        .expect("program content");
        let program_id = programs::create_program(&pool, admin_id, &content.name)
            .await
            .expect("call")
            .expect("program");
        let version_id = programs::create_version(&pool, admin_id, program_id, &content)
            .await
            .expect("call")
            .expect("version");
        programs::publish_version(&pool, admin_id, version_id)
            .await
            .expect("call")
            .expect("publish");
        let daily_form_id: i64 = sqlx::query_scalar(
            "SELECT id FROM evaluation_form WHERE program_version_id = ?1 AND record_type = 'daily_report'",
        )
        .bind(version_id)
        .fetch_one(&pool)
        .await
        .expect("daily form");

        let enrollment_id = enrollments::enroll(&pool, admin_id, version_id, trainee_id)
            .await
            .expect("call")
            .expect("enroll");
        assignments::create(&pool, admin_id, enrollment_id, instructor_id)
            .await
            .expect("call")
            .expect("assign");

        // Finalize record 1 (retained across disposition).
        let retained_record_id = create_finalized_daily(
            &pool,
            admin_id,
            instructor_id,
            enrollment_id,
            daily_form_id,
            "2026-01-05",
        )
        .await
        .0;

        // Capture a clean pre-record-2 base snapshot so we can build an authentic
        // post-disposition DB without bypassing immutability triggers.
        let clean_base = tmp.path().join("clean-base.db");
        sqlx::query("VACUUM INTO ?1")
            .bind(clean_base.to_str().expect("utf8"))
            .execute(&pool)
            .await
            .expect("base snapshot");

        // Finalize record 2 (the record to be lawfully disposed).
        let (disposed_record_id, disposed_content_hash, disposed_chain_hash, disposed_finalized_at) =
            create_finalized_daily(
                &pool,
                admin_id,
                instructor_id,
                enrollment_id,
                daily_form_id,
                "2026-01-06",
            )
            .await;
        pool.close().await;

        // Capture the pre-disposition snapshot (contains BOTH record 1 and record 2).
        let pre_snap = backup::run(&data_dir, 14)
            .await
            .expect("pre-disposition backup")
            .snapshot;
        let pre_disposition_snapshot = tmp.path().join("consolebook-20260106T120000Z.db");
        std::fs::copy(&pre_snap, &pre_disposition_snapshot).expect("copy pre snap");
        std::fs::remove_file(&pre_snap).expect("clean local pre snap");

        // Replace live DB with clean_base + completed disposition_run /
        // disposition_event / disposition_version_hash for disposed_record_id.
        for sidecar in ["consolebook.db-wal", "consolebook.db-shm"] {
            let p = data_dir.root().join(sidecar);
            if p.exists() {
                std::fs::remove_file(p).expect("remove sidecar");
            }
        }
        std::fs::copy(&clean_base, data_dir.database()).expect("install post-disposition base");

        let disposed_at = disposed_finalized_at + 31 * 86_400;
        let pool = storage::open(&data_dir.database()).await.expect("reopen");
        let digest = "b".repeat(64);
        let run_id: i64 = sqlx::query(
            "INSERT INTO disposition_run (actor_user_id, confirmation_digest, authority_reference, reason_code, method, state, started_at, completed_at) \
             VALUES (?1, ?2, 'INVENTED-CEDAR-DAILY-2026', 'scheduled_retention_elapsed', 'lawful_destruction', 'completed', ?3, ?4)",
        )
        .bind(admin_id)
        .bind(&digest)
        .bind(disposed_at)
        .bind(disposed_at + 10)
        .execute(&pool)
        .await
        .expect("insert completed run")
        .last_insert_rowid();

        let event_id: i64 = sqlx::query(
            "INSERT INTO disposition_event \
             (disposition_run_id, evaluation_record_id, enrollment_id, record_class, record_policy_id, metadata_policy_id, version_count, earliest_finalized_at, latest_finalized_at, hashes_retained, actor_user_id, authority_reference, reason_code, method, outcome, disposed_at) \
             VALUES (?1, ?2, ?3, 'daily_report', ?4, NULL, 1, ?5, ?5, 1, ?6, 'INVENTED-CEDAR-DAILY-2026', 'scheduled_retention_elapsed', 'lawful_destruction', 'disposed', ?7)",
        )
        .bind(run_id)
        .bind(disposed_record_id)
        .bind(enrollment_id)
        .bind(daily_destroy_policy_id)
        .bind(disposed_finalized_at)
        .bind(admin_id)
        .bind(disposed_at)
        .execute(&pool)
        .await
        .expect("insert disposition event")
        .last_insert_rowid();

        sqlx::query(
            "INSERT INTO disposition_version_hash (disposition_event_id, version_number, record_schema, content_hash, chain_hash, predecessor_content_hash, finalized_at) \
             VALUES (?1, 1, 2, ?2, ?3, NULL, ?4)",
        )
        .bind(event_id)
        .bind(&disposed_content_hash)
        .bind(&disposed_chain_hash)
        .bind(disposed_finalized_at)
        .execute(&pool)
        .await
        .expect("insert version hash");

        // Write durable local ledger and mirror to off-server copy directory.
        let ledger = disposition_recovery::sync_local_ledger_from_db(&data_dir, &pool, disposed_at)
            .await
            .expect("sync local ledger");
        disposition_recovery::mirror_ledger_to_copy_dir(&data_dir, &copy_dir, &ledger)
            .await
            .expect("mirror ledger");
        pool.close().await;

        // Take a genuine post-disposition snapshot and copy it off-server.
        let post_disposition_snapshot = backup::run(&data_dir, 14)
            .await
            .expect("post-disposition backup")
            .snapshot;
        backup_copy::copy_snapshot_with_data_dir(
            &data_dir,
            &post_disposition_snapshot,
            &copy_dir,
            14,
        )
        .await
        .expect("copy post-disposition snapshot");

        Self {
            tmp,
            other_fs,
            data_dir,
            copy_dir,
            installation_id,
            admin_id,
            retained_record_id,
            disposed_record_id,
            disposed_content_hash,
            disposed_at,
            pre_disposition_snapshot,
            post_disposition_snapshot,
        }
    }
}

async fn create_finalized_daily(
    pool: &SqlitePool,
    admin_id: i64,
    instructor_id: i64,
    enrollment_id: i64,
    daily_form_id: i64,
    date: &str,
) -> (i64, String, String, i64) {
    let session_id = training_sessions::create(
        pool,
        admin_id,
        enrollment_id,
        &training_sessions::SessionInput {
            business_date: date.into(),
            timezone: "UTC".into(),
            local_start: format!("{date}T08:00"),
            local_end: Some(format!("{date}T16:00")),
            disposition: Some(training_sessions::Disposition::Completed),
            phase_id: None,
            trainer_user_ids: vec![instructor_id],
        },
    )
    .await
    .expect("call")
    .expect("session");
    let record_id = evaluation_drafts::create(pool, instructor_id, session_id, Some(daily_form_id))
        .await
        .expect("call")
        .expect("draft");
    let meta = finalization::finalize(pool, admin_id, record_id, 0)
        .await
        .expect("call")
        .expect("finalize");
    (
        record_id,
        meta.content_hash,
        meta.chain_hash,
        meta.finalized_at,
    )
}

#[test]
fn recovery_ledger_strictly_validates_format_allowlist_and_canonical_encoding() {
    let ledger = DispositionLedger::new(
        "invented-install-001",
        1_788_000_000,
        vec![DispositionLedgerEntry {
            disposition_run_id: 1,
            evaluation_record_id: 42,
            content_hashes: vec!["a".repeat(64), "b".repeat(64)],
            disposed_at: 1_788_000_000,
            policy_authority: "INVENTED-SCHEDULE-2026".into(),
        }],
    );
    let bytes = disposition_recovery::encode_canonical(&ledger).expect("encode");
    let parsed = disposition_recovery::parse_and_verify(Path::new("ledger.json"), &bytes)
        .expect("round-trip");
    assert_eq!(parsed, ledger);
    assert_eq!(parsed.format, DISPOSITION_LEDGER_FORMAT);
    assert_eq!(parsed.format_version, DISPOSITION_LEDGER_FORMAT_VERSION);
    assert_eq!(parsed.authenticity, AUTHENTICITY_SELF_CONSISTENCY_ONLY);

    let base: Value = serde_json::from_slice(&bytes).expect("json");
    let canon = |v: &Value| canonical::canonical_bytes(v).expect("canon");

    // Unknown format_version fails by name.
    let mut bad_ver = base.clone();
    bad_ver["format_version"] = Value::from(99);
    assert!(matches!(
        disposition_recovery::parse_and_verify(Path::new("l.json"), &canon(&bad_ver)),
        Err(LedgerError::UnsupportedFormat {
            format_version: 99,
            ..
        })
    ));

    // Non-canonical JSON fails closed.
    let pretty = serde_json::to_vec_pretty(&base).expect("pretty");
    assert!(matches!(
        disposition_recovery::parse_and_verify(Path::new("l.json"), &pretty),
        Err(LedgerError::NotCanonical { .. })
    ));

    // Injected content-bearing or false signature fields fail closed.
    for (field, mutate) in [
        (
            "signature",
            Box::new(|v: &mut Value| {
                v["signature"] = Value::from("fake");
            }) as Box<dyn Fn(&mut Value)>,
        ),
        (
            "narrative",
            Box::new(|v: &mut Value| {
                v["entries"][0]["narrative"] = Value::from("Leaked text");
            }),
        ),
        (
            "display_name",
            Box::new(|v: &mut Value| {
                v["entries"][0]["display_name"] = Value::from("Taylor Trainee");
            }),
        ),
    ] {
        let mut mutated = base.clone();
        mutate(&mut mutated);
        assert!(
            matches!(
                disposition_recovery::parse_and_verify(Path::new("l.json"), &canon(&mutated)),
                Err(LedgerError::DisallowedOrUnknownField { field: f, .. }) if f == field
            ),
            "must reject disallowed field {field}"
        );
    }

    // False authenticity claim fails closed.
    let mut bad_auth = base.clone();
    bad_auth["authenticity"] = Value::from("signed");
    assert!(matches!(
        disposition_recovery::parse_and_verify(Path::new("l.json"), &canon(&bad_auth)),
        Err(LedgerError::UnsupportedAuthenticity { .. })
    ));

    // Out-of-order entries fail closed.
    let mut out_of_order = base;
    let first = out_of_order["entries"][0].clone();
    let mut second = first.clone();
    second["evaluation_record_id"] = Value::from(10);
    out_of_order["entries"] = Value::Array(vec![first, second]);
    assert!(matches!(
        disposition_recovery::parse_and_verify(Path::new("l.json"), &canon(&out_of_order)),
        Err(LedgerError::InvalidContent { .. })
    ));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn interrupted_progress_and_controlled_inventory_surface_honestly_without_mutating_state() {
    let fx = DrillInstallation::new().await;

    // Seed scheduled, pre-migration, pre-restore (.db & .damaged), temp, and
    // staged export artifacts to prove inventory coverage across all families.
    let premig = fx
        .data_dir
        .backups()
        .join("consolebook-premigration-20260101T000000Z.db");
    let prerestore_db = fx
        .data_dir
        .backups()
        .join("consolebook-prerestore-20260101T000001Z.db");
    let prerestore_dmg = fx
        .data_dir
        .backups()
        .join("consolebook-prerestore-20260101T000002Z.damaged");
    let partial = fx
        .data_dir
        .backups()
        .join("consolebook-20260101T000003Z.db.partial");
    let staged_export = fx.data_dir.exports().join("staged-invented.zip");
    for path in [
        &premig,
        &prerestore_db,
        &prerestore_dmg,
        &partial,
        &staged_export,
    ] {
        std::fs::write(path, b"invented artifact").expect("seed artifact");
    }

    let inv =
        disposition_recovery::enumerate_controlled_artifacts(&fx.data_dir, Some(&fx.copy_dir))
            .expect("inventory");
    assert_eq!(inv.local_snapshots.len(), 1);
    assert_eq!(inv.local_premigration_snapshots, vec![premig]);
    assert_eq!(
        inv.local_prerestore_files,
        vec![prerestore_db, prerestore_dmg]
    );
    assert_eq!(inv.local_temp_files, vec![partial]);
    assert_eq!(inv.staged_exports, vec![staged_export]);
    assert_eq!(inv.external_copies_note, EXTERNAL_COPIES_NON_GUARANTEE);
    match &inv.copy {
        CopyArtifactsInventory::Reachable {
            snapshots,
            has_recovery_ledger,
            ..
        } => {
            assert_eq!(snapshots.len(), 1);
            assert!(has_recovery_ledger);
        }
        other => panic!("expected reachable copy inventory, got {other:?}"),
    }

    // Unreachable copy directory is surfaced honestly without creating it.
    let unmounted = fx.other_fs.path().join("missing-share");
    let unreach_inv =
        disposition_recovery::enumerate_controlled_artifacts(&fx.data_dir, Some(&unmounted))
            .expect("unreachable inventory");
    assert!(matches!(
        unreach_inv.copy,
        CopyArtifactsInventory::Unreachable { .. }
    ));
    assert!(!unmounted.exists());

    // Insert an incomplete disposition_run (simulating a crash right after DB commit).
    let pool = storage::open(&fx.data_dir.database()).await.expect("open");
    let digest = "c".repeat(64);
    let incomplete_run_id: i64 = sqlx::query(
        "INSERT INTO disposition_run (actor_user_id, confirmation_digest, authority_reference, reason_code, method, state, started_at) \
         VALUES (?1, ?2, 'INVENTED-CEDAR-DAILY-2026', 'scheduled_retention_elapsed', 'lawful_destruction', 'db_committed_vacuum_pending', ?3)",
    )
    .bind(fx.admin_id)
    .bind(&digest)
    .bind(fx.disposed_at + 100)
    .execute(&pool)
    .await
    .expect("insert incomplete run")
    .last_insert_rowid();

    // Cannot mark completed directly from db_committed_vacuum_pending.
    assert!(
        disposition_recovery::mark_run_completed(&pool, incomplete_run_id, fx.disposed_at + 200)
            .await
            .is_err()
    );

    // Record step failure and confirm doctor + /api/backups/status report incomplete_disposition.
    disposition_recovery::record_step_failure(
        &pool,
        incomplete_run_id,
        DispositionErrorCode::VacuumFailed,
    )
    .await
    .expect("record vacuum failure");

    let settings = BackupSettings {
        data_dir: fx.data_dir.clone(),
        interval_hours: 24,
        keep: 14,
        copy_dir: Some(fx.copy_dir.clone()),
    };
    let status = backup_copy::status_for(&pool, fx.admin_id, &settings)
        .await
        .expect("call")
        .expect("admin allowed");
    assert_eq!(
        status.disposition_recovery.state,
        RecoveryHealthState::IncompleteDisposition
    );
    assert_eq!(status.disposition_recovery.incomplete_runs.len(), 1);
    assert_eq!(
        status.disposition_recovery.incomplete_runs[0].state,
        DispositionRunState::DbCommittedVacuumPending
    );
    assert_eq!(
        status.disposition_recovery.incomplete_runs[0].last_error_code,
        Some(DispositionErrorCode::VacuumFailed)
    );

    // HTTP /api/backups/status carries the same typed disposition_recovery payload.
    let app = http::router(http::AppState::new(pool.clone()).with_backups(settings.clone()));
    let session = support::login(app.clone(), "avery.admin", support::password()).await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/backups/status")
                .header(COOKIE, format!("{}={session}", http::SESSION_COOKIE))
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.expect("body").to_bytes())
            .expect("json");
    assert_eq!(
        body["disposition_recovery"]["state"],
        "incomplete_disposition"
    );
    assert_eq!(
        body["disposition_recovery"]["incomplete_runs"][0]["last_error_code"],
        "vacuum_failed"
    );

    // Doctor CLI fails on incomplete run and leaves ledger & DB untouched.
    pool.close().await;
    let db_before = std::fs::read(fx.data_dir.database()).expect("db before");
    let ledger_before = std::fs::read(fx.data_dir.disposition_ledger()).expect("ledger before");
    let out = run_cli(
        &fx.data_dir,
        &[
            "doctor",
            "--backup-copy-dir",
            fx.copy_dir.to_str().expect("utf8"),
        ],
    );
    assert!(!out.status.success(), "doctor must fail on incomplete run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("FAIL")
            && stdout.contains("disposition recovery")
            && stdout.contains("vacuum_failed"),
        "{stdout}"
    );
    assert_eq!(
        std::fs::read(fx.data_dir.database()).expect("db after"),
        db_before
    );
    assert_eq!(
        std::fs::read(fx.data_dir.disposition_ledger()).expect("ledger after"),
        ledger_before
    );

    // Advance monotonically to copy_cleanup_pending -> still incomplete; then complete -> clear.
    let pool = storage::open(&fx.data_dir.database())
        .await
        .expect("reopen");
    disposition_recovery::advance_to_copy_cleanup_pending(
        &pool,
        incomplete_run_id,
        Some(DispositionErrorCode::CopyDirUnreachable),
    )
    .await
    .expect("advance");
    let health =
        disposition_recovery::inspect_recovery_health(&pool, &fx.data_dir, Some(&fx.copy_dir))
            .await
            .expect("health");
    assert_eq!(health.state, RecoveryHealthState::IncompleteDisposition);
    assert_eq!(
        health.incomplete_runs[0].state,
        DispositionRunState::CopyCleanupPending
    );
    assert_eq!(
        health.incomplete_runs[0].last_error_code,
        Some(DispositionErrorCode::CopyDirUnreachable)
    );

    disposition_recovery::mark_run_completed(&pool, incomplete_run_id, fx.disposed_at + 300)
        .await
        .expect("complete");
    // Completed run cannot be mutated again.
    assert!(
        disposition_recovery::record_step_failure(
            &pool,
            incomplete_run_id,
            DispositionErrorCode::CopyCleanupFailed
        )
        .await
        .is_err()
    );
    assert!(
        disposition_recovery::advance_to_copy_cleanup_pending(&pool, incomplete_run_id, None)
            .await
            .is_err()
    );

    let health =
        disposition_recovery::inspect_recovery_health(&pool, &fx.data_dir, Some(&fx.copy_dir))
            .await
            .expect("health");
    assert_eq!(health.state, RecoveryHealthState::Clear);
    assert!(matches!(
        health.local_ledger,
        LocalLedgerStatus::Present { entry_count: 1, .. }
    ));
    assert!(matches!(
        health.copy_ledger,
        CopyLedgerStatus::Current { entry_count: 1, .. }
    ));
    pool.close().await;

    let findings = doctor::run(
        &fx.data_dir,
        &doctor::Options {
            backup_copy_dir: Some(&fx.copy_dir),
            ..doctor::Options::default()
        },
    )
    .await;
    assert!(!doctor::has_failure(&findings), "{findings:?}");
    let rec = findings
        .iter()
        .find(|f| f.check == "disposition recovery")
        .expect("finding");
    assert_eq!(rec.verdict, doctor::Verdict::Ok);
    assert!(
        fx.copy_dir.join(DISPOSITION_LEDGER_FILENAME).is_file(),
        "mirror stayed intact"
    );
}
