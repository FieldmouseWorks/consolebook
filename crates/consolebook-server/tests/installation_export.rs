//! #98: the complete installation export — every finalized version as a
//! record-export unit and every other stored table as a typed document,
//! credentials excluded; verified from the archive alone with named
//! findings; produced for `export_records` holders, audited, and delivered
//! over the API. Every fixture is invented.

use std::io::{Cursor, Read, Write};

use axum::body::Body;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, Request, StatusCode};
use consolebook_server::acknowledgments::{self, TraineeAckKind};
use consolebook_server::capabilities::RoleBundle;
use consolebook_server::draft_content::{self, DraftContent, NarrativeEntry, RatingEntry};
use consolebook_server::draft_review::{self, ReviewDecisionKind};
use consolebook_server::export_verify::{self, ArchiveKind, ArchiveReport, Finding};
use consolebook_server::installation_export::{
    self, INSTALLATION_FORMAT, INSTALLATION_FORMAT_VERSION, InstallationManifest, spec,
};
use consolebook_server::installation_verify::InstallationFinding;
use consolebook_server::lifecycle::{self, EnrollmentEventKind, PhaseEventKind};
use consolebook_server::record_export::{ARCHIVE_MANIFEST_PATH, ExportRefusal};
use consolebook_server::retention::{
    self, HoldInput, HoldKind, HoldScope, PolicyInput, RecordClass, RetentionAction,
    RetentionTrigger,
};
use consolebook_server::task_signoffs::{self, SignoffKind};
use consolebook_server::training_sessions::{self, Disposition, SessionInput};
use consolebook_server::{
    amendments, assignments, canonical, data_dir::DataDir, enrollments, evaluation_drafts,
    finalization, programs, session_membership, setup, storage, summaries, users,
};
use http_body_util::BodyExt;
use tower::ServiceExt;
use zip::write::{SimpleFileOptions, ZipWriter};

#[path = "installation_export/coverage.rs"]
mod coverage;
#[path = "installation_export/document.rs"]
mod document;
#[path = "installation_export/floor.rs"]
mod floor;
#[path = "installation_export/golden.rs"]
mod golden;
#[path = "installation_export/negative.rs"]
mod negative;
#[path = "installation_export/properties.rs"]
mod properties;
#[path = "installation_export/verifier.rs"]
mod verifier;

const PASSWORD: &str = "invented-passphrase-1";

/// 2026-09-01T19:00:00Z.
const EXPORTED_AT: i64 = 1_788_289_200;

struct Fixture {
    _tmp: tempfile::TempDir,
    pool: sqlx::SqlitePool,
    admin_id: i64,
    /// The setup code's stored hash, read before the setup transaction
    /// deleted it.
    setup_code_hash: String,
}

impl Fixture {
    async fn new() -> Self {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let data_dir = DataDir::new(tmp.path().join("data"));
        data_dir.ensure_layout().expect("create layout");
        let pool = storage::open(&data_dir.database()).await.expect("open");
        let code = setup::issue_setup_code(&pool)
            .await
            .expect("issue")
            .expect("uninitialized")
            .0;
        let setup_code_hash: String = sqlx::query_scalar("SELECT code_hash FROM setup_code")
            .fetch_one(&pool)
            .await
            .expect("stored setup code");
        let admin_id = setup::initialize(
            &pool,
            &code.raw,
            "Example County Communications",
            "avery.admin",
            "Avery Admin",
            PASSWORD,
        )
        .await
        .expect("initialize")
        .expect("accepted");
        Self {
            _tmp: tmp,
            pool,
            admin_id,
            setup_code_hash,
        }
    }

    fn app(&self) -> axum::Router {
        consolebook_server::http::router(consolebook_server::http::AppState::new(self.pool.clone()))
    }

    async fn login(&self, username: &str) -> String {
        let response = self
            .app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "username": username, "password": PASSWORD })
                            .to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK, "login {username}");
        let cookie = response
            .headers()
            .get(SET_COOKIE)
            .expect("cookie")
            .to_str()
            .expect("ascii");
        let (pair, _) = cookie.split_once(';').expect("attrs");
        pair.split_once('=').expect("pair").1.to_string()
    }

    async fn user_with_role(&self, username: &str, display_name: &str, role: RoleBundle) -> i64 {
        let created = users::create_with_reset_code(
            &self.pool,
            self.admin_id,
            username,
            display_name,
            "E-1001",
            "Telecommunicator",
            role,
        )
        .await
        .expect("create")
        .expect("accepted");
        assert_eq!(
            users::use_reset_code(&self.pool, username, &created.reset_code.raw, PASSWORD)
                .await
                .expect("reset"),
            users::ResetOutcome::Done
        );
        created.id
    }

    async fn id(&self, sql: &str, bind: i64) -> i64 {
        sqlx::query_scalar(sql)
            .bind(bind)
            .fetch_one(&self.pool)
            .await
            .unwrap_or_else(|err| panic!("{sql}: {err}"))
    }

    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM \"{table}\""))
            .fetch_one(&self.pool)
            .await
            .expect("count")
    }

    async fn installation_audits(
        &self,
    ) -> Vec<(Option<i64>, Option<i64>, Option<String>, Option<i64>)> {
        sqlx::query_as(
            "SELECT actor_user_id, subject_user_id, subject_kind, subject_id
             FROM audit_event WHERE kind = 'installation_exported' ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .expect("audits")
    }
}

#[allow(clippy::struct_field_names)]
struct Populated {
    fx: Fixture,
    taylor_id: i64,
    jordan_id: i64,
    rowan_id: i64,
    casey_id: i64,
    spare_version_id: i64,
    amended_record_id: i64,
    summary_record_id: i64,
}

/// An invented program exercising every configuration table: two phases
/// and a transition, a competency with a task, citations at every level,
/// an anchored scale, a modifier, a daily and a weekly form.
fn program_content(name: &str) -> programs::VersionContent {
    serde_json::from_value(serde_json::json!({
        "name": name,
        "label": "2026 rev A",
        "description": "Invented program for installation-export tests.",
        "phases": [
            {"name": "Phase One", "description": "Invented first phase.", "presentation_number": 1},
            {"name": "Phase Two", "description": "Invented second phase.", "presentation_number": 2}
        ],
        "phase_transitions": [
            {"from_phase": "Phase One", "to_phase": "Phase Two", "kind": "advance"}
        ],
        "competencies": [{
            "category": "Call processing",
            "name": "Emergency Call Interrogation",
            "description": "Obtains and verifies location, callback, and nature.",
            "tasks": [{
                "prompt": "Processes an invented structure-fire call.",
                "citations": [{"body": "Invented Standards Body", "edition": "2026", "clause": "T-1", "note": "Task-level."}]
            }],
            "citations": [{"body": "Invented Standards Body", "edition": "2026", "clause": "C-1", "note": "Competency-level."}]
        }],
        "rating_scales": [{
            "name": "Standard 1-7",
            "kind": "anchored_numeric",
            "min_value": 1,
            "max_value": 7,
            "anchors": [{"value": 4, "label": "Meets standards", "definition": "To the invented standard."}]
        }],
        "rating_modifiers": [{"code": "R", "label": "Remediation", "description": "Invented remediation marker."}],
        "evaluation_forms": [
            {
                "record_type": "daily_report",
                "name": "Daily Observation Report",
                "instructions": "Rate observed performance.",
                "competencies": [{"competency": "Emergency Call Interrogation", "rating_scale": "Standard 1-7"}],
                "narratives": [{"prompt": "Most acceptable performance.", "required": false}]
            },
            {
                "record_type": "weekly_summary",
                "name": "Weekly Summary",
                "instructions": "Summarize the invented week.",
                "competencies": [],
                "narratives": [{"prompt": "Week in review.", "required": false}]
            }
        ],
        "citations": [{"body": "Invented Standards Body", "edition": "", "clause": "V-1", "note": "Version-level."}],
        "finalization_policy": {"review_approved": false, "required_narratives": false, "ratings_complete": false}
    }))
    .expect("program content")
}

async fn session_for(fx: &Fixture, trainer_id: i64, enrollment_id: i64, date: &str) -> i64 {
    training_sessions::create(
        &fx.pool,
        trainer_id,
        enrollment_id,
        &SessionInput {
            business_date: date.to_owned(),
            timezone: "America/Chicago".to_owned(),
            local_start: format!("{date}T07:00"),
            local_end: Some(format!("{date}T15:00")),
            disposition: Some(Disposition::Completed),
            phase_id: None,
            trainer_user_ids: Vec::new(),
        },
    )
    .await
    .expect("call")
    .expect("created")
}

/// Saves a draft's content at its current revision; returns the new one.
async fn save(
    fx: &Fixture,
    actor: i64,
    record_id: i64,
    value: i64,
    modifier: Option<i64>,
    text: &str,
) -> i64 {
    let workspace = evaluation_drafts::workspace(&fx.pool, actor, record_id)
        .await
        .expect("call")
        .expect("readable");
    let ratings = workspace
        .form
        .competencies
        .iter()
        .map(|competency| RatingEntry {
            form_competency_id: competency.form_competency_id,
            value: Some(value),
            not_observed: false,
            modifier_ids: modifier.into_iter().collect(),
        })
        .collect();
    draft_content::save(
        &fx.pool,
        actor,
        record_id,
        workspace.detail.revision,
        &DraftContent {
            ratings,
            narratives: vec![NarrativeEntry {
                form_narrative_id: workspace.form.narratives[0].form_narrative_id,
                text: text.to_owned(),
            }],
        },
    )
    .await
    .expect("call")
    .expect("saved")
}

/// An installation where every carried table holds at least one row,
/// reached only through the product's own services and API.
#[allow(clippy::too_many_lines)]
async fn populate() -> Populated {
    let fx = Fixture::new().await;
    let taylor_id = fx
        .user_with_role("taylor.trainee", "Taylor Trainee", RoleBundle::Trainee)
        .await;
    let jordan_id = fx
        .user_with_role("jordan.trainer", "Jordan Trainer", RoleBundle::Trainer)
        .await;
    let rowan_id = fx
        .user_with_role("rowan.trainer", "Rowan Trainer", RoleBundle::Trainer)
        .await;
    let casey_id = fx
        .user_with_role(
            "casey.coordinator",
            "Casey Coordinator",
            RoleBundle::Coordinator,
        )
        .await;
    // An unused reset code and a live login session: credentials the
    // export must leave out.
    users::issue_reset_code(
        &fx.pool,
        "taylor.trainee",
        users::ResetOrigin::Administrator {
            issued_by: fx.admin_id,
        },
    )
    .await
    .expect("call")
    .expect("issued");
    fx.login("avery.admin").await;
    // A session as an HTTPS deployment creates it, so the Secure marker
    // table holds a row too.
    consolebook_server::sessions::create(
        &fx.pool,
        fx.admin_id,
        consolebook_server::sessions::CookieTransport::Secure,
    )
    .await
    .expect("secure session");

    // Retention administration.
    retention::set_authority(
        &fx.pool,
        fx.admin_id,
        fx.admin_id,
        true,
        "Invented delegation.",
    )
    .await
    .expect("call")
    .expect("granted");
    retention::create_policy(
        &fx.pool,
        fx.admin_id,
        &PolicyInput {
            record_class: RecordClass::DailyReport,
            expected_current_id: None,
            authority: "INVENTED-SCHEDULE-1".into(),
            retention_trigger: RetentionTrigger::FinalizedAt,
            retention_days: 365,
            action: RetentionAction::Destroy,
            reason: "Invented schedule approval.".into(),
        },
    )
    .await
    .expect("call")
    .expect("policy");

    // Configuration: a published program and a spare draft version.
    let content = program_content("Example County Program");
    let program_id = programs::create_program(&fx.pool, fx.admin_id, &content.name)
        .await
        .expect("call")
        .expect("program");
    let version_id = programs::create_version(&fx.pool, fx.admin_id, program_id, &content)
        .await
        .expect("call")
        .expect("version");
    programs::publish_version(&fx.pool, fx.admin_id, version_id)
        .await
        .expect("call")
        .expect("published");
    let spare = program_content("Example County Spare Program");
    let spare_program = programs::create_program(&fx.pool, fx.admin_id, &spare.name)
        .await
        .expect("call")
        .expect("program");
    let spare_version_id = programs::create_version(&fx.pool, fx.admin_id, spare_program, &spare)
        .await
        .expect("call")
        .expect("version");

    // Training.
    let enrollment_id = enrollments::enroll(&fx.pool, fx.admin_id, version_id, taylor_id)
        .await
        .expect("call")
        .expect("enrolled");
    for trainer in [jordan_id, rowan_id] {
        assignments::create(&fx.pool, fx.admin_id, enrollment_id, trainer)
            .await
            .expect("call")
            .expect("assigned");
    }
    let phase_one = fx
        .id(
            "SELECT id FROM phase WHERE name = 'Phase One' AND program_version_id = ?1",
            version_id,
        )
        .await;
    lifecycle::record_phase_event(
        &fx.pool,
        casey_id,
        enrollment_id,
        PhaseEventKind::Advance,
        Some(phase_one),
        None,
        "",
    )
    .await
    .expect("call")
    .expect("entered");
    let session_id = session_for(&fx, jordan_id, enrollment_id, "2026-06-02").await;
    session_membership::add_trainer(&fx.pool, fx.admin_id, session_id, rowan_id)
        .await
        .expect("call")
        .expect("added");
    let task_id = fx
        .id(
            "SELECT id FROM task WHERE program_version_id = ?1",
            version_id,
        )
        .await;
    task_signoffs::record(
        &fx.pool,
        jordan_id,
        enrollment_id,
        task_id,
        SignoffKind::Observed,
        "",
    )
    .await
    .expect("call")
    .expect("signed");
    let modifier_id = fx
        .id(
            "SELECT id FROM rating_modifier WHERE program_version_id = ?1",
            version_id,
        )
        .await;

    // A daily report through a whole review cycle, sealed, acknowledged,
    // amended, and sealed again.
    let record_id = evaluation_drafts::create(&fx.pool, jordan_id, session_id, None)
        .await
        .expect("call")
        .expect("draft");
    let revision = save(
        &fx,
        jordan_id,
        record_id,
        3,
        Some(modifier_id),
        "Invented first pass.",
    )
    .await;
    evaluation_drafts::submit(&fx.pool, jordan_id, record_id, revision)
        .await
        .expect("call")
        .expect("submitted");
    draft_review::decide(
        &fx.pool,
        casey_id,
        record_id,
        ReviewDecisionKind::ChangesRequested,
        Some("Add the invented callback detail."),
    )
    .await
    .expect("call")
    .expect("decided");
    let revision = save(
        &fx,
        jordan_id,
        record_id,
        4,
        Some(modifier_id),
        "Invented callback verified.",
    )
    .await;
    evaluation_drafts::submit(&fx.pool, jordan_id, record_id, revision)
        .await
        .expect("call")
        .expect("resubmitted");
    draft_review::decide(
        &fx.pool,
        casey_id,
        record_id,
        ReviewDecisionKind::Approved,
        None,
    )
    .await
    .expect("call")
    .expect("approved");
    finalization::finalize(&fx.pool, casey_id, record_id, revision)
        .await
        .expect("call")
        .expect("sealed");

    // A weekly summary covering the sealed daily.
    let daily_v1 = fx
        .id(
            "SELECT id FROM evaluation_version WHERE evaluation_record_id = ?1",
            record_id,
        )
        .await;
    let summary_record_id = summaries::create(&fx.pool, jordan_id, enrollment_id, None)
        .await
        .expect("call")
        .expect("summary");
    let revision = summaries::add_link(&fx.pool, jordan_id, summary_record_id, daily_v1, 0)
        .await
        .expect("call")
        .expect("linked");
    finalization::finalize(&fx.pool, casey_id, summary_record_id, revision)
        .await
        .expect("call")
        .expect("summary sealed");

    acknowledgments::acknowledge(
        &fx.pool,
        taylor_id,
        record_id,
        TraineeAckKind::Acknowledged,
        "",
    )
    .await
    .expect("call")
    .expect("acknowledged");
    amendments::open(
        &fx.pool,
        casey_id,
        record_id,
        "The invented rating was one point low.",
    )
    .await
    .expect("call")
    .expect("opened");
    let revision = save(
        &fx,
        jordan_id,
        record_id,
        5,
        None,
        "Corrected the invented rating.",
    )
    .await;
    finalization::finalize(&fx.pool, casey_id, record_id, revision)
        .await
        .expect("call")
        .expect("amendment sealed");

    // An open draft handed to another trainer.
    let second_session = session_for(&fx, jordan_id, enrollment_id, "2026-06-03").await;
    let open_draft = evaluation_drafts::create(&fx.pool, jordan_id, second_session, None)
        .await
        .expect("call")
        .expect("draft");
    save(
        &fx,
        jordan_id,
        open_draft,
        2,
        Some(modifier_id),
        "Invented work in progress.",
    )
    .await;
    evaluation_drafts::transfer(&fx.pool, casey_id, open_draft, rowan_id)
        .await
        .expect("call")
        .expect("transferred");

    // A hold, replaced, and its replacement released.
    let hold = HoldInput {
        scope: HoldScope::Record { record_id },
        kind: HoldKind::PublicRecordsRequest,
        authority: "INVENTED-REQUEST-7".into(),
        reason: "Invented preservation instruction.".into(),
    };
    let first = retention::create_hold(&fx.pool, fx.admin_id, &hold)
        .await
        .expect("call")
        .expect("hold");
    let second = retention::replace_hold(&fx.pool, fx.admin_id, first, &hold)
        .await
        .expect("call")
        .expect("replaced");
    retention::release_hold(&fx.pool, fx.admin_id, second, "Invented request closed.")
        .await
        .expect("call")
        .expect("released");

    lifecycle::record_enrollment_event(
        &fx.pool,
        casey_id,
        enrollment_id,
        EnrollmentEventKind::Withdraw,
        "Invented transfer to another center.",
        None,
    )
    .await
    .expect("call")
    .expect("withdrawn");

    Populated {
        fx,
        taylor_id,
        jordan_id,
        rowan_id,
        casey_id,
        spare_version_id,
        amended_record_id: record_id,
        summary_record_id,
    }
}

async fn export(fx: &Fixture, actor: i64) -> Vec<u8> {
    installation_export::export_at(&fx.pool, actor, EXPORTED_AT)
        .await
        .expect("call")
        .expect("exported")
        .bytes
}

fn entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut out = Vec::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).expect("entry");
        let mut content = Vec::new();
        file.read_to_end(&mut content).expect("read");
        out.push((file.name().to_owned(), content));
    }
    out
}

fn repack(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, content) in entries {
        writer.start_file(name.as_str(), options).expect("start");
        writer.write_all(content).expect("write");
    }
    writer.finish().expect("finish").into_inner()
}

fn entry<'a>(entries: &'a [(String, Vec<u8>)], name: &str) -> &'a [u8] {
    &entries
        .iter()
        .find(|(entry_name, _)| entry_name == name)
        .unwrap_or_else(|| panic!("entry {name}"))
        .1
}

fn manifest_of(entries: &[(String, Vec<u8>)]) -> InstallationManifest {
    serde_json::from_slice(entry(entries, ARCHIVE_MANIFEST_PATH)).expect("manifest")
}

fn rows_of(entries: &[(String, Vec<u8>)], table: &str) -> Vec<serde_json::Value> {
    serde_json::from_slice(entry(entries, &installation_export::table_path(table))).expect("rows")
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// Every finding anywhere in a report.
fn all_findings(report: &ArchiveReport) -> Vec<Finding> {
    let mut findings = report.findings.clone();
    for unit in &report.units {
        findings.extend(unit.findings.iter().cloned());
    }
    for table in &report.tables {
        findings.extend(table.findings.iter().cloned());
    }
    findings
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn the_export_carries_every_table_and_verifies() {
    let p = populate().await;
    let fx = &p.fx;

    // Every carried table holds at least one row, reached through the
    // product: no table's document is proven only empty.
    for table in &spec::TABLES {
        assert!(fx.count(table.name).await > 0, "{} has no row", table.name);
    }

    let produced = installation_export::export_at(&fx.pool, fx.admin_id, EXPORTED_AT)
        .await
        .expect("call")
        .expect("exported");
    assert_eq!(
        produced.file_name,
        "consolebook-installation-export-20260901T190000Z.zip"
    );
    let listed = entries(&produced.bytes);
    let manifest = manifest_of(&listed);
    assert_eq!(manifest.format, INSTALLATION_FORMAT);
    assert_eq!(manifest.format_version, INSTALLATION_FORMAT_VERSION);
    assert_eq!(
        manifest.installation_id,
        storage::installation_id(&fx.pool).await.expect("id")
    );
    assert_eq!(manifest.exported_at, EXPORTED_AT);

    // Entry order: the manifest, the units, the documents by name.
    let names: Vec<&str> = listed.iter().map(|(name, _)| name.as_str()).collect();
    let mut expected = vec![ARCHIVE_MANIFEST_PATH.to_owned()];
    for unit in &manifest.units {
        expected.push(format!("{}/record.json", unit.path));
        expected.push(format!("{}/manifest.json", unit.path));
    }
    for table in &spec::TABLES {
        expected.push(installation_export::table_path(table.name));
    }
    assert_eq!(names, expected);

    // The ledger: 43 tables with their counts and hashes, and the five
    // exclusions.
    assert_eq!(manifest.tables.len(), 43);
    for (stated, table) in manifest.tables.iter().zip(&spec::TABLES) {
        assert_eq!(stated.name, table.name);
        assert_eq!(stated.rows, fx.count(table.name).await, "{}", table.name);
        assert_eq!(
            stated.sha256,
            canonical::content_hash_hex(entry(&listed, &stated.path))
        );
    }
    let excluded: Vec<(&str, &str)> = manifest
        .excluded
        .iter()
        .map(|entry| (entry.name.as_str(), entry.reason.as_str()))
        .collect();
    assert_eq!(
        excluded,
        [
            ("password_reset_code", "credential"),
            ("session", "credential"),
            ("session_secure_cookie", "credential"),
            ("setup_code", "credential"),
            ("user.password_hash", "credential"),
        ]
    );

    // The units are every finalized version, superseded originals
    // included, listed exactly as the record export lists them.
    let versions: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT evaluation_record_id, version_number FROM evaluation_version
         ORDER BY evaluation_record_id, version_number",
    )
    .fetch_all(&fx.pool)
    .await
    .expect("versions");
    let units: Vec<(i64, i64)> = manifest
        .units
        .iter()
        .map(|unit| (unit.record_id, unit.version_number))
        .collect();
    assert_eq!(units, versions);
    assert_eq!(units.len(), 3);
    assert_eq!(produced.unit_count, 3);
    let records = consolebook_server::record_export::export_at(
        &fx.pool,
        fx.admin_id,
        consolebook_server::record_export::Scope::Installation,
        EXPORTED_AT,
    )
    .await
    .expect("call")
    .expect("record export");
    let record_entries = entries(&records.bytes);
    let record_manifest: serde_json::Value =
        serde_json::from_slice(entry(&record_entries, ARCHIVE_MANIFEST_PATH)).expect("manifest");
    assert_eq!(
        serde_json::to_value(&manifest.units).expect("units"),
        record_manifest["units"]
    );
    for (name, content) in &record_entries {
        if name.starts_with("records/") {
            assert_eq!(entry(&listed, name), content.as_slice(), "{name}");
        }
    }

    // Typed rows: flags as booleans, nullable columns present as null,
    // text unchanged, `canonical_bytes` and the password hash absent.
    let narratives = rows_of(&listed, "form_narrative");
    assert_eq!(narratives[0]["required"], serde_json::Value::Bool(false));
    let users = rows_of(&listed, "user");
    assert!(users.iter().all(|row| row.get("password_hash").is_none()));
    assert_eq!(users[0]["username"], "avery.admin");
    let versions_doc = rows_of(&listed, "evaluation_version");
    assert!(
        versions_doc
            .iter()
            .all(|row| row.get("canonical_bytes").is_none())
    );
    assert!(versions_doc[0]["predecessor_id"].is_null());
    let audits = rows_of(&listed, "audit_event");
    let own = audits.last().expect("audit rows");
    assert_eq!(
        own["kind"], "installation_exported",
        "the export's own audit"
    );
    assert_eq!(own["actor_user_id"], fx.admin_id);
    let grants = rows_of(&listed, "capability_grant");
    assert!(grants.windows(2).all(|pair| {
        (pair[0]["user_id"].as_i64(), pair[0]["capability"].as_str())
            < (pair[1]["user_id"].as_i64(), pair[1]["capability"].as_str())
    }));

    let report = export_verify::verify_archive(&produced.bytes);
    assert!(report.verified(), "{:#?}", all_findings(&report));
    assert_eq!(report.kind, Some(ArchiveKind::InstallationExport));
    assert_eq!(report.units.len(), 3);
    assert_eq!(report.tables.len(), 43);
    for table in &report.tables {
        let stated = manifest
            .tables
            .iter()
            .find(|entry| entry.name == table.table)
            .expect("listed");
        assert_eq!(
            table.rows.and_then(|rows| i64::try_from(rows).ok()),
            Some(stated.rows)
        );
    }
    let _ = (p.taylor_id, p.jordan_id, p.rowan_id, p.casey_id);
}

#[tokio::test]
async fn only_export_records_holders_export_and_each_export_is_audited() {
    let p = populate().await;
    let fx = &p.fx;
    let before = fx.installation_audits().await.len();
    for actor in [p.taylor_id, p.jordan_id, p.casey_id] {
        let refused = installation_export::export_at(&fx.pool, actor, EXPORTED_AT)
            .await
            .expect("call")
            .expect_err("refused");
        assert_eq!(refused, ExportRefusal::CapabilityRequired);
    }
    assert_eq!(
        fx.installation_audits().await.len(),
        before,
        "a refusal is not audited"
    );
    export(fx, fx.admin_id).await;
    let audits = fx.installation_audits().await;
    assert_eq!(audits.len(), before + 1);
    assert_eq!(
        audits.last().expect("audit"),
        &(Some(fx.admin_id), None, None, None),
        "the actor, and no subject or content"
    );

    // The route answers the same way.
    let cookie = fx.login("casey.coordinator").await;
    let (status, _, body) = get(fx.app(), "/api/exports/installation", &cookie).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(body["error"], "capability_required", "{body}");
    assert_eq!(fx.installation_audits().await.len(), before + 1);

    let cookie = fx.login("avery.admin").await;
    let (status, headers, body) = get(fx.app(), "/api/exports/installation", &cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[CONTENT_TYPE], "application/zip");
    let disposition = headers[CONTENT_DISPOSITION].to_str().expect("ascii");
    assert!(
        disposition.starts_with("attachment; filename=\"consolebook-installation-export-"),
        "{disposition}"
    );
    let report = export_verify::verify_archive(&body);
    assert!(report.verified(), "{:#?}", all_findings(&report));
    assert_eq!(report.kind, Some(ArchiveKind::InstallationExport));
    assert_eq!(fx.installation_audits().await.len(), before + 2);
}

#[tokio::test]
async fn an_installation_without_finalized_versions_still_exports() {
    let fx = Fixture::new().await;
    let bytes = export(&fx, fx.admin_id).await;
    let listed = entries(&bytes);
    assert!(manifest_of(&listed).units.is_empty());
    let report = export_verify::verify_archive(&bytes);
    assert!(report.verified(), "{:#?}", all_findings(&report));
}

#[tokio::test]
async fn the_cli_prints_a_line_per_unit_and_table_document() {
    let p = populate().await;
    let bytes = export(&p.fx, p.fx.admin_id).await;
    let scratch = tempfile::tempdir().expect("scratch");
    let archive = scratch.path().join("installation.zip");
    std::fs::write(&archive, &bytes).expect("write");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_consolebook-server"))
        .args(["export", "verify"])
        .arg(&archive)
        .output()
        .expect("run verifier");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "stdout: {stdout}");
    assert!(
        stdout.contains("kind          installation export"),
        "{stdout}"
    );
    for table in &spec::TABLES {
        assert!(
            stdout.contains(&format!(
                "ok    {:<36} {} table",
                installation_export::table_path(table.name),
                table.name
            )),
            "{} line: {stdout}",
            table.name
        );
    }
    assert!(
        stdout.contains("verified 3 of 3 units and 43 of 43 documents"),
        "{stdout}"
    );
    let _ = (p.spare_version_id, p.amended_record_id, p.summary_record_id);
}

async fn get(app: axum::Router, uri: &str, cookie: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(
                    COOKIE,
                    format!("{}={}", consolebook_server::http::SESSION_COOKIE, cookie),
                )
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    (status, headers, bytes)
}
