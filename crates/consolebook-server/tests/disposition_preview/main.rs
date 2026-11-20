//! Separate disposition authority (`execute_disposition`) and read-only
//! exact-scope disposition preview (ADR 0020; ADR 0030; #211).
//! All agencies, people, schedules, authorities, and reasons are invented.

#[path = "../support/mod.rs"]
mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use consolebook_server::{
    acknowledgments::{self, TraineeAckKind},
    amendments, assignments,
    backup_copy::BackupSettings,
    capabilities::{self, Capability, RoleBundle},
    data_dir::DataDir,
    enrollments, evaluation_drafts, finalization, http,
    lifecycle::{self, EnrollmentEventKind},
    programs,
    retention::{
        self, ControlledCopyInventory, DispositionBlocker, DispositionPreviewInput, HoldInput,
        HoldKind, HoldScope, PREVIEW_VALIDITY_SECONDS, PolicyInput, RecordClass,
        RecoveryObligation, RetentionAction, RetentionCapability, RetentionRefusal,
        RetentionTrigger, UncontrolledCopyCategory,
    },
    storage, summaries, training_sessions, users,
};
use http_body_util::BodyExt;
use sqlx::SqlitePool;
use tower::ServiceExt;

const SECONDS_PER_DAY: i64 = 86_400;

struct Fixture {
    tmp: tempfile::TempDir,
    pool: SqlitePool,
    admin_id: i64,
    retention_admin_id: i64,
    executor_id: i64,
    second_executor_id: i64,
    instructor_id: i64,
    trainee_id: i64,
    program_version_id: i64,
    daily_form_id: i64,
    weekly_form_id: i64,
}

impl Fixture {
    #[allow(clippy::too_many_lines)]
    async fn new() -> Self {
        let tmp = tempfile::tempdir().expect("scratch");
        let pool = storage::open(&tmp.path().join("consolebook.db"))
            .await
            .expect("migrate");

        let mut tx = storage::write_tx(&pool).await.expect("seed users");
        let hash = consolebook_server::secrets::hash_password(support::password()).expect("hash");

        let mut ids = Vec::new();
        for (username, display_name, bundle) in [
            ("avery.admin", "Avery Admin", RoleBundle::Administrator),
            (
                "morgan.retention",
                "Morgan Retention",
                RoleBundle::Coordinator,
            ),
            ("casey.executor", "Casey Executor", RoleBundle::Coordinator),
            ("riley.executor", "Riley Executor", RoleBundle::Coordinator),
            ("jordan.trainer", "Jordan Trainer", RoleBundle::Trainer),
            ("taylor.trainee", "Taylor Trainee", RoleBundle::Trainee),
        ] {
            let id = users::create(&mut tx, username, display_name, "", "", &hash)
                .await
                .expect("create user");
            capabilities::grant_bundle(&mut tx, id, bundle.capabilities(), None)
                .await
                .expect("grant bundle");
            ids.push(id);
        }
        let [
            admin_id,
            retention_admin_id,
            executor_id,
            second_executor_id,
            instructor_id,
            trainee_id,
        ] = ids[..]
        else {
            unreachable!()
        };
        // Admin also gets review_evaluation so test helpers can finalize cleanly.
        capabilities::grant_bundle(&mut tx, admin_id, &[Capability::ReviewEvaluation], None)
            .await
            .expect("grant review");
        tx.commit().await.expect("commit users");

        let content: programs::VersionContent = serde_json::from_value(serde_json::json!({
            "name": "Invented County CTO Program",
            "label": "2026 rev A",
            "description": "Invented program for disposition preview tests.",
            "phases": [],
            "phase_transitions": [],
            "competencies": [],
            "rating_scales": [],
            "rating_modifiers": [],
            "citations": [],
            "evaluation_forms": [
                {
                    "record_type": "daily_report",
                    "name": "Invented Daily Report",
                    "instructions": "",
                    "competencies": [],
                    "narratives": [{"prompt": "Invented notes", "required": false}]
                },
                {
                    "record_type": "weekly_summary",
                    "name": "Invented Weekly Summary",
                    "instructions": "",
                    "competencies": [],
                    "narratives": [{"prompt": "Invented summary", "required": false}]
                }
            ],
            "finalization_policy": {
                "review_approved": false,
                "required_narratives": false,
                "ratings_complete": false
            }
        }))
        .expect("version content");

        let program_id = programs::create_program(&pool, admin_id, &content.name)
            .await
            .expect("call")
            .expect("program");
        let program_version_id = programs::create_version(&pool, admin_id, program_id, &content)
            .await
            .expect("call")
            .expect("version");
        programs::publish_version(&pool, admin_id, program_version_id)
            .await
            .expect("call")
            .expect("publish");

        let daily_form_id: i64 = sqlx::query_scalar(
            "SELECT id FROM evaluation_form WHERE program_version_id = ?1 AND record_type = 'daily_report'",
        )
        .bind(program_version_id)
        .fetch_one(&pool)
        .await
        .expect("daily form");
        let weekly_form_id: i64 = sqlx::query_scalar(
            "SELECT id FROM evaluation_form WHERE program_version_id = ?1 AND record_type = 'weekly_summary'",
        )
        .bind(program_version_id)
        .fetch_one(&pool)
        .await
        .expect("weekly form");

        Self {
            tmp,
            pool,
            admin_id,
            retention_admin_id,
            executor_id,
            second_executor_id,
            instructor_id,
            trainee_id,
            program_version_id,
            daily_form_id,
            weekly_form_id,
        }
    }

    async fn grant_retention_admin(&self) {
        retention::set_authority(
            &self.pool,
            self.admin_id,
            self.retention_admin_id,
            true,
            "Invented retention administrator appointment",
        )
        .await
        .expect("call")
        .expect("grant manage_retention");
    }

    async fn grant_executor(&self, user_id: i64) {
        retention::set_disposition_authority(
            &self.pool,
            self.admin_id,
            user_id,
            true,
            "Invented disposition officer appointment",
        )
        .await
        .expect("call")
        .expect("grant execute_disposition");
    }

    async fn enroll(&self, trainee_id: i64) -> i64 {
        let enrollment_id = enrollments::enroll(
            &self.pool,
            self.admin_id,
            self.program_version_id,
            trainee_id,
        )
        .await
        .expect("call")
        .expect("enrollment");
        assignments::create(&self.pool, self.admin_id, enrollment_id, self.instructor_id)
            .await
            .expect("call")
            .expect("assign trainer");
        enrollment_id
    }

    async fn create_session(&self, enrollment_id: i64, date: &str) -> i64 {
        training_sessions::create(
            &self.pool,
            self.admin_id,
            enrollment_id,
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
        .expect("session")
    }

    async fn create_finalized_daily(&self, enrollment_id: i64, date: &str) -> (i64, i64, i64) {
        let session_id = self.create_session(enrollment_id, date).await;
        let record_id = evaluation_drafts::create(
            &self.pool,
            self.instructor_id,
            session_id,
            Some(self.daily_form_id),
        )
        .await
        .expect("call")
        .expect("draft");
        let meta = finalization::finalize(&self.pool, self.admin_id, record_id, 0)
            .await
            .expect("call")
            .expect("finalize");
        let version_id: i64 = sqlx::query_scalar(
            "SELECT id FROM evaluation_version WHERE evaluation_record_id = ?1 AND version_number = ?2",
        )
        .bind(record_id)
        .bind(meta.version_number)
        .fetch_one(&self.pool)
        .await
        .expect("version id");
        (record_id, version_id, meta.finalized_at)
    }

    async fn create_weekly_draft(&self, enrollment_id: i64) -> i64 {
        summaries::create(
            &self.pool,
            self.instructor_id,
            enrollment_id,
            Some(self.weekly_form_id),
        )
        .await
        .expect("call")
        .expect("weekly draft")
    }

    async fn create_policy(
        &self,
        record_class: RecordClass,
        expected_current_id: Option<i64>,
        trigger: RetentionTrigger,
        days: i64,
        action: RetentionAction,
        authority: &str,
    ) -> i64 {
        retention::create_policy(
            &self.pool,
            self.retention_admin_id,
            &PolicyInput {
                record_class,
                expected_current_id,
                authority: authority.into(),
                retention_trigger: trigger,
                retention_days: days,
                action,
                reason: "Invented schedule adoption".into(),
            },
        )
        .await
        .expect("call")
        .expect("policy")
    }

    async fn create_hold(
        &self,
        scope: HoldScope,
        kind: HoldKind,
        authority: &str,
        reason: &str,
    ) -> i64 {
        retention::create_hold(
            &self.pool,
            self.retention_admin_id,
            &HoldInput {
                scope,
                kind,
                authority: authority.into(),
                reason: reason.into(),
            },
        )
        .await
        .expect("call")
        .expect("hold")
    }
}

mod authority;
mod confirmation;
mod eligibility;
