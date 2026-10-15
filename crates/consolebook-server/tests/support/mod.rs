//! Shared support for the integration tests: the one owner of test
//! credentials (#126).
//!
//! Every password a test uses comes from here and is generated at run
//! time, once per test process, from OS randomness, except the
//! allowlisted compatibility fixture in `upgrade_compat.rs`. Test files
//! declare no password literal of their own, and
//! `tests/no_credential_literals.rs` fails `cargo test` on the shapes it
//! names in its scan contract.
//!
//! The sign-in helper never formats a username or password into a
//! panic or assertion message; it names the request path instead.
//!
//! Include it from a test crate root with `mod support;`. Each crate
//! uses only some of these items.
#![allow(dead_code)]

use std::sync::OnceLock;

use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, SET_COOKIE};
use axum::http::{Request, StatusCode};
use consolebook_server::capabilities::RoleBundle;
use consolebook_server::secrets::{MAX_PASSWORD_BYTES, MIN_PASSWORD_BYTES};
use consolebook_server::users;
use tower::ServiceExt;

const LOGIN_PATH: &str = "/api/auth/login";

/// A new password of `invented-` and 32 random hex digits: 41 bytes,
/// inside the product's length rules, and never equal to a username.
fn generate() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("OS randomness for a test password");
    let generated = format!("invented-{:032x}", u128::from_le_bytes(bytes));
    assert!((MIN_PASSWORD_BYTES..=MAX_PASSWORD_BYTES).contains(&generated.len()));
    generated
}

/// The password every invented account in this test process signs in
/// with.
pub fn password() -> &'static str {
    static PASSWORD: OnceLock<String> = OnceLock::new();
    PASSWORD.get_or_init(generate)
}

/// A well-formed password that no account in this test process has.
pub fn wrong_password() -> &'static str {
    static WRONG: OnceLock<String> = OnceLock::new();
    let wrong = WRONG.get_or_init(generate);
    // `assert!`, not `assert_ne!`: a failure never prints a password.
    assert!(wrong != password(), "the wrong password differs");
    wrong
}

/// A new well-formed password on every call, for flows that set or
/// rotate one (reset, recovery, first sign-in).
pub fn fresh_password() -> String {
    let fresh = generate();
    assert!(fresh != password(), "a fresh password differs");
    fresh
}

/// One byte shorter than the product's minimum, so the length rule
/// alone refuses it.
pub fn too_short_password() -> String {
    password()[..MIN_PASSWORD_BYTES - 1].to_owned()
}

/// Signs in over `app` and returns the session cookie's value.
pub async fn login(app: axum::Router, username: &str, password: &str) -> String {
    let body = serde_json::json!({ "username": username, "password": password });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(LOGIN_PATH)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK, "POST {LOGIN_PATH}");
    let cookie = response
        .headers()
        .get(SET_COOKIE)
        .expect("session cookie")
        .to_str()
        .expect("ascii");
    let (pair, _) = cookie.split_once(';').expect("cookie attributes");
    pair.split_once('=').expect("cookie pair").1.to_string()
}

/// Creates an account through the administrator's reset-code flow and
/// sets [`password`] through the standard reset; returns its id.
pub async fn user_with_role(
    pool: &sqlx::SqlitePool,
    admin_id: i64,
    username: &str,
    display_name: &str,
    employee_id: &str,
    title: &str,
    role: RoleBundle,
) -> i64 {
    let created = users::create_with_reset_code(
        pool,
        admin_id,
        username,
        display_name,
        employee_id,
        title,
        role,
    )
    .await
    .expect("create")
    .expect("accepted");
    assert_eq!(
        users::use_reset_code(pool, username, &created.reset_code.raw, password())
            .await
            .expect("reset"),
        users::ResetOutcome::Done
    );
    created.id
}
