//! Disposition-aware record exports (v2), legacy v1 compatibility, file-only
//! verification, CLI output honesty, and real-server streaming race proof
//! (ADR 0030 §7; docs/formats/record-export.md; #215).
//! All agencies, people, schedules, narratives, and reasons are invented.

#[path = "../support/mod.rs"]
mod support;

mod golden;
mod golden_and_negative;
mod streaming_and_cli;

use std::io::{Cursor, Read, Write};
use std::path::PathBuf;

use consolebook_server::backup_copy::BackupSettings;
use consolebook_server::capabilities::{self, RoleBundle};
use consolebook_server::data_dir::DataDir;
use consolebook_server::disposition_evidence::DispositionReasonCode;
use consolebook_server::draft_content::{self, DraftContent, NarrativeEntry, RatingEntry};
use consolebook_server::programs::{
    self, AnchorDef, CompetencyDef, FormCompetencyDef, FormDef, NarrativeDef, PolicyDef,
    RecordType, ScaleDef, ScaleKind, TaskDef, VersionContent,
};
use consolebook_server::retention::{
    self, DispositionExecutionInput, DispositionPreviewInput, PolicyInput, RecordClass,
    RetentionAction, RetentionTrigger,
};
use consolebook_server::training_sessions::{self, Disposition, SessionInput};
use consolebook_server::{
    amendments, assignments, canonical, enrollments, evaluation_drafts, finalization, secrets,
    setup, storage, users,
};
use sqlx::SqlitePool;
use zip::write::{SimpleFileOptions, ZipWriter};

pub(crate) const SECONDS_PER_DAY: i64 = 86_400;
pub(crate) const EXPORTED_AT: i64 = 1_800_000_000;

const OPEN_POLICY: PolicyDef = PolicyDef {
    review_approved: false,
    required_narratives: false,
    ratings_complete: false,
};

pub(crate) struct DispExportFixture {
    pub _tmp: tempfile::TempDir,
    pub data_dir: DataDir,
    pub pool: SqlitePool,
    pub admin_id: i64,
    pub executor_id: i64,
    pub instructor_id: i64,
    pub outsider_id: i64,
    pub other_instructor_id: i64,
    pub trainee_id: i64,
    pub enrollment_id: i64,
    pub other_enrollment_id: i64,
    pub daily_form_id: i64,
    pub form_comp_id: i64,
    pub form_narr_id: i64,
}

impl DispExportFixture {
    #[allow(clippy::too_many_lines)]
    pub async fn new() -> Self {
        let tmp = tempfile::tempdir().expect("scratch");
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
        let mut ids = Vec::new();
        for (username, display_name, bundle) in [
            ("casey.executor", "Casey Executor", RoleBundle::Coordinator),
            ("jordan.trainer", "Jordan Trainer", RoleBundle::Trainer),
            ("robin.outsider", "Robin Outsider", RoleBundle::Trainer),
            ("taylor.trainee", "Taylor Trainee", RoleBundle::Trainee),
            ("morgan.trainee", "Morgan Trainee", RoleBundle::Trainee),
            ("riley.trainer", "Riley Trainer", RoleBundle::Trainer),
        ] {
            let id = users::create(&mut tx, username, display_name, "", "", &hash)
                .await
                .expect("user");
            capabilities::grant_bundle(&mut tx, id, bundle.capabilities(), Some(admin_id))
                .await
                .expect("bundle");
            ids.push(id);
        }
        tx.commit().await.expect("commit users");
        let [
            executor_id,
            instructor_id,
            outsider_id,
            trainee_id,
            other_trainee_id,
            other_instructor_id,
        ] = ids[..]
        else {
            unreachable!()
        };

        retention::set_authority(
            &pool,
            admin_id,
            admin_id,
            true,
            "Invented retention administrator appointment",
        )
        .await
        .expect("call")
        .expect("grant manage_retention");
        retention::set_disposition_authority(
            &pool,
            admin_id,
            executor_id,
            true,
            "Invented disposition officer appointment",
        )
        .await
        .expect("call")
        .expect("grant execute_disposition");

        let content = VersionContent {
            name: "Cedar Ridge CTO Program".to_owned(),
            label: "2026 A".to_owned(),
            description: "Invented program for disposition export tests.".to_owned(),
            phases: Vec::new(),
            phase_transitions: Vec::new(),
            competencies: vec![CompetencyDef {
                category: "Radio".to_owned(),
                name: "Call Control".to_owned(),
                description: "Invented competency".to_owned(),
                tasks: vec![TaskDef {
                    prompt: "Handle invented priority traffic".to_owned(),
                    citations: Vec::new(),
                }],
                citations: Vec::new(),
            }],
            rating_scales: vec![ScaleDef {
                name: "DOR 1-7".to_owned(),
                kind: ScaleKind::AnchoredNumeric,
                min_value: Some(1),
                max_value: Some(7),
                anchors: vec![
                    AnchorDef {
                        value: 1,
                        label: "Not Acceptable".to_owned(),
                        definition: "Invented 1".to_owned(),
                    },
                    AnchorDef {
                        value: 4,
                        label: "Acceptable".to_owned(),
                        definition: "Invented 4".to_owned(),
                    },
                    AnchorDef {
                        value: 7,
                        label: "Superior".to_owned(),
                        definition: "Invented 7".to_owned(),
                    },
                ],
            }],
            rating_modifiers: Vec::new(),
            evaluation_forms: vec![FormDef {
                record_type: RecordType::DailyReport,
                name: "Invented Daily Report".to_owned(),
                instructions: String::new(),
                competencies: vec![FormCompetencyDef {
                    competency: "Call Control".to_owned(),
                    rating_scale: "DOR 1-7".to_owned(),
                }],
                narratives: vec![NarrativeDef {
                    prompt: "Shift Observations".to_owned(),
                    required: false,
                }],
            }],
            citations: Vec::new(),
            finalization_policy: OPEN_POLICY,
        };
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
        let form_comp_id: i64 =
            sqlx::query_scalar("SELECT id FROM form_competency WHERE evaluation_form_id = ?1")
                .bind(daily_form_id)
                .fetch_one(&pool)
                .await
                .expect("form comp");
        let form_narr_id: i64 =
            sqlx::query_scalar("SELECT id FROM form_narrative WHERE evaluation_form_id = ?1")
                .bind(daily_form_id)
                .fetch_one(&pool)
                .await
                .expect("form narr");

        let enrollment_id = enrollments::enroll(&pool, admin_id, version_id, trainee_id)
            .await
            .expect("call")
            .expect("enrollment");
        let other_enrollment_id =
            enrollments::enroll(&pool, admin_id, version_id, other_trainee_id)
                .await
                .expect("call")
                .expect("other enrollment");
        assignments::create(&pool, admin_id, enrollment_id, instructor_id)
            .await
            .expect("call")
            .expect("assign trainer");
        assignments::create(&pool, admin_id, other_enrollment_id, other_instructor_id)
            .await
            .expect("call")
            .expect("assign other trainer");

        Self {
            _tmp: tmp,
            data_dir,
            pool,
            admin_id,
            executor_id,
            instructor_id,
            outsider_id,
            other_instructor_id,
            trainee_id,
            enrollment_id,
            other_enrollment_id,
            daily_form_id,
            form_comp_id,
            form_narr_id,
        }
    }

    pub fn backups(&self) -> BackupSettings {
        BackupSettings {
            data_dir: self.data_dir.clone(),
            interval_hours: 24,
            keep: 14,
            copy_dir: None,
        }
    }

    pub async fn set_destroy_policies(&self) -> (i64, i64) {
        let daily_policy_id = retention::create_policy(
            &self.pool,
            self.admin_id,
            &PolicyInput {
                record_class: RecordClass::DailyReport,
                expected_current_id: None,
                authority: "INVENTED-SCHED-DAILY-30D".to_owned(),
                retention_trigger: RetentionTrigger::FinalizedAt,
                retention_days: 30,
                action: RetentionAction::Destroy,
                reason: "Invented 30-day daily report schedule".to_owned(),
            },
        )
        .await
        .expect("call")
        .expect("daily policy");
        let meta_policy_id = retention::create_policy(
            &self.pool,
            self.admin_id,
            &PolicyInput {
                record_class: RecordClass::DispositionEvent,
                expected_current_id: None,
                authority: "INVENTED-SCHED-META-10Y".to_owned(),
                retention_trigger: RetentionTrigger::DisposedAt,
                retention_days: 3650,
                action: RetentionAction::Destroy,
                reason: "Invented 10-year disposition metadata schedule".to_owned(),
            },
        )
        .await
        .expect("call")
        .expect("meta policy");
        (daily_policy_id, meta_policy_id)
    }

    pub async fn create_finalized_daily(
        &self,
        enrollment_id: i64,
        shift_date: &str,
        narrative: &str,
        amend_text: Option<&str>,
    ) -> (i64, i64) {
        let actor = if enrollment_id == self.enrollment_id {
            self.instructor_id
        } else {
            self.other_instructor_id
        };
        let session_id = training_sessions::create(
            &self.pool,
            self.executor_id,
            enrollment_id,
            &SessionInput {
                business_date: shift_date.to_owned(),
                timezone: "UTC".to_owned(),
                local_start: format!("{shift_date}T08:00"),
                local_end: Some(format!("{shift_date}T16:00")),
                disposition: Some(Disposition::Completed),
                phase_id: None,
                trainer_user_ids: vec![actor],
            },
        )
        .await
        .expect("call")
        .expect("session");

        let record_id =
            evaluation_drafts::create(&self.pool, actor, session_id, Some(self.daily_form_id))
                .await
                .expect("call")
                .expect("draft");
        let rev = draft_content::save(
            &self.pool,
            actor,
            record_id,
            0,
            &DraftContent {
                ratings: vec![RatingEntry {
                    form_competency_id: self.form_comp_id,
                    value: Some(4),
                    not_observed: false,
                    modifier_ids: Vec::new(),
                }],
                narratives: vec![NarrativeEntry {
                    form_narrative_id: self.form_narr_id,
                    text: narrative.to_owned(),
                }],
            },
        )
        .await
        .expect("call")
        .expect("save v1");
        let mut meta = finalization::finalize(&self.pool, self.executor_id, record_id, rev)
            .await
            .expect("call")
            .expect("finalize v1");

        if let Some(second) = amend_text {
            amendments::open(
                &self.pool,
                self.executor_id,
                record_id,
                "Invented amendment",
            )
            .await
            .expect("call")
            .expect("amend");
            let cur_rev: i64 =
                sqlx::query_scalar("SELECT revision FROM evaluation_record WHERE id = ?1")
                    .bind(record_id)
                    .fetch_one(&self.pool)
                    .await
                    .expect("rev");
            let rev2 = draft_content::save(
                &self.pool,
                actor,
                record_id,
                cur_rev,
                &DraftContent {
                    ratings: vec![RatingEntry {
                        form_competency_id: self.form_comp_id,
                        value: Some(5),
                        not_observed: false,
                        modifier_ids: Vec::new(),
                    }],
                    narratives: vec![NarrativeEntry {
                        form_narrative_id: self.form_narr_id,
                        text: second.to_owned(),
                    }],
                },
            )
            .await
            .expect("call")
            .expect("save v2");
            meta = finalization::finalize(&self.pool, self.executor_id, record_id, rev2)
                .await
                .expect("call")
                .expect("finalize v2");
        }
        (record_id, meta.finalized_at)
    }

    pub async fn dispose_records(
        &self,
        record_ids: Vec<i64>,
        evaluated_at: i64,
        retain_version_hashes: bool,
    ) {
        let backups = self.backups();
        let preview = retention::preview_disposition_at(
            &self.pool,
            self.executor_id,
            &DispositionPreviewInput {
                record_ids: record_ids.clone(),
            },
            Some(&backups),
            evaluated_at,
        )
        .await
        .expect("call")
        .expect("preview");
        assert!(preview.eligible, "preview should be eligible: {preview:?}");
        let report = retention::execute_disposition_at(
            &self.pool,
            self.executor_id,
            &DispositionExecutionInput {
                record_ids,
                confirmation_digest: preview.confirmation_digest,
                expires_at: preview.expires_at,
                authority_reference: "INVENTED-DISP-AUTH-2026".to_owned(),
                reason_code: DispositionReasonCode::ScheduledRetentionElapsed,
                retain_version_hashes,
            },
            &backups,
            evaluated_at + 1,
        )
        .await
        .expect("call")
        .expect("execute");
        assert!(report.is_completed(), "{report:?}");
    }

    pub async fn insert_boundary_checkpoint(&self, meta_policy_id: i64, through: i64) -> i64 {
        // Metadata expiry (#218) has no producer yet, so the fixture writes
        // the run and checkpoint rows the schema (migration 0018) defines.
        let run_id = sqlx::query(
            "INSERT INTO disposition_run
             (actor_user_id, confirmation_digest, authority_reference, reason_code, method,
              state, started_at, completed_at)
             VALUES (?1, ?2, 'INVENTED-BOUNDARY-AUTH-2026', 'metadata_retention_elapsed',
                     'lawful_destruction', 'completed', ?3, ?3)",
        )
        .bind(self.executor_id)
        .bind("b".repeat(64))
        .bind(through + 10)
        .execute(&self.pool)
        .await
        .expect("insert boundary run")
        .last_insert_rowid();
        sqlx::query(
            "INSERT INTO disposition_boundary_checkpoint
             (disposition_run_id, metadata_policy_id, authority_reference, reason_code,
              expired_event_count, boundary_through_disposed_at, recorded_at, recorded_by)
             VALUES (?1, ?2, 'INVENTED-BOUNDARY-AUTH-2026', 'metadata_retention_elapsed', 3, ?3, ?4, ?5)",
        )
        .bind(run_id)
        .bind(meta_policy_id)
        .bind(through)
        .bind(through + 10)
        .bind(self.executor_id)
        .execute(&self.pool)
        .await
        .expect("insert boundary checkpoint")
        .last_insert_rowid()
    }
}

pub(crate) fn entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut out = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).expect("entry");
        let name = file.name().expect("entry name").into_owned();
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).expect("read");
        out.push((name, buf));
    }
    out
}

pub(crate) fn entry<'a>(entries: &'a [(String, Vec<u8>)], name: &str) -> &'a [u8] {
    entries
        .iter()
        .find(|(n, _)| n == name)
        .map_or_else(|| panic!("missing entry {name}"), |(_, b)| b.as_slice())
}

pub(crate) fn repack(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o644);
    for (name, content) in entries {
        writer.start_file(name, opts).expect("start");
        writer.write_all(content).expect("write");
    }
    writer.finish().expect("finish").into_inner()
}

pub(crate) fn edit_json(bytes: &[u8], f: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut value: serde_json::Value = serde_json::from_slice(bytes).expect("json");
    f(&mut value);
    canonical::canonical_bytes(&value).expect("canonical")
}

pub(crate) fn with_entry(
    listed: &[(String, Vec<u8>)],
    name: &str,
    transform: impl FnOnce(&[u8]) -> Option<Vec<u8>>,
) -> Vec<(String, Vec<u8>)> {
    let mut transform = Some(transform);
    let mut out = Vec::with_capacity(listed.len());
    for (n, b) in listed {
        if n == name {
            if let Some(f) = transform.take()
                && let Some(updated) = f(b)
            {
                out.push((n.clone(), updated));
            }
        } else {
            out.push((n.clone(), b.clone()));
        }
    }
    out
}

pub(crate) fn rehash_evidence(
    listed: &[(String, Vec<u8>)],
    evidence_bytes: Vec<u8>,
) -> Vec<(String, Vec<u8>)> {
    let hash = canonical::content_hash_hex(&evidence_bytes);
    let updated = with_entry(
        listed,
        consolebook_server::record_export::DISPOSITION_EVIDENCE_PATH,
        |_| Some(evidence_bytes),
    );
    with_entry(
        &updated,
        consolebook_server::record_export::ARCHIVE_MANIFEST_PATH,
        |bytes| {
            Some(edit_json(bytes, |m| {
                m["disposition_evidence"]["sha256"] = serde_json::Value::String(hash);
            }))
        },
    )
}

pub(crate) fn server_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_consolebook-server"))
}
