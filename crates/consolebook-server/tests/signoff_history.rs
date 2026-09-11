//! Issue #49: a trainee reads the current state and the complete retained
//! history of task signoffs for their own enrollment, and nothing for
//! anyone else's. The read contract is service-owned (ADR 0021); this
//! suite proves its authorization line, its history semantics across a
//! program-version change, its one-connection discipline, and its HTTP
//! shape. Every fixture is invented.

use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{Request, StatusCode};
use consolebook_server::capabilities::{Capability, RoleBundle};
use consolebook_server::lifecycle::EnrollmentEventKind;
use consolebook_server::programs::{
    self, AnchorDef, CompetencyDef, FormCompetencyDef, FormDef, NarrativeDef, PolicyDef,
    RecordType, ScaleDef, ScaleKind, TaskDef, VersionContent,
};
use consolebook_server::task_signoffs::{self, SignoffHistory, SignoffKind, SignoffRefusal};
use consolebook_server::{
    assignments, data_dir::DataDir, enrollments, lifecycle, setup, storage, trainee_packet, users,
};
use http_body_util::BodyExt;
use tower::ServiceExt;

const PASSWORD: &str = "invented-passphrase-1";

const OPEN_POLICY: PolicyDef = PolicyDef {
    review_approved: false,
    required_narratives: false,
    ratings_complete: false,
};

struct Fixture {
    _tmp: tempfile::TempDir,
    pool: sqlx::SqlitePool,
    admin_id: i64,
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
        }
    }

    fn app(&self) -> axum::Router {
        consolebook_server::http::router(consolebook_server::http::AppState {
            pool: self.pool.clone(),
        })
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
            "",
            "",
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
}

async fn request(
    app: axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(
            COOKIE,
            format!("{}={}", consolebook_server::http::SESSION_COOKIE, cookie),
        );
    }
    let request = if let Some(body) = body {
        builder
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
    } else {
        builder.body(Body::empty())
    }
    .expect("request");
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, body)
}

/// One tasked competency. The label and the task prompt make a version
/// change visible in the history.
fn content(label: &str, prompt: &str) -> VersionContent {
    content_with_tasks(label, &[prompt])
}

/// The same version shape with one task per prompt, so a task list can be
/// given more than one member.
fn content_with_tasks(label: &str, prompts: &[&str]) -> VersionContent {
    VersionContent {
        name: "Example County CTO Program".to_owned(),
        label: label.to_owned(),
        description: "Invented program for signoff-history tests.".to_owned(),
        phases: Vec::new(),
        phase_transitions: Vec::new(),
        competencies: vec![CompetencyDef {
            category: "Call processing".to_owned(),
            name: "Emergency Call Interrogation".to_owned(),
            description: "Obtains and verifies location, callback, and nature.".to_owned(),
            tasks: prompts
                .iter()
                .map(|prompt| TaskDef {
                    prompt: (*prompt).to_owned(),
                    citations: Vec::new(),
                })
                .collect(),
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
        finalization_policy: OPEN_POLICY,
    }
}

#[allow(clippy::struct_field_names)]
struct Seeded {
    program_id: i64,
    version_id: i64,
    enrollment_id: i64,
    taylor_id: i64,
    riley_id: i64,
    jordan_id: i64,
    casey_id: i64,
}

/// One published program version, one enrolled trainee (Taylor), one
/// assigned trainer (Jordan), one coordinator (Casey), and one further
/// trainee (Riley) with no enrollment.
async fn seed(fx: &Fixture, suffix: &str) -> Seeded {
    let program_id = programs::create_program(&fx.pool, fx.admin_id, "Example County CTO Program")
        .await
        .expect("create program")
        .expect("accepted");
    let version_id = programs::create_version(
        &fx.pool,
        fx.admin_id,
        program_id,
        &content("2026 rev A", "Processes an invented structure-fire call."),
    )
    .await
    .expect("create version")
    .expect("accepted");
    programs::publish_version(&fx.pool, fx.admin_id, version_id)
        .await
        .expect("publish")
        .expect("accepted");
    let taylor_id = fx
        .user_with_role(
            &format!("taylor.{suffix}"),
            "Taylor Trainee",
            RoleBundle::Trainee,
        )
        .await;
    let riley_id = fx
        .user_with_role(
            &format!("riley.{suffix}"),
            "Riley Trainee",
            RoleBundle::Trainee,
        )
        .await;
    let jordan_id = fx
        .user_with_role(
            &format!("jordan.{suffix}"),
            "Jordan Trainer",
            RoleBundle::Trainer,
        )
        .await;
    let casey_id = fx
        .user_with_role(
            &format!("casey.{suffix}"),
            "Casey Coordinator",
            RoleBundle::Coordinator,
        )
        .await;
    let enrollment_id = enrollments::enroll(&fx.pool, fx.admin_id, version_id, taylor_id)
        .await
        .expect("call")
        .expect("enrolled");
    assignments::create(&fx.pool, fx.admin_id, enrollment_id, jordan_id)
        .await
        .expect("call")
        .expect("assigned");
    Seeded {
        program_id,
        version_id,
        enrollment_id,
        taylor_id,
        riley_id,
        jordan_id,
        casey_id,
    }
}

/// The one task id of a program version.
async fn task_of(pool: &sqlx::SqlitePool, version_id: i64) -> i64 {
    sqlx::query_scalar("SELECT id FROM task WHERE program_version_id = ?1 ORDER BY id LIMIT 1")
        .bind(version_id)
        .fetch_one(pool)
        .await
        .expect("task")
}

/// Publishes a second version of the same program and repoints the
/// enrollment through the modeled version-change event. Returns the new
/// version's id and number.
async fn change_version(fx: &Fixture, seeded: &Seeded, label: &str, prompt: &str) -> (i64, i64) {
    let next_version_id = programs::create_version(
        &fx.pool,
        fx.admin_id,
        seeded.program_id,
        &content(label, prompt),
    )
    .await
    .expect("create version")
    .expect("accepted");
    programs::publish_version(&fx.pool, fx.admin_id, next_version_id)
        .await
        .expect("publish")
        .expect("accepted");
    lifecycle::record_enrollment_event(
        &fx.pool,
        fx.admin_id,
        seeded.enrollment_id,
        EnrollmentEventKind::VersionChange,
        &format!("Invented change to {label}."),
        Some(next_version_id),
    )
    .await
    .expect("call")
    .expect("accepted");
    let number: i64 =
        sqlx::query_scalar("SELECT version_number FROM program_version WHERE id = ?1")
            .bind(next_version_id)
            .fetch_one(&fx.pool)
            .await
            .expect("version number");
    (next_version_id, number)
}

async fn history(
    fx: &Fixture,
    actor: i64,
    enrollment_id: i64,
) -> std::result::Result<SignoffHistory, SignoffRefusal> {
    task_signoffs::history(&fx.pool, actor, enrollment_id)
        .await
        .expect("call")
}

/// Repoints the enrollment at a version that already exists, the way a
/// version change back to an earlier pin does — not by publishing a copy of
/// it. Returns the version's number.
async fn repin_version(fx: &Fixture, seeded: &Seeded, version_id: i64, label: &str) -> i64 {
    lifecycle::record_enrollment_event(
        &fx.pool,
        fx.admin_id,
        seeded.enrollment_id,
        EnrollmentEventKind::VersionChange,
        &format!("Invented return to {label}."),
        Some(version_id),
    )
    .await
    .expect("call")
    .expect("accepted");
    sqlx::query_scalar("SELECT version_number FROM program_version WHERE id = ?1")
        .bind(version_id)
        .fetch_one(&fx.pool)
        .await
        .expect("version number")
}

/// Publishes a further version under `program_id` and returns its id.
async fn publish_extra_version(fx: &Fixture, program_id: i64, label: &str, prompt: &str) -> i64 {
    let version_id =
        programs::create_version(&fx.pool, fx.admin_id, program_id, &content(label, prompt))
            .await
            .expect("create version")
            .expect("accepted");
    programs::publish_version(&fx.pool, fx.admin_id, version_id)
        .await
        .expect("publish")
        .expect("accepted");
    version_id
}

fn task<'a>(
    history: &'a SignoffHistory,
    prompt: &str,
) -> &'a consolebook_server::task_signoffs::SignoffTaskRow {
    history
        .tasks
        .iter()
        .find(|task| task.prompt == prompt)
        .unwrap_or_else(|| panic!("no task {prompt:?} in {:?}", history.tasks))
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn a_trainee_reads_their_own_current_state_and_nothing_else() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "own").await;
    let structure_fire = task_of(&fx.pool, s.version_id).await;
    task_signoffs::record(
        &fx.pool,
        s.casey_id,
        s.enrollment_id,
        structure_fire,
        SignoffKind::Observed,
        "",
    )
    .await
    .expect("call")
    .expect("accepted");

    let read = history(&fx, s.taylor_id, s.enrollment_id)
        .await
        .expect("the trainee reads their own enrollment");
    assert_eq!(read.enrollment_id, s.enrollment_id);
    assert_eq!(read.trainee_user_id, s.taylor_id);
    assert_eq!(read.current_program_version_id, s.version_id);
    assert_eq!(read.current_program_version_number, 1);
    assert_eq!(read.current_program_version_label, "2026 rev A");
    let signed = task(&read, "Processes an invented structure-fire call.");
    assert_eq!(signed.current_kind, Some(SignoffKind::Observed));
    assert!(signed.in_current_version);
    assert_eq!(signed.signoffs.len(), 1);
    assert_eq!(
        signed.signoffs[0].signed_by_display_name,
        "Casey Coordinator"
    );
    assert_eq!(signed.signoffs[0].kind, SignoffKind::Observed);
    assert_eq!(signed.signoffs[0].reason, "");

    // Nothing for another trainee's enrollment: Riley holds
    // view_own_records and is not the trainee here.
    assert_eq!(
        history(&fx, s.riley_id, s.enrollment_id).await,
        Err(SignoffRefusal::CapabilityRequired)
    );

    // The capability, not the identity alone, is the line: the trainee's
    // own grant is removed, and their own enrollment is refused. This is
    // the case a rule that admitted the trainee on identity would pass.
    let revoked =
        sqlx::query("DELETE FROM capability_grant WHERE user_id = ?1 AND capability = ?2")
            .bind(s.taylor_id)
            .bind(Capability::ViewOwnRecords.as_str())
            .execute(&fx.pool)
            .await
            .expect("revoke");
    assert_eq!(revoked.rows_affected(), 1);
    assert_eq!(
        history(&fx, s.taylor_id, s.enrollment_id).await,
        Err(SignoffRefusal::CapabilityRequired)
    );

    // Nonexistent enrollment is a typed refusal, never an empty history,
    // whether or not the actor could have read it.
    assert_eq!(
        history(&fx, s.taylor_id, 9_999).await,
        Err(SignoffRefusal::NoSuchEnrollment)
    );
    assert_eq!(
        history(&fx, s.casey_id, 9_999).await,
        Err(SignoffRefusal::NoSuchEnrollment)
    );
}

#[tokio::test]
async fn history_carries_every_retained_row_with_its_own_kind_and_reason() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "rows").await;
    let structure_fire = task_of(&fx.pool, s.version_id).await;
    for (kind, reason) in [
        (SignoffKind::Observed, ""),
        (
            SignoffKind::Demonstrated,
            "Re-checked at the invented console.",
        ),
        (
            SignoffKind::Revoked,
            "The invented observation was recorded in error.",
        ),
    ] {
        let outcome = task_signoffs::record(
            &fx.pool,
            s.casey_id,
            s.enrollment_id,
            structure_fire,
            kind,
            reason,
        )
        .await
        .expect("call");
        assert!(outcome.is_ok(), "{outcome:?}");
        // Distinct recorded instants, so the later ordering assertions are
        // not the whole of the proof.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    }
    let read = history(&fx, s.taylor_id, s.enrollment_id)
        .await
        .expect("read");
    let signed = task(&read, "Processes an invented structure-fire call.");
    assert_eq!(signed.signoffs.len(), 3, "every retained row is reported");
    assert_eq!(
        signed
            .signoffs
            .iter()
            .map(|entry| entry.kind)
            .collect::<Vec<_>>(),
        vec![
            SignoffKind::Observed,
            SignoffKind::Demonstrated,
            SignoffKind::Revoked
        ]
    );
    assert_eq!(
        signed.signoffs[1].reason,
        "Re-checked at the invented console."
    );
    assert_eq!(
        signed.signoffs[2].reason,
        "The invented observation was recorded in error."
    );
    // The latest row answers the current state, so a revoked task is
    // distinguishable from one that was never signed off.
    assert_eq!(signed.current_kind, Some(SignoffKind::Revoked));
    // Stable identity in recorded order.
    let ids: Vec<i64> = signed
        .signoffs
        .iter()
        .map(|entry| entry.signoff_id)
        .collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "ascending signoff_id is recorded order");
    // The signer's name is the stored snapshot, not a live join: a later
    // rename never rewrites what the record says.
    sqlx::query("UPDATE user SET display_name = 'Casey Coordinator (renamed)' WHERE id = ?1")
        .bind(s.casey_id)
        .execute(&fx.pool)
        .await
        .expect("rename");
    let after_rename = history(&fx, s.taylor_id, s.enrollment_id)
        .await
        .expect("read");
    let renamed = task(&after_rename, "Processes an invented structure-fire call.");
    assert!(
        renamed
            .signoffs
            .iter()
            .all(|entry| entry.signed_by_display_name == "Casey Coordinator")
    );
}

#[tokio::test]
async fn same_second_signoffs_keep_their_recorded_order() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "second").await;
    let structure_fire = task_of(&fx.pool, s.version_id).await;
    task_signoffs::record(
        &fx.pool,
        s.casey_id,
        s.enrollment_id,
        structure_fire,
        SignoffKind::Observed,
        "",
    )
    .await
    .expect("call")
    .expect("accepted");
    task_signoffs::record(
        &fx.pool,
        s.casey_id,
        s.enrollment_id,
        structure_fire,
        SignoffKind::Demonstrated,
        "Invented re-check in the same second.",
    )
    .await
    .expect("call")
    .expect("accepted");
    let read = history(&fx, s.taylor_id, s.enrollment_id)
        .await
        .expect("read");
    let signed = task(&read, "Processes an invented structure-fire call.");
    assert_eq!(signed.signoffs.len(), 2);
    // Ordering is by row identity, not by the second the act was recorded
    // in: the row written second is reported second even when the two
    // share an instant.
    if signed.signoffs[0].signed_at == signed.signoffs[1].signed_at {
        assert!(signed.signoffs[0].signoff_id < signed.signoffs[1].signoff_id);
    }
    assert_eq!(signed.signoffs[1].kind, SignoffKind::Demonstrated);
    assert_eq!(signed.current_kind, Some(SignoffKind::Demonstrated));
}

#[tokio::test]
async fn a_version_change_keeps_prior_version_signoffs_discoverable() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "change").await;
    let old_task = task_of(&fx.pool, s.version_id).await;
    task_signoffs::record(
        &fx.pool,
        s.casey_id,
        s.enrollment_id,
        old_task,
        SignoffKind::Demonstrated,
        "",
    )
    .await
    .expect("call")
    .expect("accepted");

    let (next_version_id, next_number) =
        change_version(&fx, &s, "2026 rev B", "Processes an invented medical call.").await;
    assert_eq!(next_number, 2);
    let new_task = task_of(&fx.pool, next_version_id).await;
    assert_ne!(new_task, old_task, "a new version has a fresh vocabulary");

    // Vocabularies the enrollment never pinned are not its history: a
    // further version of the same program, and a whole second program, are
    // both published and both absent from the read.
    let unrelated_version_id = publish_extra_version(
        &fx,
        s.program_id,
        "2026 rev C",
        "Processes an invented alarm call.",
    )
    .await;
    let other_program_id =
        programs::create_program(&fx.pool, fx.admin_id, "Second Example County Program")
            .await
            .expect("create program")
            .expect("accepted");
    let other_version_id = publish_extra_version(
        &fx,
        other_program_id,
        "2026 rev A",
        "Processes an invented test call.",
    )
    .await;
    let unrelated_task = task_of(&fx.pool, unrelated_version_id).await;
    let other_program_task = task_of(&fx.pool, other_version_id).await;

    let read = history(&fx, s.taylor_id, s.enrollment_id)
        .await
        .expect("read");
    assert_eq!(read.current_program_version_id, next_version_id);
    assert_eq!(read.current_program_version_number, 2);
    assert_eq!(read.current_program_version_label, "2026 rev B");
    // The read is exactly this enrollment's own pins, not the
    // installation's task vocabulary.
    let mut returned: Vec<i64> = read.tasks.iter().map(|task| task.task_id).collect();
    returned.sort_unstable();
    let mut expected = vec![old_task, new_task];
    expected.sort_unstable();
    assert_eq!(returned, expected, "{:?}", read.tasks);
    assert!(!returned.contains(&unrelated_task));
    assert!(!returned.contains(&other_program_task));

    // The prior version's signoff stays discoverable and is labelled with
    // the version that was pinned when it was signed.
    let prior = task(&read, "Processes an invented structure-fire call.");
    assert_eq!(prior.program_version_id, s.version_id);
    assert_eq!(prior.program_version_number, 1);
    assert_eq!(prior.program_version_label, "2026 rev A");
    assert!(!prior.in_current_version);
    assert_eq!(prior.current_kind, Some(SignoffKind::Demonstrated));
    assert_eq!(prior.signoffs.len(), 1);

    // The current version's task is present with no signoff at all, which
    // is distinguishable from a revoked one.
    let current = task(&read, "Processes an invented medical call.");
    assert!(current.in_current_version);
    assert_eq!(current.current_kind, None);
    assert!(current.signoffs.is_empty());

    // The history is not concealed by an enrollment without a finalized
    // version: this enrollment has none, and both tasks are still there.
    assert_eq!(read.tasks.len(), 2);
}

#[tokio::test]
async fn a_return_to_a_prior_version_keeps_both_epochs_readable() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "return").await;
    let first_task = task_of(&fx.pool, s.version_id).await;
    task_signoffs::record(
        &fx.pool,
        s.casey_id,
        s.enrollment_id,
        first_task,
        SignoffKind::Observed,
        "",
    )
    .await
    .expect("call")
    .expect("accepted");
    let (second_version_id, _) =
        change_version(&fx, &s, "2026 rev B", "Processes an invented medical call.").await;
    let second_task = task_of(&fx.pool, second_version_id).await;
    task_signoffs::record(
        &fx.pool,
        s.casey_id,
        s.enrollment_id,
        second_task,
        SignoffKind::Demonstrated,
        "",
    )
    .await
    .expect("call")
    .expect("accepted");

    // Back to the first version: the enrollment is re-pinned to the row it
    // pinned before, not to a third version that copies it.
    assert_eq!(repin_version(&fx, &s, s.version_id, "2026 rev A").await, 1);
    let read = history(&fx, s.taylor_id, s.enrollment_id)
        .await
        .expect("read");
    assert_eq!(read.current_program_version_id, s.version_id);
    assert_eq!(read.current_program_version_number, 1);
    assert_eq!(read.current_program_version_label, "2026 rev A");
    assert_eq!(read.tasks.len(), 2);

    // The returned-to version's task is current again, and its retained
    // signoff answers for it: a task's current state is the latest row for
    // that task row, so this is the epoch-1 act presented as the current
    // state of the re-pinned version. ADR 0021 states that limit rather
    // than reconstructing epochs.
    let first = task(&read, "Processes an invented structure-fire call.");
    assert!(first.in_current_version);
    assert_eq!(first.program_version_number, 1);
    assert_eq!(first.current_kind, Some(SignoffKind::Observed));
    assert_eq!(first.signoffs.len(), 1);

    // The intervening version's later act is neither dropped nor
    // reattributed: it stays reported under the version it was recorded
    // under, and the returned-to pin does not make it current.
    let second = task(&read, "Processes an invented medical call.");
    assert!(!second.in_current_version);
    assert_eq!(second.program_version_number, 2);
    assert_eq!(second.program_version_label, "2026 rev B");
    assert_eq!(second.current_kind, Some(SignoffKind::Demonstrated));
    assert_eq!(second.signoffs.len(), 1);
    assert!(
        second
            .signoffs
            .iter()
            .all(|entry| entry.signed_by_display_name == "Casey Coordinator")
    );
}

#[tokio::test]
async fn tasks_are_grouped_and_each_tasks_rows_stay_in_recorded_order() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "order").await;
    // A version with two tasks, pinned so both are recordable.
    let two_tasks = programs::create_version(
        &fx.pool,
        fx.admin_id,
        s.program_id,
        &content_with_tasks(
            "2026 rev B",
            &[
                "Processes an invented structure-fire call.",
                "Processes an invented medical call.",
            ],
        ),
    )
    .await
    .expect("create version")
    .expect("accepted");
    programs::publish_version(&fx.pool, fx.admin_id, two_tasks)
        .await
        .expect("publish")
        .expect("accepted");
    assert_eq!(repin_version(&fx, &s, two_tasks, "2026 rev B").await, 2);
    let task_ids: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM task WHERE program_version_id = ?1 ORDER BY id")
            .bind(two_tasks)
            .fetch_all(&fx.pool)
            .await
            .expect("task list");
    assert_eq!(task_ids.len(), 2, "the version has two tasks");

    // Two rows on the first task — the second recorded in the same second,
    // so the recorded order is decided by the row's own identity — and one
    // on the second, interleaved in time between them.
    for (task_id, kind, reason) in [
        (task_ids[0], SignoffKind::Observed, ""),
        (task_ids[1], SignoffKind::Demonstrated, ""),
        (
            task_ids[0],
            SignoffKind::Revoked,
            "The invented observation was recorded in error.",
        ),
    ] {
        let outcome =
            task_signoffs::record(&fx.pool, s.casey_id, s.enrollment_id, task_id, kind, reason)
                .await
                .expect("call");
        assert!(outcome.is_ok(), "{outcome:?}");
    }

    let read = history(&fx, s.taylor_id, s.enrollment_id)
        .await
        .expect("read");
    let mut seen: Vec<i64> = read.tasks.iter().map(|task| task.task_id).collect();
    seen.sort_unstable();
    assert_eq!(seen, task_ids, "each task is reported once");
    // Per task, ascending `signoff_id` is the recorded order the packet
    // reports; grouping is contiguous, so no task is split across the list.
    for reported in &read.tasks {
        let ids: Vec<i64> = reported
            .signoffs
            .iter()
            .map(|entry| entry.signoff_id)
            .collect();
        let mut ascending = ids.clone();
        ascending.sort_unstable();
        assert_eq!(
            ids, ascending,
            "task {} rows are in recorded order",
            reported.task_id
        );
    }
    let first = task(&read, "Processes an invented structure-fire call.");
    assert_eq!(first.signoffs.len(), 2);
    assert_eq!(first.current_kind, Some(SignoffKind::Revoked));
    let second = task(&read, "Processes an invented medical call.");
    assert_eq!(second.signoffs.len(), 1);
    assert_eq!(second.current_kind, Some(SignoffKind::Demonstrated));
}

#[tokio::test]
async fn existing_readers_keep_their_access_and_trainees_gain_no_write() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "access").await;
    let structure_fire = task_of(&fx.pool, s.version_id).await;

    // Assigned trainer, coordinator, and the explicit administrator are
    // unchanged readers.
    for reader in [s.jordan_id, s.casey_id, fx.admin_id] {
        let read = history(&fx, reader, s.enrollment_id)
            .await
            .unwrap_or_else(|refusal| panic!("reader {reader} refused: {refusal:?}"));
        assert_eq!(read.tasks.len(), 1);
    }
    // An unassigned trainer holding the same capability is refused.
    let robin_id = fx
        .user_with_role("robin.unassigned", "Robin Unassigned", RoleBundle::Trainer)
        .await;
    assert_eq!(
        history(&fx, robin_id, s.enrollment_id).await,
        Err(SignoffRefusal::CapabilityRequired)
    );

    // A trainee-only account gains no recording authority by gaining the
    // read: first and override signoffs are both refused.
    let refused_write = task_signoffs::record(
        &fx.pool,
        s.taylor_id,
        s.enrollment_id,
        structure_fire,
        SignoffKind::Observed,
        "",
    )
    .await
    .expect("call");
    assert_eq!(refused_write, Err(SignoffRefusal::CapabilityRequired));
    task_signoffs::record(
        &fx.pool,
        s.casey_id,
        s.enrollment_id,
        structure_fire,
        SignoffKind::Observed,
        "",
    )
    .await
    .expect("call")
    .expect("accepted");
    let refused_override = task_signoffs::record(
        &fx.pool,
        s.taylor_id,
        s.enrollment_id,
        structure_fire,
        SignoffKind::Revoked,
        "An invented trainee attempt to revoke.",
    )
    .await
    .expect("call");
    assert_eq!(refused_override, Err(SignoffRefusal::CapabilityRequired));
    // The refused writes left no trace, and the trainee still reads the
    // state the trainer recorded.
    let read = history(&fx, s.taylor_id, s.enrollment_id)
        .await
        .expect("read");
    let signed = task(&read, "Processes an invented structure-fire call.");
    assert_eq!(signed.signoffs.len(), 1);
    assert_eq!(signed.current_kind, Some(SignoffKind::Observed));
}

#[tokio::test]
async fn history_agrees_with_the_packet_document_it_mirrors() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "packet").await;
    let first_task = task_of(&fx.pool, s.version_id).await;
    task_signoffs::record(
        &fx.pool,
        s.casey_id,
        s.enrollment_id,
        first_task,
        SignoffKind::Observed,
        "",
    )
    .await
    .expect("call")
    .expect("accepted");
    task_signoffs::record(
        &fx.pool,
        s.casey_id,
        s.enrollment_id,
        first_task,
        SignoffKind::Demonstrated,
        "Invented second look.",
    )
    .await
    .expect("call")
    .expect("accepted");
    let (second_version_id, _) =
        change_version(&fx, &s, "2026 rev B", "Processes an invented medical call.").await;
    let second_task = task_of(&fx.pool, second_version_id).await;
    task_signoffs::record(
        &fx.pool,
        s.casey_id,
        s.enrollment_id,
        second_task,
        SignoffKind::Demonstrated,
        "",
    )
    .await
    .expect("call")
    .expect("accepted");

    // The packet carries the same history in its own document format; the
    // two must agree row for row on identity, kind, reason, signer, and
    // instant, so the interface and the packet are not two interpretations
    // of one table (ADR 0015, ADR 0021).
    let packet = trainee_packet::export_at(&fx.pool, s.taylor_id, s.enrollment_id, 1_788_289_200)
        .await
        .expect("call")
        .expect("packed");
    let listed = packet_bytes(&packet.bytes);
    let mut packet_rows = listed
        .iter()
        .map(|row| {
            (
                row["signoff_id"].as_i64().expect("id"),
                row["task_id"].as_i64().expect("task"),
                row["kind"].as_str().expect("kind").to_owned(),
                row["reason"].as_str().expect("reason").to_owned(),
                row["signed_by"]["display_name"]
                    .as_str()
                    .expect("name")
                    .to_owned(),
                row["signed_at"].as_i64().expect("signed_at"),
            )
        })
        .collect::<Vec<_>>();
    packet_rows.sort_by_key(|row| row.0);

    let read = history(&fx, s.taylor_id, s.enrollment_id)
        .await
        .expect("read");
    let mut ours = read
        .tasks
        .iter()
        .flat_map(|task| {
            task.signoffs.iter().map(|entry| {
                (
                    entry.signoff_id,
                    task.task_id,
                    match entry.kind {
                        SignoffKind::Observed => "observed".to_owned(),
                        SignoffKind::Demonstrated => "demonstrated".to_owned(),
                        SignoffKind::Revoked => "revoked".to_owned(),
                    },
                    entry.reason.clone(),
                    entry.signed_by_display_name.clone(),
                    entry.signed_at,
                )
            })
        })
        .collect::<Vec<_>>();
    ours.sort_by_key(|row| row.0);
    assert_eq!(ours, packet_rows, "the read and the packet agree");
    assert_eq!(ours.len(), 3);
}

/// The packet's signoff document, read from the archive by the same
/// verifier the CLI uses.
fn packet_bytes(bytes: &[u8]) -> Vec<serde_json::Value> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).expect("readable archive");
    let mut file = archive
        .by_name("packet/signoffs.json")
        .expect("signoffs document");
    let mut text = String::new();
    std::io::Read::read_to_string(&mut file, &mut text).expect("read document");
    serde_json::from_str(&text).expect("signoff document")
}

#[tokio::test]
async fn a_one_connection_pool_serves_the_read_without_deadlock() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "single").await;
    let structure_fire = task_of(&fx.pool, s.version_id).await;
    task_signoffs::record(
        &fx.pool,
        s.casey_id,
        s.enrollment_id,
        structure_fire,
        SignoffKind::Observed,
        "",
    )
    .await
    .expect("call")
    .expect("accepted");

    // The read evaluates permission and returns content while holding one
    // pooled connection; with a one-connection pool it must not need a
    // second one, or it would wait on itself (the packet's rule).
    let database = fx.pool.connect_options().get_filename().to_owned();
    fx.pool.close().await;
    let single = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(database)
                .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
                .foreign_keys(true)
                .busy_timeout(std::time::Duration::from_secs(1)),
        )
        .await
        .expect("one-connection pool");
    assert_eq!(
        single.size(),
        1,
        "the proof is only sharp while the pool has one connection"
    );
    let read = task_signoffs::history(&single, s.taylor_id, s.enrollment_id)
        .await
        .expect("call")
        .expect("read");
    assert_eq!(read.tasks.len(), 1);
    assert_eq!(read.tasks[0].signoffs.len(), 1);
    single.close().await;
}

#[tokio::test]
async fn the_history_route_serves_the_trainee_and_refuses_everyone_else() {
    let fx = Fixture::new().await;
    let s = seed(&fx, "http").await;
    let structure_fire = task_of(&fx.pool, s.version_id).await;
    task_signoffs::record(
        &fx.pool,
        s.casey_id,
        s.enrollment_id,
        structure_fire,
        SignoffKind::Observed,
        "",
    )
    .await
    .expect("call")
    .expect("accepted");
    let path = format!("/api/enrollments/{}/signoff-history", s.enrollment_id);

    let taylor = fx.login("taylor.http").await;
    let (status, body) = request(fx.app(), "GET", &path, Some(&taylor), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enrollment_id"], s.enrollment_id);
    assert_eq!(body["current_program_version_number"], 1);
    assert_eq!(body["tasks"][0]["current_kind"], "observed");
    assert_eq!(body["tasks"][0]["in_current_version"], true);
    assert_eq!(body["tasks"][0]["signoffs"][0]["kind"], "observed");
    assert_eq!(
        body["tasks"][0]["signoffs"][0]["signed_by_display_name"],
        "Casey Coordinator"
    );

    // Another trainee sees the same 403 a missing capability earns.
    let riley = fx.login("riley.http").await;
    let (status, body) = request(fx.app(), "GET", &path, Some(&riley), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], "capability_required");

    // The assigned trainer keeps today's access, and the nonexistent
    // enrollment is a 404 rather than an empty history.
    let jordan = fx.login("jordan.http").await;
    let (status, _) = request(fx.app(), "GET", &path, Some(&jordan), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = request(
        fx.app(),
        "GET",
        "/api/enrollments/9999/signoff-history",
        Some(&jordan),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "no_such_enrollment");

    // The matrix route keeps exactly the behavior it had: the trainee is
    // still refused there, because that read drives recording controls.
    let (status, body) = request(
        fx.app(),
        "GET",
        &format!("/api/enrollments/{}/signoffs", s.enrollment_id),
        Some(&taylor),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], "capability_required");
}
