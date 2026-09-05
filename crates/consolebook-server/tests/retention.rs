//! Retention administration: explicit authority, versioned policy, and holds.
//! All names, schedules, authorities, and reasons are invented.
use consolebook_server::{
    capabilities::{self, Capability},
    retention::{
        self, HoldInput, HoldKind, HoldScope, PolicyInput, RecordClass, RetentionAction,
        RetentionRefusal, RetentionTrigger,
    },
    storage, users,
};
use sqlx::{ConnectOptions, Connection, SqlitePool};

struct Fixture {
    tmp: tempfile::TempDir,
    pool: SqlitePool,
}
impl Fixture {
    async fn new() -> Self {
        let tmp = tempfile::tempdir().expect("scratch");
        let pool = storage::open(&tmp.path().join("consolebook.db"))
            .await
            .expect("migrate");
        let mut tx = storage::write_tx(&pool).await.expect("seed");
        for (username, bundle) in [
            ("avery.admin", capabilities::ADMINISTRATOR_BUNDLE.as_slice()),
            ("jordan.trainer", capabilities::TRAINER_BUNDLE.as_slice()),
            ("taylor.trainee", capabilities::TRAINEE_BUNDLE.as_slice()),
        ] {
            let id = users::create(&mut tx, username, username, "", "", "invented-unused-hash")
                .await
                .expect("user");
            capabilities::grant_bundle(&mut tx, id, bundle, None)
                .await
                .expect("grants");
        }
        capabilities::grant_bundle(&mut tx, 1, &[Capability::ReviewEvaluation], None)
            .await
            .expect("review grant");
        tx.commit().await.expect("commit");
        Self { tmp, pool }
    }
    async fn authorize(&self) {
        retention::set_authority(&self.pool, 1, 2, true, "Invented delegation")
            .await
            .expect("call")
            .expect("grant");
    }
    async fn record(&self) -> (i64, i64) {
        use consolebook_server::{
            enrollments, evaluation_drafts, finalization, programs, training_sessions,
        };
        let content: programs::VersionContent = serde_json::from_value(serde_json::json!({
            "name":"Invented County Training", "label":"A", "description":"", "phases":[], "phase_transitions":[], "competencies":[], "rating_scales":[], "rating_modifiers":[], "citations":[],
            "evaluation_forms":[{"record_type":"daily_report", "name":"Invented Daily", "instructions":"", "competencies":[], "narratives":[{"prompt":"Invented observations", "required":false}]}],
            "finalization_policy":{"review_approved":false,"required_narratives":false,"ratings_complete":false}
        })).expect("content");
        let program = programs::create_program(&self.pool, 1, &content.name)
            .await
            .expect("call")
            .expect("program");
        let version = programs::create_version(&self.pool, 1, program, &content)
            .await
            .expect("call")
            .expect("version");
        programs::publish_version(&self.pool, 1, version)
            .await
            .expect("call")
            .expect("publish");
        let enrollment = enrollments::enroll(&self.pool, 1, version, 3)
            .await
            .expect("call")
            .expect("enrollment");
        let session = training_sessions::create(
            &self.pool,
            1,
            enrollment,
            &training_sessions::SessionInput {
                business_date: "2026-06-02".into(),
                timezone: "UTC".into(),
                local_start: "2026-06-02T08:00".into(),
                local_end: Some("2026-06-02T16:00".into()),
                disposition: Some(training_sessions::Disposition::Completed),
                phase_id: None,
                trainer_user_ids: vec![2],
            },
        )
        .await
        .expect("call")
        .expect("session");
        let record = evaluation_drafts::create(&self.pool, 2, session, None)
            .await
            .expect("call")
            .expect("draft");
        finalization::finalize(&self.pool, 1, record, 0)
            .await
            .expect("call")
            .expect("finalized");
        (enrollment, record)
    }
    async fn probe(&self) {
        let mut c = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(self.tmp.path().join("consolebook.db"))
            .busy_timeout(std::time::Duration::ZERO)
            .connect()
            .await
            .expect("probe");
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut c)
            .await
            .expect("released lock");
        sqlx::query("ROLLBACK")
            .execute(&mut c)
            .await
            .expect("rollback");
        c.close().await.expect("close");
    }
}
fn policy() -> PolicyInput {
    PolicyInput {
        record_class: RecordClass::DailyReport,
        expected_current_id: None,
        authority: "INVENTED-SCHEDULE-1".into(),
        retention_trigger: RetentionTrigger::FinalizedAt,
        retention_days: 365,
        action: RetentionAction::Destroy,
        reason: "Invented schedule approval".into(),
    }
}
fn hold(scope: HoldScope) -> HoldInput {
    HoldInput {
        scope,
        kind: HoldKind::Litigation,
        authority: "INVENTED-HOLD-1".into(),
        reason: "Invented preservation instruction".into(),
    }
}

#[tokio::test]
async fn authority_is_explicit_revocable_and_audited() {
    let fx = Fixture::new().await;
    for bundle in [
        capabilities::ADMINISTRATOR_BUNDLE.as_slice(),
        capabilities::COORDINATOR_BUNDLE.as_slice(),
        capabilities::TRAINER_BUNDLE.as_slice(),
        capabilities::TRAINEE_BUNDLE.as_slice(),
    ] {
        assert!(!bundle.contains(&Capability::ManageRetention));
    }
    for actor in [1, 2, 3] {
        assert_eq!(
            retention::list_policies(&fx.pool, actor)
                .await
                .expect("call"),
            Err(RetentionRefusal::CapabilityRequired)
        );
        assert_eq!(
            retention::create_policy(&fx.pool, actor, &policy())
                .await
                .expect("call"),
            Err(RetentionRefusal::CapabilityRequired)
        );
        assert_eq!(
            retention::list_holds(&fx.pool, actor).await.expect("call"),
            Err(RetentionRefusal::CapabilityRequired)
        );
    }
    assert_eq!(
        retention::set_authority(&fx.pool, 2, 2, true, "Invented self-grant")
            .await
            .expect("call"),
        Err(RetentionRefusal::CapabilityRequired)
    );
    fx.authorize().await;
    retention::create_policy(&fx.pool, 2, &policy())
        .await
        .expect("call")
        .expect("policy");
    assert_eq!(
        retention::authority_history(&fx.pool, 2)
            .await
            .expect("call"),
        Err(RetentionRefusal::CapabilityRequired)
    );
    retention::set_authority(&fx.pool, 1, 2, false, "Invented reassignment")
        .await
        .expect("call")
        .expect("revoke");
    assert_eq!(
        retention::list_policies(&fx.pool, 2).await.expect("call"),
        Err(RetentionRefusal::CapabilityRequired)
    );
    let events = retention::authority_history(&fx.pool, 1)
        .await
        .expect("call")
        .expect("history");
    assert_eq!(events.len(), 2);
    assert!(!events[0].granted);
    assert!(events[1].granted);
    assert_eq!(events[0].actor_user_id, 1);
    let audits: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_event WHERE kind IN ('retention_authority_granted', 'retention_authority_revoked') AND actor_user_id = 1 AND subject_user_id = 2").fetch_one(&fx.pool).await.expect("audit");
    assert_eq!(audits, 2);
    fx.probe().await;
}

#[tokio::test]
async fn policies_preserve_history_and_refuse_stale_replacement() {
    let fx = Fixture::new().await;
    fx.authorize().await;
    let first = retention::create_policy(&fx.pool, 2, &policy())
        .await
        .expect("call")
        .expect("policy");
    let before = retention::list_policies(&fx.pool, 2)
        .await
        .expect("call")
        .expect("history")[0]
        .clone();
    let next = PolicyInput {
        expected_current_id: Some(first),
        retention_days: 730,
        reason: "Invented schedule revision".into(),
        ..policy()
    };
    let (a, b) = tokio::join!(
        retention::create_policy(&fx.pool, 2, &next),
        retention::create_policy(&fx.pool, 2, &next)
    );
    let (a, b) = (a.expect("a"), b.expect("b"));
    assert!(matches!(
        (a, b),
        (Ok(_), Err(RetentionRefusal::StalePolicy)) | (Err(RetentionRefusal::StalePolicy), Ok(_))
    ));
    let after = retention::list_policies(&fx.pool, 2)
        .await
        .expect("call")
        .expect("history");
    assert_eq!(after.len(), 2);
    assert_eq!(after[1], before);
    assert_eq!(after[0].version_number, 2);
    assert_eq!(after[0].supersedes_id, Some(first));
    fx.probe().await;
}

#[tokio::test]
async fn policy_class_trigger_duration_and_text_contracts_hold() {
    let fx = Fixture::new().await;
    fx.authorize().await;
    for class in [
        RecordClass::DailyReport,
        RecordClass::WeeklySummary,
        RecordClass::PhaseEvaluation,
        RecordClass::DispositionEvent,
    ] {
        for trigger in [
            RetentionTrigger::FinalizedAt,
            RetentionTrigger::EnrollmentClosedAt,
            RetentionTrigger::DisposedAt,
        ] {
            let mut input = policy();
            input.record_class = class;
            input.retention_trigger = trigger;
            input.expected_current_id = retention::list_policies(&fx.pool, 2)
                .await
                .expect("call")
                .expect("list")
                .iter()
                .find(|p| p.record_class == class)
                .map(|p| p.id);
            let result = retention::create_policy(&fx.pool, 2, &input)
                .await
                .expect("call");
            assert_eq!(
                result.is_ok(),
                (class == RecordClass::DispositionEvent)
                    == (trigger == RetentionTrigger::DisposedAt)
            );
        }
    }
    for input in [
        PolicyInput {
            retention_days: -1,
            ..policy()
        },
        PolicyInput {
            retention_days: 365_251,
            ..policy()
        },
        PolicyInput {
            action: RetentionAction::Retain,
            ..policy()
        },
        PolicyInput {
            authority: "\u{2003}".into(),
            ..policy()
        },
        PolicyInput {
            reason: "x".repeat(1001),
            ..policy()
        },
    ] {
        assert!(matches!(
            retention::create_policy(&fx.pool, 2, &input)
                .await
                .expect("call"),
            Err(RetentionRefusal::Invalid(_))
        ));
    }
    let mut retain = policy();
    retain.record_class = RecordClass::DailyReport;
    retain.retention_days = 0;
    retain.action = RetentionAction::Retain;
    retain.expected_current_id = retention::list_policies(&fx.pool, 2)
        .await
        .expect("call")
        .expect("list")
        .iter()
        .find(|p| p.record_class == RecordClass::DailyReport)
        .map(|p| p.id);
    retention::create_policy(&fx.pool, 2, &retain)
        .await
        .expect("call")
        .expect("retain policy");
}

#[tokio::test]
async fn all_hold_kinds_match_only_their_exact_active_scopes() {
    let fx = Fixture::new().await;
    fx.authorize().await;
    let (enrollment, record) = fx.record().await;
    let scopes = [
        HoldScope::Installation,
        HoldScope::Enrollment {
            enrollment_id: enrollment,
        },
        HoldScope::Record { record_id: record },
    ];
    for (i, kind) in [
        HoldKind::Litigation,
        HoldKind::AnticipatedLitigation,
        HoldKind::Audit,
        HoldKind::Investigation,
        HoldKind::PublicRecordsRequest,
        HoldKind::Other,
    ]
    .into_iter()
    .enumerate()
    {
        let mut input = hold(scopes[i % 3].clone());
        input.kind = kind;
        retention::create_hold(&fx.pool, 2, &input)
            .await
            .expect("call")
            .expect("hold");
    }
    let matched = retention::active_holds_for_record(&fx.pool, 2, record)
        .await
        .expect("call")
        .expect("matches");
    assert_eq!(matched.len(), 6);
    retention::release_hold(&fx.pool, 2, matched[0].id, "Invented release")
        .await
        .expect("call")
        .expect("release");
    assert_eq!(
        retention::active_holds_for_record(&fx.pool, 2, record)
            .await
            .expect("call")
            .expect("matches")
            .len(),
        5
    );
    // Same user and program names never expand scope to another enrollment.
    let mut tx = storage::write_tx(&fx.pool).await.expect("tx");
    sqlx::query("INSERT INTO enrollment (id,user_id,program_version_id,enrolled_at) SELECT 99,2,program_version_id,1 FROM enrollment WHERE id=?1").bind(enrollment).execute(&mut *tx).await.expect("other enrollment");
    tx.commit().await.expect("commit");
    let other = retention::create_hold(
        &fx.pool,
        2,
        &hold(HoldScope::Enrollment { enrollment_id: 99 }),
    )
    .await
    .expect("call")
    .expect("other hold");
    assert!(
        !retention::active_holds_for_record(&fx.pool, 2, record)
            .await
            .expect("call")
            .expect("matches")
            .iter()
            .any(|h| h.id == other)
    );
    assert_eq!(
        retention::active_holds_for_record(&fx.pool, 3, record)
            .await
            .expect("call"),
        Err(RetentionRefusal::CapabilityRequired)
    );
    assert_eq!(
        retention::create_hold(&fx.pool, 2, &hold(HoldScope::Record { record_id: 999 }))
            .await
            .expect("call"),
        Err(RetentionRefusal::NoSuchRecord)
    );
    assert!(
        sqlx::query("DELETE FROM evaluation_version")
            .execute(&fx.pool)
            .await
            .is_err(),
        "holds do not open deletion guards"
    );
}

#[tokio::test]
async fn hold_replacement_is_atomic_attributed_and_not_repeatable() {
    let fx = Fixture::new().await;
    fx.authorize().await;
    let original = retention::create_hold(&fx.pool, 2, &hold(HoldScope::Installation))
        .await
        .expect("call")
        .expect("hold");
    let invalid = hold(HoldScope::Enrollment { enrollment_id: 999 });
    assert_eq!(
        retention::replace_hold(&fx.pool, 2, original, &invalid)
            .await
            .expect("call"),
        Err(RetentionRefusal::NoSuchEnrollment)
    );
    assert!(
        retention::list_holds(&fx.pool, 2)
            .await
            .expect("call")
            .expect("holds")[0]
            .release
            .is_none()
    );
    // Inject failure after the replacement trigger would have released the
    // predecessor; the service must roll back the entire operation.
    sqlx::raw_sql("CREATE TRIGGER fail_retention_audit BEFORE INSERT ON audit_event WHEN NEW.kind = 'record_hold_replaced' BEGIN SELECT RAISE(ABORT, 'injected audit failure'); END;").execute(&fx.pool).await.expect("failure injection");
    assert!(
        retention::replace_hold(&fx.pool, 2, original, &hold(HoldScope::Installation))
            .await
            .is_err()
    );
    let history = retention::list_holds(&fx.pool, 2)
        .await
        .expect("call")
        .expect("holds");
    assert_eq!(history.len(), 1);
    assert!(history[0].release.is_none());
    sqlx::query("DROP TRIGGER fail_retention_audit")
        .execute(&fx.pool)
        .await
        .expect("remove injection");
    let input = HoldInput {
        kind: HoldKind::Investigation,
        reason: "Invented successor authority".into(),
        ..hold(HoldScope::Installation)
    };
    let (a, b) = tokio::join!(
        retention::replace_hold(&fx.pool, 2, original, &input),
        retention::release_hold(&fx.pool, 2, original, "Concurrent invented release")
    );
    match (a.expect("replace"), b.expect("release")) {
        (Ok(id), Err(RetentionRefusal::HoldReleased)) => {
            let rows = retention::list_holds(&fx.pool, 2)
                .await
                .expect("call")
                .expect("holds");
            assert_eq!(rows[0].id, id);
            assert_eq!(rows[0].replaces_id, Some(original));
            assert!(rows[0].release.is_none());
            let release = rows[1].release.as_ref().expect("released predecessor");
            assert_eq!(release.replacement_id, Some(id));
            assert_eq!(release.reason, input.reason);
            assert_eq!(release.released_by, 2);
        }
        (Err(RetentionRefusal::HoldReleased), Ok(())) => assert_eq!(
            retention::list_holds(&fx.pool, 2)
                .await
                .expect("call")
                .expect("holds")
                .len(),
            1
        ),
        pair => panic!("one change wins: {pair:?}"),
    }
    assert_eq!(
        retention::release_hold(&fx.pool, 2, original, "Repeated release")
            .await
            .expect("call"),
        Err(RetentionRefusal::HoldReleased)
    );
    fx.probe().await;
}

#[tokio::test]
async fn authorization_is_rechecked_after_waiting_for_a_writer() {
    let fx = Fixture::new().await;
    fx.authorize().await;
    let mut blocker = storage::write_tx(&fx.pool).await.expect("blocker");
    sqlx::query("DELETE FROM capability_grant WHERE user_id=2 AND capability='manage_retention'")
        .execute(&mut *blocker)
        .await
        .expect("pending revocation");
    let input = policy();
    let attempt = retention::create_policy(&fx.pool, 2, &input);
    tokio::pin!(attempt);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut attempt)
            .await
            .is_err()
    );
    blocker.commit().await.expect("commit revocation");
    assert_eq!(
        attempt.await.expect("call"),
        Err(RetentionRefusal::CapabilityRequired)
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM retention_policy")
        .fetch_one(&fx.pool)
        .await
        .expect("count");
    assert_eq!(count, 0);
    fx.probe().await;
}

#[tokio::test]
async fn storage_refuses_mutated_history_and_invalid_shapes() {
    let fx = Fixture::new().await;
    fx.authorize().await;
    let policy_id = retention::create_policy(&fx.pool, 2, &policy())
        .await
        .expect("call")
        .expect("policy");
    let hold_id = retention::create_hold(&fx.pool, 2, &hold(HoldScope::Installation))
        .await
        .expect("call")
        .expect("hold");
    retention::release_hold(&fx.pool, 2, hold_id, "Invented release")
        .await
        .expect("call")
        .expect("release");
    for table in [
        "retention_authority_event",
        "retention_policy",
        "record_hold",
        "hold_release",
    ] {
        for sql in [
            format!("UPDATE {table} SET reason='changed'"),
            format!("DELETE FROM {table}"),
        ] {
            let error = sqlx::query(&sql)
                .execute(&fx.pool)
                .await
                .expect_err("append-only")
                .to_string();
            assert!(error.contains("append-only"), "{error}");
        }
    }
    for (class, trigger, days, action, authority) in [
        ("daily_report", "disposed_at", 1, "destroy", "X"),
        ("disposition_event", "finalized_at", 1, "destroy", "X"),
        ("weekly_summary", "finalized_at", -1, "destroy", "X"),
        ("weekly_summary", "finalized_at", 365_251, "destroy", "X"),
        ("weekly_summary", "finalized_at", 1, "retain", "X"),
        ("weekly_summary", "finalized_at", 1, "destroy", "\u{2003}"),
    ] {
        assert!(sqlx::query("INSERT INTO retention_policy (record_class,version_number,authority,retention_trigger,retention_days,action,reason,created_by,created_at) VALUES (?1,1,?2,?3,?4,?5,'Invented raw test',2,1)").bind(class).bind(authority).bind(trigger).bind(days).bind(action).execute(&fx.pool).await.is_err());
    }
    assert!(sqlx::query("INSERT INTO retention_policy (record_class,version_number,supersedes_id,authority,retention_trigger,retention_days,action,reason,created_by,created_at) VALUES ('weekly_summary',2,?1,'X','finalized_at',1,'destroy','Invented wrong predecessor',2,1)").bind(policy_id).execute(&fx.pool).await.is_err());
    assert!(sqlx::query("INSERT INTO record_hold (kind,authority,reason,created_by,created_at,replaces_id) VALUES ('audit','X','Invented released predecessor',2,1,?1)").bind(hold_id).execute(&fx.pool).await.is_err());
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&fx.pool)
            .await
            .expect("fk check")
            .is_empty()
    );
}

#[tokio::test]
async fn authority_and_policy_failures_leave_no_partial_state() {
    let fx = Fixture::new().await;
    sqlx::raw_sql("CREATE TRIGGER fail_retention_audit BEFORE INSERT ON audit_event WHEN NEW.kind LIKE 'retention_%' BEGIN SELECT RAISE(ABORT, 'injected audit failure'); END;").execute(&fx.pool).await.expect("inject");
    assert!(
        retention::set_authority(&fx.pool, 1, 2, true, "Invented grant")
            .await
            .is_err()
    );
    assert!(
        !capabilities::user_has(&fx.pool, 2, Capability::ManageRetention)
            .await
            .expect("grant rolled back")
    );
    assert!(
        retention::authority_history(&fx.pool, 1)
            .await
            .expect("call")
            .expect("history")
            .is_empty()
    );
    sqlx::query("DROP TRIGGER fail_retention_audit")
        .execute(&fx.pool)
        .await
        .expect("remove");
    fx.authorize().await;
    sqlx::raw_sql("CREATE TRIGGER fail_retention_audit BEFORE INSERT ON audit_event WHEN NEW.kind = 'retention_policy_created' BEGIN SELECT RAISE(ABORT, 'injected audit failure'); END;").execute(&fx.pool).await.expect("inject");
    assert!(
        retention::create_policy(&fx.pool, 2, &policy())
            .await
            .is_err()
    );
    assert!(
        retention::list_policies(&fx.pool, 2)
            .await
            .expect("call")
            .expect("policies")
            .is_empty()
    );
}

#[tokio::test]
async fn http_refusals_are_typed_and_private() {
    use axum::{
        body::Body,
        http::{
            Request, StatusCode,
            header::{CONTENT_TYPE, COOKIE},
        },
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let fx = Fixture::new().await;
    let token = consolebook_server::sessions::create(&fx.pool, 3)
        .await
        .expect("token")
        .0;
    let app = consolebook_server::http::router(consolebook_server::http::AppState {
        pool: fx.pool.clone(),
    });
    for (method, path, body) in [
        ("GET", "/api/retention/policies", serde_json::Value::Null),
        ("GET", "/api/retention/holds", serde_json::Value::Null),
        ("GET", "/api/retention/scopes", serde_json::Value::Null),
        (
            "GET",
            "/api/retention/records/999/holds",
            serde_json::Value::Null,
        ),
        (
            "POST",
            "/api/retention/authority",
            serde_json::json!({"user_id":3,"granted":true,"reason":"Invented self grant"}),
        ),
        (
            "POST",
            "/api/retention/holds/999/release",
            serde_json::json!({"reason":"Invented release"}),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(
                        COOKIE,
                        format!("{}={}", consolebook_server::http::SESSION_COOKIE, token.raw),
                    )
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(json["error"], "capability_required");
    }
}

#[tokio::test]
async fn hold_storage_refuses_ambiguous_scope_self_replacement_and_early_release() {
    let fx = Fixture::new().await;
    fx.authorize().await;
    let (enrollment, record) = fx.record().await;
    assert!(sqlx::query("INSERT INTO record_hold (enrollment_id,evaluation_record_id,kind,authority,reason,created_by,created_at) VALUES (?1,?2,'audit','X','Invented ambiguous scope',2,1)").bind(enrollment).bind(record).execute(&fx.pool).await.is_err());
    assert!(sqlx::query("INSERT INTO record_hold (id,replaces_id,kind,authority,reason,created_by,created_at) VALUES (500,500,'audit','X','Invented self replacement',2,1)").execute(&fx.pool).await.is_err());
    for (kind, reason) in [("unknown", "Invented unknown kind"), ("audit", "\u{2003}")] {
        assert!(sqlx::query("INSERT INTO record_hold (kind,authority,reason,created_by,created_at) VALUES (?1,'X',?2,2,1)").bind(kind).bind(reason).execute(&fx.pool).await.is_err());
    }
    let id = retention::create_hold(&fx.pool, 2, &hold(HoldScope::Record { record_id: record }))
        .await
        .expect("call")
        .expect("hold");
    assert!(sqlx::query("INSERT INTO hold_release (hold_id,released_by,released_at,reason) VALUES (?1,2,0,'Invented early release')").bind(id).execute(&fx.pool).await.is_err());
    let replacement = retention::replace_hold(
        &fx.pool,
        2,
        id,
        &HoldInput {
            kind: HoldKind::Audit,
            reason: "Invented replacement".into(),
            ..hold(HoldScope::Installation)
        },
    )
    .await
    .expect("call")
    .expect("replace");
    let rows = retention::list_holds(&fx.pool, 2)
        .await
        .expect("call")
        .expect("holds");
    assert_eq!(rows[0].id, replacement);
    assert_eq!(rows[0].replaces_id, Some(id));
    assert!(rows[0].release.is_none());
    let release = rows[1].release.as_ref().expect("released predecessor");
    assert_eq!(release.replacement_id, Some(replacement));
    assert_eq!(release.released_at, rows[0].created_at);
    assert_eq!(release.reason, rows[0].reason);
    let applicable = retention::active_holds_for_record(&fx.pool, 2, record)
        .await
        .expect("call")
        .expect("lookup");
    assert_eq!(applicable.len(), 1);
    assert_eq!(applicable[0].id, replacement);
}
