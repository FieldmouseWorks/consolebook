use super::*;
use consolebook_server::{sessions, setup};
use users::{CreateUserRefusal, ResetOrigin, ResetOutcome};

#[tokio::test]
async fn duplicate_usernames_are_rechecked_after_hashing() {
    let fx = Fixture::new().await;
    let created = one_winner(
        contend(
            &fx.pool,
            users::create_with_reset_code(
                &fx.pool,
                ACTOR,
                "casey.example",
                "Casey Example",
                "",
                "",
                capabilities::RoleBundle::Trainee,
            ),
            users::create_with_reset_code(
                &fx.pool,
                ACTOR,
                "CASEY.EXAMPLE",
                "Casey Example",
                "",
                "",
                capabilities::RoleBundle::Trainee,
            ),
        )
        .await,
        &CreateUserRefusal::UsernameTaken,
    );
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM password_reset_code WHERE user_id = ?1")
            .bind(created.id)
            .fetch_one(&fx.pool)
            .await
            .expect("initial code");
    assert_eq!(count, 1);
    fx.probe().await;
}

#[tokio::test]
async fn reset_issuance_serializes_without_discarding_valid_codes() {
    let fx = Fixture::new().await;
    let (left, right) = contend(
        &fx.pool,
        users::issue_reset_code(
            &fx.pool,
            "taylor.trainee",
            ResetOrigin::Administrator { issued_by: ACTOR },
        ),
        users::issue_reset_code(
            &fx.pool,
            "taylor.trainee",
            ResetOrigin::Administrator { issued_by: ACTOR },
        ),
    )
    .await;
    let left = left.expect("left").expect("issued");
    let right = right.expect("right").expect("issued");
    assert_ne!(left.code.digest_hex, right.code.digest_hex);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM password_reset_code WHERE user_id = ?1 AND used_at IS NULL",
    )
    .bind(TRAINEE)
    .fetch_one(&fx.pool)
    .await
    .expect("codes");
    assert_eq!(
        count, 2,
        "issuance permits multiple independent codes as before"
    );
}

#[tokio::test]
async fn reset_code_is_consumed_once_and_revokes_sessions() {
    let fx = Fixture::new().await;
    let issued = users::issue_reset_code(
        &fx.pool,
        "taylor.trainee",
        ResetOrigin::Administrator { issued_by: ACTOR },
    )
    .await
    .expect("call")
    .expect("issued");
    let token = sessions::create(&fx.pool, TRAINEE)
        .await
        .expect("session")
        .0;
    let (left, right) = contend(
        &fx.pool,
        users::use_reset_code(&fx.pool, "taylor.trainee", &issued.code.raw, PASSWORD),
        users::use_reset_code(&fx.pool, "taylor.trainee", &issued.code.raw, PASSWORD),
    )
    .await;
    let outcomes = (left.expect("left"), right.expect("right"));
    assert!(
        matches!(
            outcomes,
            (ResetOutcome::Done, ResetOutcome::Invalid)
                | (ResetOutcome::Invalid, ResetOutcome::Done)
        ),
        "{outcomes:?}"
    );
    assert!(
        sessions::validate(&fx.pool, &token.raw)
            .await
            .expect("validate")
            .is_none()
    );
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_event WHERE kind = 'reset_code_used'")
            .fetch_one(&fx.pool)
            .await
            .expect("audit");
    assert_eq!(count, 1);
    fx.probe().await;
}

#[tokio::test]
async fn setup_has_one_administrator_and_consumes_its_code_once() {
    let fx = Fixture::empty().await;
    let code = setup::issue_setup_code(&fx.pool)
        .await
        .expect("issue")
        .expect("uninitialized")
        .0;
    one_winner(
        contend(
            &fx.pool,
            setup::initialize(
                &fx.pool,
                &code.raw,
                "Invented County",
                "avery.admin",
                "Avery Admin",
                PASSWORD,
            ),
            setup::initialize(
                &fx.pool,
                &code.raw,
                "Invented County",
                "rowan.admin",
                "Rowan Admin",
                PASSWORD,
            ),
        )
        .await,
        &setup::SetupRefusal::AlreadyInitialized,
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user")
        .fetch_one(&fx.pool)
        .await
        .expect("users");
    assert_eq!(count, 1);
    assert!(
        setup::issue_setup_code(&fx.pool)
            .await
            .expect("issue after setup")
            .is_none()
    );
    fx.probe().await;
}

#[tokio::test]
async fn setup_code_rotation_serializes_with_initialization() {
    let fx = Fixture::empty().await;
    let code = setup::issue_setup_code(&fx.pool)
        .await
        .expect("issue")
        .expect("uninitialized")
        .0;
    let (initialized, rotated) = contend(
        &fx.pool,
        setup::initialize(
            &fx.pool,
            &code.raw,
            "Invented County",
            "avery.admin",
            "Avery Admin",
            PASSWORD,
        ),
        setup::issue_setup_code(&fx.pool),
    )
    .await;
    match (initialized.expect("initialize"), rotated.expect("rotate")) {
        (Ok(_), None) => assert!(setup::is_initialized(&fx.pool).await.expect("initialized")),
        (Err(setup::SetupRefusal::InvalidCode), Some((replacement, _))) => {
            setup::initialize(
                &fx.pool,
                &replacement.raw,
                "Invented County",
                "avery.admin",
                "Avery Admin",
                PASSWORD,
            )
            .await
            .expect("call")
            .expect("replacement accepted");
        }
        _ => panic!("rotation either precedes setup or is refused after setup"),
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM setup_code")
        .fetch_one(&fx.pool)
        .await
        .expect("codes");
    assert_eq!(count, 0, "no setup code survives initialization");
    fx.probe().await;
}
