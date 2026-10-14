//! The public URL and the `Secure` session cookie (ADR 0025). All fixtures
//! are invented.

use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use consolebook_server::public_url::PublicUrl;
use consolebook_server::{http, setup, storage};

const PASSWORD: &str = "invented-passphrase-1";

async fn initialized_pool() -> (tempfile::TempDir, sqlx::SqlitePool) {
    let tmp = tempfile::tempdir().expect("temp dir");
    let pool = storage::open(&tmp.path().join("consolebook.db"))
        .await
        .expect("open");
    let code = setup::issue_setup_code(&pool)
        .await
        .expect("issue")
        .expect("uninitialized")
        .0;
    setup::initialize(
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
    (tmp, pool)
}

/// Signs in and signs out; returns both `Set-Cookie` headers.
async fn cookie_headers(state: http::AppState) -> (String, String) {
    let app = http::router(state);
    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({ "username": "avery.admin", "password": PASSWORD })
                        .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("login");
    assert_eq!(login.status(), StatusCode::OK);
    let set = login.headers()[SET_COOKIE]
        .to_str()
        .expect("ascii")
        .to_owned();
    let pair = set.split(';').next().expect("pair").to_owned();
    let logout = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/logout")
                .header(CONTENT_TYPE, "application/json")
                .header(COOKIE, pair)
                .body(Body::from("{}"))
                .expect("request"),
        )
        .await
        .expect("logout");
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    let cleared = logout.headers()[SET_COOKIE]
        .to_str()
        .expect("ascii")
        .to_owned();
    (set, cleared)
}

fn has_attribute(cookie: &str, name: &str) -> bool {
    cookie
        .split(';')
        .any(|part| part.trim().eq_ignore_ascii_case(name))
}

#[tokio::test]
async fn an_https_public_url_marks_session_cookies_secure() {
    let (_tmp, pool) = initialized_pool().await;
    let url: PublicUrl = "https://consolebook.example.org".parse().expect("url");
    let (set, cleared) = cookie_headers(http::AppState::new(pool).with_public_url(url)).await;
    for cookie in [&set, &cleared] {
        assert!(has_attribute(cookie, "Secure"), "{cookie}");
        assert!(has_attribute(cookie, "HttpOnly"), "{cookie}");
        assert!(has_attribute(cookie, "SameSite=Strict"), "{cookie}");
    }
}

#[tokio::test]
async fn without_an_https_public_url_cookies_work_over_plain_http() {
    let (_tmp, pool) = initialized_pool().await;
    let (set, cleared) = cookie_headers(http::AppState::new(pool.clone())).await;
    assert!(!has_attribute(&set, "Secure"), "{set}");
    assert!(!has_attribute(&cleared, "Secure"), "{cleared}");

    let trial: PublicUrl = "http://192.0.2.10:7770".parse().expect("url");
    let (set, _) = cookie_headers(http::AppState::new(pool).with_public_url(trial)).await;
    assert!(!has_attribute(&set, "Secure"), "{set}");
}

#[tokio::test]
async fn the_instance_reports_the_public_url_when_set() {
    let (_tmp, pool) = initialized_pool().await;
    for (state, expected) in [
        (http::AppState::new(pool.clone()), serde_json::Value::Null),
        (
            http::AppState::new(pool)
                .with_public_url("https://consolebook.example.org/".parse().expect("url")),
            serde_json::json!("https://consolebook.example.org"),
        ),
    ] {
        let response = http::router(state)
            .oneshot(
                Request::builder()
                    .uri("/api/instance")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("instance");
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(body["public_url"], expected);
    }
}

/// The installer validates `--public-url` with this command before it stops
/// a working service, so it must agree with `serve`'s parser exactly.
#[test]
fn the_installer_check_uses_the_server_parser() {
    let check = |raw: &str| {
        std::process::Command::new(env!("CARGO_BIN_EXE_consolebook-server"))
            .args(["check-public-url", raw])
            .output()
            .expect("run")
    };
    let ok = check("https://consolebook.example.org/");
    assert!(ok.status.success(), "{ok:?}");
    assert_eq!(
        String::from_utf8_lossy(&ok.stdout),
        "https://consolebook.example.org\n"
    );
    for bad in [
        "https://:",
        "https://consolebook.example.org:notaport",
        "consolebook.example.org",
    ] {
        assert!(!check(bad).status.success(), "{bad} was accepted");
    }
}
