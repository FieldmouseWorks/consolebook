//! Confirmed lawful disposition execution, raw residual-byte scanning,
//! crash/cleanup fault injection, idempotent retry, and confirmation race
//! verification (ADR 0030; #214).
//! All agencies, people, schedules, authorities, narratives, and reasons are invented.

#[path = "../support/mod.rs"]
mod support;

mod content_scan;
mod faults_and_races;

use std::path::{Path, PathBuf};

use consolebook_server::acknowledgments::{self, TraineeAckKind};
use consolebook_server::backup_copy::BackupSettings;
use consolebook_server::capabilities::{self, RoleBundle};
use consolebook_server::data_dir::DataDir;
use consolebook_server::draft_content::{DraftContent, NarrativeEntry, RatingEntry};
use consolebook_server::draft_review::{self, ReviewDecisionKind};
use consolebook_server::retention::{
    self, PolicyInput, RecordClass, RetentionAction, RetentionTrigger,
};
use consolebook_server::task_signoffs::{self, SignoffKind};
use consolebook_server::{
    amendments, assignments, draft_content, enrollments, evaluation_drafts, finalization, programs,
    secrets, setup, storage, summaries, training_sessions, users,
};
use sqlx::SqlitePool;

pub(crate) const SECONDS_PER_DAY: i64 = 86_400;

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

pub(crate) fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

pub(crate) struct ExecFixture {
    pub tmp: tempfile::TempDir,
    pub other_fs: tempfile::TempDir,
    pub data_dir: DataDir,
    pub copy_dir: PathBuf,
    pub pool: SqlitePool,
    pub admin_id: i64,
    pub executor_id: i64,
    pub instructor_id: i64,
    pub reviewer_id: i64,
    pub trainee_id: i64,
    pub enrollment_id: i64,
    pub daily_form_id: i64,
    pub weekly_form_id: i64,
    pub form_comp_id: i64,
    pub form_narr_id: i64,
    pub modifier_id: i64,
}

impl ExecFixture {
    #[allow(clippy::too_many_lines)]
    pub async fn new() -> Self {
        let tmp = tempfile::tempdir().expect("scratch");
        let other_fs = other_filesystem(tmp.path());
        let data_dir = DataDir::new(tmp.path().join("data"));
        data_dir.ensure_layout().expect("layout");
        let copy_dir = other_fs.path().join("offsite");
        std::fs::create_dir_all(&copy_dir).expect("copy dir");

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
            ("sam.reviewer", "Sam Reviewer", RoleBundle::Coordinator),
            ("taylor.trainee", "Taylor Trainee", RoleBundle::Trainee),
        ] {
            let id = users::create(&mut tx, username, display_name, "", "", &hash)
                .await
                .expect("user");
            capabilities::grant_bundle(&mut tx, id, bundle.capabilities(), Some(admin_id))
                .await
                .expect("bundle");
            ids.push(id);
        }
        let [executor_id, instructor_id, reviewer_id, trainee_id] = ids[..] else {
            unreachable!()
        };
        tx.commit().await.expect("commit users");

        // Grant separate manage_retention to admin and execute_disposition to executor.
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

        let content: programs::VersionContent = serde_json::from_value(serde_json::json!({
            "name": "Cedar Ridge Dispatch CTO",
            "label": "2026 A",
            "description": "Invented disposition execution program.",
            "phases": [],
            "phase_transitions": [],
            "competencies": [{
                "category": "Radio",
                "name": "Call Control",
                "description": "Invented competency",
                "tasks": [{"prompt": "Handle invented priority traffic", "citations": []}],
                "citations": []
            }],
            "rating_scales": [{
                "name": "DOR 1-7",
                "kind": "anchored_numeric",
                "min_value": 1,
                "max_value": 7,
                "anchors": [
                    {"value": 1, "label": "Not Acceptable", "definition": "Invented 1"},
                    {"value": 4, "label": "Acceptable", "definition": "Invented 4"},
                    {"value": 7, "label": "Superior", "definition": "Invented 7"}
                ]
            }],
            "rating_modifiers": [{
                "code": "RT",
                "label": "Remedial Training",
                "description": "Invented modifier"
            }],
            "citations": [],
            "evaluation_forms": [
                {
                    "record_type": "daily_report",
                    "name": "Invented Daily Report",
                    "instructions": "",
                    "competencies": [{"competency": "Call Control", "rating_scale": "DOR 1-7"}],
                    "narratives": [{"prompt": "Shift Observations", "required": true}]
                },
                {
                    "record_type": "weekly_summary",
                    "name": "Invented Weekly Summary",
                    "instructions": "",
                    "competencies": [],
                    "narratives": [{"prompt": "Weekly Synthesis", "required": true}]
                }
            ],
            "finalization_policy": {
                "review_approved": true,
                "required_narratives": true,
                "ratings_complete": true
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
        let weekly_form_id: i64 = sqlx::query_scalar(
            "SELECT id FROM evaluation_form WHERE program_version_id = ?1 AND record_type = 'weekly_summary'",
        )
        .bind(version_id)
        .fetch_one(&pool)
        .await
        .expect("weekly form");
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
        let modifier_id: i64 =
            sqlx::query_scalar("SELECT id FROM rating_modifier WHERE program_version_id = ?1")
                .bind(version_id)
                .fetch_one(&pool)
                .await
                .expect("modifier");
        let task_id: i64 = sqlx::query_scalar("SELECT id FROM task WHERE program_version_id = ?1")
            .bind(version_id)
            .fetch_one(&pool)
            .await
            .expect("task");

        let enrollment_id = enrollments::enroll(&pool, admin_id, version_id, trainee_id)
            .await
            .expect("call")
            .expect("enroll");
        assignments::create(&pool, admin_id, enrollment_id, instructor_id)
            .await
            .expect("call")
            .expect("assign");

        // Record a task signoff to verify unrelated training records survive disposition.
        task_signoffs::record(
            &pool,
            instructor_id,
            enrollment_id,
            task_id,
            SignoffKind::Observed,
            "",
        )
        .await
        .expect("call")
        .expect("signoff");

        Self {
            tmp,
            other_fs,
            data_dir,
            copy_dir,
            pool,
            admin_id,
            executor_id,
            instructor_id,
            reviewer_id,
            trainee_id,
            enrollment_id,
            daily_form_id,
            weekly_form_id,
            form_comp_id,
            form_narr_id,
            modifier_id,
        }
    }

    pub fn backups(&self, with_copy_dir: bool) -> BackupSettings {
        BackupSettings {
            data_dir: self.data_dir.clone(),
            interval_hours: 24,
            keep: 14,
            copy_dir: with_copy_dir.then(|| self.copy_dir.clone()),
        }
    }

    pub async fn set_destroy_policy(
        &self,
        record_class: RecordClass,
        expected_current_id: Option<i64>,
        days: i64,
    ) -> i64 {
        retention::create_policy(
            &self.pool,
            self.admin_id,
            &PolicyInput {
                record_class,
                expected_current_id,
                authority: format!("INVENTED-SCHED-{}", record_class.as_str()),
                retention_trigger: if record_class == RecordClass::DispositionEvent {
                    RetentionTrigger::DisposedAt
                } else {
                    RetentionTrigger::FinalizedAt
                },
                retention_days: days,
                action: RetentionAction::Destroy,
                reason: "Invented retention schedule".into(),
            },
        )
        .await
        .expect("call")
        .expect("policy")
    }

    /// Creates a rich two-version amended daily record with ratings, modifiers,
    /// narratives, change-request snapshot, review comments, acknowledgments,
    /// amendment reason, and record-linked workflow notices — every one carrying
    /// distinct sentinel tokens derived from `tag`.
    #[allow(clippy::too_many_lines)]
    pub async fn create_rich_amended_daily(
        &self,
        date: &str,
        tag: &str,
    ) -> (i64, i64, Vec<String>) {
        let sentinels = vec![
            format!("{tag}_NARR_V1_INITIAL"),
            format!("{tag}_REVIEW_CHANGES_REQ"),
            format!("{tag}_NARR_V1_REVISED"),
            format!("{tag}_REVIEW_APPROVE_V1"),
            format!("{tag}_ACK_RESPONSE_V1"),
            format!("{tag}_AMENDMENT_REASON_V2"),
            format!("{tag}_NARR_V2_AMENDED"),
            format!("{tag}_REVIEW_APPROVE_V2"),
            format!("{tag}_ACK_RESPONSE_V2"),
        ];

        let session_id = training_sessions::create(
            &self.pool,
            self.admin_id,
            self.enrollment_id,
            &training_sessions::SessionInput {
                business_date: date.into(),
                timezone: "UTC".into(),
                local_start: format!("{date}T08:00"),
                local_end: Some(format!("{date}T16:00")),
                disposition: Some(training_sessions::Disposition::Completed),
                phase_id: None,
                trainer_user_ids: vec![self.instructor_id],
            },
        )
        .await
        .expect("call")
        .expect("session");

        let record_id = evaluation_drafts::create(
            &self.pool,
            self.instructor_id,
            session_id,
            Some(self.daily_form_id),
        )
        .await
        .expect("call")
        .expect("draft");

        let rev = draft_content::save(
            &self.pool,
            self.instructor_id,
            record_id,
            0,
            &DraftContent {
                ratings: vec![RatingEntry {
                    form_competency_id: self.form_comp_id,
                    value: Some(4),
                    not_observed: false,
                    modifier_ids: vec![self.modifier_id],
                }],
                narratives: vec![NarrativeEntry {
                    form_narrative_id: self.form_narr_id,
                    text: sentinels[0].clone(),
                }],
            },
        )
        .await
        .expect("call")
        .expect("save v1");

        evaluation_drafts::submit(&self.pool, self.instructor_id, record_id, rev)
            .await
            .expect("call")
            .expect("submit 1");
        draft_review::decide(
            &self.pool,
            self.reviewer_id,
            record_id,
            ReviewDecisionKind::ChangesRequested,
            Some(&sentinels[1]),
        )
        .await
        .expect("call")
        .expect("request changes");

        let rev = draft_content::save(
            &self.pool,
            self.instructor_id,
            record_id,
            rev,
            &DraftContent {
                ratings: vec![RatingEntry {
                    form_competency_id: self.form_comp_id,
                    value: Some(5),
                    not_observed: false,
                    modifier_ids: vec![self.modifier_id],
                }],
                narratives: vec![NarrativeEntry {
                    form_narrative_id: self.form_narr_id,
                    text: sentinels[2].clone(),
                }],
            },
        )
        .await
        .expect("call")
        .expect("save revised");

        evaluation_drafts::submit(&self.pool, self.instructor_id, record_id, rev)
            .await
            .expect("call")
            .expect("submit 2");
        draft_review::decide(
            &self.pool,
            self.reviewer_id,
            record_id,
            ReviewDecisionKind::Approved,
            Some(&sentinels[3]),
        )
        .await
        .expect("call")
        .expect("approve v1");

        finalization::finalize(&self.pool, self.reviewer_id, record_id, rev)
            .await
            .expect("call")
            .expect("finalize v1");
        acknowledgments::acknowledge(
            &self.pool,
            self.trainee_id,
            record_id,
            TraineeAckKind::AcknowledgedWithResponse,
            &sentinels[4],
        )
        .await
        .expect("call")
        .expect("ack v1");

        amendments::open(&self.pool, self.reviewer_id, record_id, &sentinels[5])
            .await
            .expect("call")
            .expect("open amendment");
        let cur_rev: i64 =
            sqlx::query_scalar("SELECT revision FROM evaluation_record WHERE id = ?1")
                .bind(record_id)
                .fetch_one(&self.pool)
                .await
                .expect("rev");
        let rev = draft_content::save(
            &self.pool,
            self.instructor_id,
            record_id,
            cur_rev,
            &DraftContent {
                ratings: vec![RatingEntry {
                    form_competency_id: self.form_comp_id,
                    value: Some(6),
                    not_observed: false,
                    modifier_ids: vec![self.modifier_id],
                }],
                narratives: vec![NarrativeEntry {
                    form_narrative_id: self.form_narr_id,
                    text: sentinels[6].clone(),
                }],
            },
        )
        .await
        .expect("call")
        .expect("save v2");

        evaluation_drafts::submit(&self.pool, self.instructor_id, record_id, rev)
            .await
            .expect("call")
            .expect("submit v2");
        draft_review::decide(
            &self.pool,
            self.reviewer_id,
            record_id,
            ReviewDecisionKind::Approved,
            Some(&sentinels[7]),
        )
        .await
        .expect("call")
        .expect("approve v2");

        let meta = finalization::finalize(&self.pool, self.reviewer_id, record_id, rev)
            .await
            .expect("call")
            .expect("finalize v2");
        acknowledgments::acknowledge(
            &self.pool,
            self.trainee_id,
            record_id,
            TraineeAckKind::AcknowledgedWithResponse,
            &sentinels[8],
        )
        .await
        .expect("call")
        .expect("ack v2");

        (record_id, meta.finalized_at, sentinels)
    }

    pub async fn create_finalized_weekly(
        &self,
        linked_daily_version_ids: &[i64],
        sentinel: &str,
    ) -> (i64, i64) {
        let weekly_narr_id: i64 =
            sqlx::query_scalar("SELECT id FROM form_narrative WHERE evaluation_form_id = ?1")
                .bind(self.weekly_form_id)
                .fetch_one(&self.pool)
                .await
                .expect("weekly narr");

        let record_id = summaries::create(
            &self.pool,
            self.instructor_id,
            self.enrollment_id,
            Some(self.weekly_form_id),
        )
        .await
        .expect("call")
        .expect("summary");

        let mut rev: i64 =
            sqlx::query_scalar("SELECT revision FROM evaluation_record WHERE id = ?1")
                .bind(record_id)
                .fetch_one(&self.pool)
                .await
                .expect("rev");
        for &daily_vid in linked_daily_version_ids {
            rev = summaries::add_link(&self.pool, self.instructor_id, record_id, daily_vid, rev)
                .await
                .expect("call")
                .expect("link daily");
        }
        let rev = draft_content::save(
            &self.pool,
            self.instructor_id,
            record_id,
            rev,
            &DraftContent {
                ratings: vec![],
                narratives: vec![NarrativeEntry {
                    form_narrative_id: weekly_narr_id,
                    text: sentinel.to_owned(),
                }],
            },
        )
        .await
        .expect("call")
        .expect("save weekly");
        evaluation_drafts::submit(&self.pool, self.instructor_id, record_id, rev)
            .await
            .expect("call")
            .expect("submit weekly");
        draft_review::decide(
            &self.pool,
            self.reviewer_id,
            record_id,
            ReviewDecisionKind::Approved,
            Some("Invented weekly approval"),
        )
        .await
        .expect("call")
        .expect("approve weekly");
        let meta = finalization::finalize(&self.pool, self.reviewer_id, record_id, rev)
            .await
            .expect("call")
            .expect("finalize weekly");
        (record_id, meta.finalized_at)
    }
}
