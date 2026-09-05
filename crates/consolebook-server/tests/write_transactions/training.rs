use super::*;
use assignments::AssignRefusal;
use consolebook_server::{
    assignments, enrollments, lifecycle, session_membership, training_sessions,
};
use enrollments::EnrollRefusal;
use lifecycle::{EnrollmentEventKind, LifecycleRefusal, PhaseEventKind};
use training_sessions::{Disposition, SessionInput, SessionRefusal, SessionUpdate};

async fn enrolled(fx: &Fixture) -> i64 {
    let (_, version) = fx.published().await;
    enrollments::enroll(&fx.pool, ACTOR, version, TRAINEE)
        .await
        .expect("call")
        .expect("enroll")
}

fn input() -> SessionInput {
    SessionInput {
        business_date: "2026-06-02".into(),
        timezone: "UTC".into(),
        local_start: "2026-06-02T08:00".into(),
        local_end: None,
        disposition: None,
        phase_id: None,
        trainer_user_ids: vec![TRAINER],
    }
}

async fn session(fx: &Fixture) -> i64 {
    let enrollment = enrolled(fx).await;
    training_sessions::create(&fx.pool, ACTOR, enrollment, &input())
        .await
        .expect("call")
        .expect("session")
}

#[tokio::test]
async fn duplicate_enrollment_has_one_winner() {
    let fx = Fixture::new().await;
    let (_, version) = fx.published().await;
    one_winner(
        contend(
            &fx.pool,
            enrollments::enroll(&fx.pool, ACTOR, version, TRAINEE),
            enrollments::enroll(&fx.pool, ACTOR, version, TRAINEE),
        )
        .await,
        &EnrollRefusal::AlreadyEnrolled,
    );
    fx.probe().await;
}

#[tokio::test]
async fn assignments_serialize_creation_and_ending() {
    let fx = Fixture::new().await;
    let enrollment = enrolled(&fx).await;
    let assignment = one_winner(
        contend(
            &fx.pool,
            assignments::create(&fx.pool, ACTOR, enrollment, TRAINER),
            assignments::create(&fx.pool, ACTOR, enrollment, TRAINER),
        )
        .await,
        &AssignRefusal::AlreadyAssigned,
    );
    fx.probe().await;
    one_winner(
        contend(
            &fx.pool,
            assignments::end(&fx.pool, ACTOR, assignment),
            assignments::end(&fx.pool, ACTOR, assignment),
        )
        .await,
        &AssignRefusal::AlreadyEnded,
    );
    fx.probe().await;
    let notices: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notice WHERE user_id = ?1")
        .bind(TRAINER)
        .fetch_one(&fx.pool)
        .await
        .expect("notices");
    assert_eq!(notices, 1, "losing create leaves no duplicate notice");
}

#[tokio::test]
async fn enrollment_events_validate_the_committed_status_and_pin() {
    let fx = Fixture::new().await;
    let enrollment = enrolled(&fx).await;
    one_winner(
        contend(
            &fx.pool,
            lifecycle::record_enrollment_event(
                &fx.pool,
                ACTOR,
                enrollment,
                EnrollmentEventKind::Withdraw,
                "Invented withdrawal",
                None,
            ),
            lifecycle::record_enrollment_event(
                &fx.pool,
                ACTOR,
                enrollment,
                EnrollmentEventKind::Withdraw,
                "Invented withdrawal",
                None,
            ),
        )
        .await,
        &LifecycleRefusal::NotActive,
    );
    fx.probe().await;
    one_winner(
        contend(
            &fx.pool,
            lifecycle::record_enrollment_event(
                &fx.pool,
                ACTOR,
                enrollment,
                EnrollmentEventKind::Reinstate,
                "Invented return",
                None,
            ),
            lifecycle::record_enrollment_event(
                &fx.pool,
                ACTOR,
                enrollment,
                EnrollmentEventKind::Reinstate,
                "Invented return",
                None,
            ),
        )
        .await,
        &LifecycleRefusal::AlreadyActive,
    );
    let next = programs::create_version(&fx.pool, ACTOR, 1, &content())
        .await
        .expect("call")
        .expect("version");
    programs::publish_version(&fx.pool, ACTOR, next)
        .await
        .expect("call")
        .expect("publish");
    one_winner(
        contend(
            &fx.pool,
            lifecycle::record_enrollment_event(
                &fx.pool,
                ACTOR,
                enrollment,
                EnrollmentEventKind::VersionChange,
                "Invented revision",
                Some(next),
            ),
            lifecycle::record_enrollment_event(
                &fx.pool,
                ACTOR,
                enrollment,
                EnrollmentEventKind::VersionChange,
                "Invented revision",
                Some(next),
            ),
        )
        .await,
        &LifecycleRefusal::SameVersion,
    );
    fx.probe().await;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM enrollment_event WHERE enrollment_id = ?1")
            .bind(enrollment)
            .fetch_one(&fx.pool)
            .await
            .expect("history");
    assert_eq!(count, 3);
}

#[tokio::test]
async fn phase_events_validate_the_committed_pause_state() {
    let fx = Fixture::new().await;
    let enrollment = enrolled(&fx).await;
    let phase: i64 = sqlx::query_scalar("SELECT id FROM phase LIMIT 1")
        .fetch_one(&fx.pool)
        .await
        .expect("phase");
    lifecycle::record_phase_event(
        &fx.pool,
        ACTOR,
        enrollment,
        PhaseEventKind::Advance,
        Some(phase),
        None,
        "",
    )
    .await
    .expect("call")
    .expect("entry");
    one_winner(
        contend(
            &fx.pool,
            lifecycle::record_phase_event(
                &fx.pool,
                ACTOR,
                enrollment,
                PhaseEventKind::Pause,
                None,
                None,
                "Invented pause",
            ),
            lifecycle::record_phase_event(
                &fx.pool,
                ACTOR,
                enrollment,
                PhaseEventKind::Pause,
                None,
                None,
                "Invented pause",
            ),
        )
        .await,
        &LifecycleRefusal::AlreadyPaused,
    );
    fx.probe().await;
}

#[tokio::test]
async fn overlapping_session_creation_has_one_winner() {
    let fx = Fixture::new().await;
    let enrollment = enrolled(&fx).await;
    let input = input();
    one_winner(
        contend(
            &fx.pool,
            training_sessions::create(&fx.pool, ACTOR, enrollment, &input),
            training_sessions::create(&fx.pool, ACTOR, enrollment, &input),
        )
        .await,
        &SessionRefusal::Overlap,
    );
    fx.probe().await;
}

#[tokio::test]
async fn session_updates_and_closes_keep_existing_outcomes() {
    let fx = Fixture::new().await;
    let session = session(&fx).await;
    let first = SessionUpdate {
        business_date: "2026-06-02".into(),
        timezone: "UTC".into(),
        local_start: "2026-06-02T07:00".into(),
        phase_id: None,
    };
    let second = SessionUpdate {
        local_start: "2026-06-02T06:00".into(),
        ..first.clone()
    };
    let (left, right) = contend(
        &fx.pool,
        training_sessions::update_open(&fx.pool, ACTOR, session, &first),
        training_sessions::update_open(&fx.pool, ACTOR, session, &second),
    )
    .await;
    left.expect("left").expect("update");
    right.expect("right").expect("update");
    one_winner(
        contend(
            &fx.pool,
            training_sessions::close(
                &fx.pool,
                ACTOR,
                session,
                Disposition::Completed,
                Some("2026-06-02T16:00"),
            ),
            training_sessions::close(
                &fx.pool,
                ACTOR,
                session,
                Disposition::Completed,
                Some("2026-06-02T16:00"),
            ),
        )
        .await,
        &SessionRefusal::SessionClosed,
    );
    fx.probe().await;
}

#[tokio::test]
async fn membership_additions_and_removals_have_typed_losers() {
    let fx = Fixture::new().await;
    let session = session(&fx).await;
    one_winner(
        contend(
            &fx.pool,
            session_membership::add_trainer(&fx.pool, ACTOR, session, OTHER_TRAINER),
            session_membership::add_trainer(&fx.pool, ACTOR, session, OTHER_TRAINER),
        )
        .await,
        &SessionRefusal::AlreadyMember,
    );
    fx.probe().await;
    one_winner(
        contend(
            &fx.pool,
            session_membership::remove_trainer(&fx.pool, ACTOR, session, TRAINER),
            session_membership::remove_trainer(&fx.pool, ACTOR, session, OTHER_TRAINER),
        )
        .await,
        &SessionRefusal::LastTrainer,
    );
    fx.probe().await;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM session_trainer WHERE session_id = ?1")
            .bind(session)
            .fetch_one(&fx.pool)
            .await
            .expect("trainers");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn competing_session_requests_return_created_and_typed_conflict() {
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
    let enrollment = enrolled(&fx).await;
    let token = consolebook_server::sessions::create(&fx.pool, ACTOR)
        .await
        .expect("login token")
        .0;
    let app = consolebook_server::http::router(consolebook_server::http::AppState {
        pool: fx.pool.clone(),
    });
    let request = || {
        Request::builder().method("POST").uri(format!("/api/enrollments/{enrollment}/sessions"))
        .header(CONTENT_TYPE, "application/json")
        .header(COOKIE, format!("{}={}", consolebook_server::http::SESSION_COOKIE, token.raw))
        .body(Body::from(serde_json::json!({"business_date":"2026-06-02", "timezone":"UTC", "local_start":"2026-06-02T08:00", "trainer_user_ids":[TRAINER]}).to_string())).expect("request")
    };
    let (left, right) = contend(
        &fx.pool,
        app.clone().oneshot(request()),
        app.oneshot(request()),
    )
    .await;
    let (left, right) = (left.expect("response"), right.expect("response"));
    let refused = match (left.status(), right.status()) {
        (StatusCode::CREATED, StatusCode::CONFLICT) => right,
        (StatusCode::CONFLICT, StatusCode::CREATED) => left,
        statuses => panic!("expected 201 and 409, got {statuses:?}"),
    };
    let bytes = refused
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("typed JSON");
    assert_eq!(body["error"], "interval_overlap");
    fx.probe().await;
}

#[tokio::test]
async fn cancellation_and_draft_creation_cannot_both_commit() {
    use consolebook_server::evaluation_drafts::{self, DraftRefusal};
    let fx = Fixture::new().await;
    let session = session(&fx).await;
    let (cancelled, documented) = contend(
        &fx.pool,
        training_sessions::close(&fx.pool, ACTOR, session, Disposition::Cancelled, None),
        evaluation_drafts::create(&fx.pool, TRAINER, session, None),
    )
    .await;
    match (
        cancelled.expect("cancel outcome"),
        documented.expect("draft outcome"),
    ) {
        (Ok(()), Err(DraftRefusal::SessionCancelled))
        | (Err(SessionRefusal::SessionDocumented), Ok(_)) => {}
        outcomes => panic!("cancellation and coverage are mutually exclusive: {outcomes:?}"),
    }
    let contradictions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM training_session ts JOIN evaluation_session es ON es.training_session_id = ts.id WHERE ts.disposition = 'cancelled'"
    ).fetch_one(&fx.pool).await.expect("coverage");
    assert_eq!(contradictions, 0);
    fx.probe().await;
}
