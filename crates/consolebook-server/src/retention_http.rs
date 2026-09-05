//! Thin retention HTTP adapter; services own authority and scope decisions.
use crate::{
    http::{ApiError, AppState, CurrentUser},
    retention::{self, RetentionRefusal},
};
use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
};
use serde::Deserialize;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/retention/scopes", get(scopes))
        .route("/api/retention/policies", get(policies).post(create_policy))
        .route("/api/retention/holds", get(holds).post(create_hold))
        .route("/api/retention/holds/{id}/replace", post(replace_hold))
        .route("/api/retention/holds/{id}/release", post(release_hold))
        .route("/api/retention/records/{id}/holds", get(record_holds))
        .route(
            "/api/retention/authority",
            get(authority).post(set_authority),
        )
}

impl From<RetentionRefusal> for ApiError {
    fn from(value: RetentionRefusal) -> Self {
        let (status, code, message) = match value {
            RetentionRefusal::CapabilityRequired => (
                StatusCode::FORBIDDEN,
                "capability_required",
                "explicit authority is required",
            ),
            RetentionRefusal::Invalid(problems) => {
                return Self::with_problems(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "invalid_retention_input",
                    "check the retention fields",
                    problems,
                );
            }
            RetentionRefusal::StalePolicy => (
                StatusCode::CONFLICT,
                "stale_policy",
                "the policy changed; reload and review the current version",
            ),
            RetentionRefusal::NoSuchEnrollment => (
                StatusCode::NOT_FOUND,
                "no_such_enrollment",
                "no such enrollment",
            ),
            RetentionRefusal::NoSuchRecord => {
                (StatusCode::NOT_FOUND, "no_such_record", "no such record")
            }
            RetentionRefusal::NoSuchHold => (StatusCode::NOT_FOUND, "no_such_hold", "no such hold"),
            RetentionRefusal::HoldReleased => (
                StatusCode::CONFLICT,
                "hold_released",
                "the hold is already released; reload its history",
            ),
            RetentionRefusal::NoSuchUser => (StatusCode::NOT_FOUND, "no_such_user", "no such user"),
            RetentionRefusal::AuthorityUnchanged => (
                StatusCode::CONFLICT,
                "authority_unchanged",
                "the user already has this authority state",
            ),
        };
        Self::new(status, code, message)
    }
}

async fn policies(
    State(s): State<AppState>,
    u: CurrentUser,
) -> Result<Json<Vec<retention::Policy>>, ApiError> {
    Ok(Json(retention::list_policies(&s.pool, u.user.id).await??))
}
async fn create_policy(
    State(s): State<AppState>,
    u: CurrentUser,
    Json(input): Json<retention::PolicyInput>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let id = retention::create_policy(&s.pool, u.user.id, &input).await??;
    Ok((StatusCode::CREATED, Json(serde_json::json!({"id": id}))))
}
async fn holds(
    State(s): State<AppState>,
    u: CurrentUser,
) -> Result<Json<Vec<retention::Hold>>, ApiError> {
    Ok(Json(retention::list_holds(&s.pool, u.user.id).await??))
}
async fn record_holds(
    State(s): State<AppState>,
    u: CurrentUser,
    Path(id): Path<i64>,
) -> Result<Json<Vec<retention::Hold>>, ApiError> {
    Ok(Json(
        retention::active_holds_for_record(&s.pool, u.user.id, id).await??,
    ))
}
async fn create_hold(
    State(s): State<AppState>,
    u: CurrentUser,
    Json(input): Json<retention::HoldInput>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let id = retention::create_hold(&s.pool, u.user.id, &input).await??;
    Ok((StatusCode::CREATED, Json(serde_json::json!({"id": id}))))
}
async fn replace_hold(
    State(s): State<AppState>,
    u: CurrentUser,
    Path(id): Path<i64>,
    Json(input): Json<retention::HoldInput>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let id = retention::replace_hold(&s.pool, u.user.id, id, &input).await??;
    Ok((StatusCode::CREATED, Json(serde_json::json!({"id": id}))))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reason {
    reason: String,
}
async fn release_hold(
    State(s): State<AppState>,
    u: CurrentUser,
    Path(id): Path<i64>,
    Json(input): Json<Reason>,
) -> Result<StatusCode, ApiError> {
    retention::release_hold(&s.pool, u.user.id, id, &input.reason).await??;
    Ok(StatusCode::NO_CONTENT)
}
async fn authority(
    State(s): State<AppState>,
    u: CurrentUser,
) -> Result<Json<Vec<retention::AuthorityEvent>>, ApiError> {
    Ok(Json(
        retention::authority_history(&s.pool, u.user.id).await??,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityInput {
    user_id: i64,
    granted: bool,
    reason: String,
}
async fn set_authority(
    State(s): State<AppState>,
    u: CurrentUser,
    Json(input): Json<AuthorityInput>,
) -> Result<StatusCode, ApiError> {
    retention::set_authority(
        &s.pool,
        u.user.id,
        input.user_id,
        input.granted,
        &input.reason,
    )
    .await??;
    Ok(StatusCode::NO_CONTENT)
}

async fn scopes(
    State(s): State<AppState>,
    u: CurrentUser,
) -> Result<Json<retention::ScopeOptions>, ApiError> {
    Ok(Json(retention::scope_options(&s.pool, u.user.id).await??))
}
