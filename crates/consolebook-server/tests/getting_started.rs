//! The administrator's setup progress: counts that follow the installation
//! from first run to its first session, and only for `manage_users`.
//! All fixtures are invented.

use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE};
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use consolebook_server::capabilities::{self, COORDINATOR_BUNDLE, TRAINEE_BUNDLE, TRAINER_BUNDLE};
use consolebook_server::data_dir::DataDir;
use consolebook_server::getting_started::{self, Progress, ProgressRefusal};
use consolebook_server::{http, secrets, setup, storage, users};

const PASSWORD: &str = "invented-passphrase-1";

const NOTHING_YET: Progress = Progress {
    coordinators: 0,
    trainers: 0,
    trainees: 0,
    programs: 0,
    published_versions: 0,
    enrollments: 0,
    assignments: 0,
    sessions: 0,
    finalized_records: 0,
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

    async fn person(&self, username: &str, bundle: &[capabilities::Capability]) -> i64 {
        let hash = secrets::hash_password(PASSWORD).expect("hash");
        let mut conn = self.pool.acquire().await.expect("conn");
        let id = users::create(&mut conn, username, username, "", "", &hash)
            .await
            .expect("create");
        capabilities::grant_bundle(&mut conn, id, bundle, Some(self.admin_id))
            .await
            .expect("grant");
        id
    }

    async fn get_as(&self, username: &str) -> (StatusCode, serde_json::Value) {
        let app = http::router(http::AppState::new(self.pool.clone()));
        let login = app
            .clone()
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
            .expect("login");
        assert_eq!(login.status(), StatusCode::OK, "sign in as {username}");
        let cookie = login
            .headers()
            .get("set-cookie")
            .expect("session cookie")
            .to_str()
            .expect("ascii")
            .split(';')
            .next()
            .expect("pair")
            .to_owned();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/getting-started")
                    .header(COOKIE, cookie)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("get");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        (status, serde_json::from_slice(&bytes).expect("json"))
    }
}

#[tokio::test]
async fn a_fresh_installation_has_nothing_yet_and_administrators_are_not_counted() {
    let fx = Fixture::new().await;
    let progress = getting_started::progress(&fx.pool, fx.admin_id)
        .await
        .expect("read")
        .expect("allowed");
    // The administrator holds none of the counted bundles' capabilities.
    assert_eq!(progress, NOTHING_YET);
}

#[tokio::test]
async fn people_are_counted_by_the_bundle_they_were_granted() {
    let fx = Fixture::new().await;
    fx.person("casey.coordinator", &COORDINATOR_BUNDLE).await;
    fx.person("jordan.trainer", &TRAINER_BUNDLE).await;
    fx.person("rowan.trainer", &TRAINER_BUNDLE).await;
    fx.person("taylor.trainee", &TRAINEE_BUNDLE).await;
    let progress = getting_started::progress(&fx.pool, fx.admin_id)
        .await
        .expect("read")
        .expect("allowed");
    assert_eq!(
        progress,
        Progress {
            coordinators: 1,
            trainers: 2,
            trainees: 1,
            ..NOTHING_YET
        }
    );
}

#[tokio::test]
async fn only_user_administrators_read_progress() {
    let fx = Fixture::new().await;
    let coordinator = fx.person("casey.coordinator", &COORDINATOR_BUNDLE).await;
    let trainer = fx.person("jordan.trainer", &TRAINER_BUNDLE).await;
    for actor in [coordinator, trainer] {
        assert_eq!(
            getting_started::progress(&fx.pool, actor)
                .await
                .expect("read"),
            Err(ProgressRefusal::CapabilityRequired)
        );
    }

    let (status, body) = fx.get_as("avery.admin").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["trainers"], 1);
    assert_eq!(body["coordinators"], 1);

    let (status, body) = fx.get_as("jordan.trainer").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "capability_required");
}
