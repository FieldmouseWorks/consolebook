//! Versioned policies, attributed holds, and explicit administration authority.
//! ADR 0020 defines this preparation stage; destruction is a separate workflow.
mod authority;
mod holds;
mod policies;
mod types;

pub use authority::{authority_history, set_authority};
pub use holds::{
    ScopeOption, ScopeOptions, active_holds_for_record, create_hold, list_holds, release_hold,
    replace_hold, scope_options,
};
pub use policies::{create_policy, list_policies};
pub use types::*;

use crate::{
    capabilities::{self, Capability},
    storage,
};
use anyhow::Result;
use sqlx::{Sqlite, SqliteConnection, SqlitePool, Transaction};

async fn authorized_write(
    pool: &SqlitePool,
    actor: i64,
    capability: Capability,
) -> Result<Result<Transaction<'static, Sqlite>, RetentionRefusal>> {
    let mut tx = storage::write_tx(pool).await?;
    if !capabilities::user_has_on(&mut tx, actor, capability).await? {
        return storage::refuse(tx, RetentionRefusal::CapabilityRequired).await;
    }
    Ok(Ok(tx))
}

async fn authorized_read(
    pool: &SqlitePool,
    actor: i64,
    capability: Capability,
) -> Result<Result<Transaction<'static, Sqlite>, RetentionRefusal>> {
    let mut tx = pool.begin().await?;
    if !capabilities::user_has_on(&mut tx, actor, capability).await? {
        return storage::refuse(tx, RetentionRefusal::CapabilityRequired).await;
    }
    Ok(Ok(tx))
}

fn text_problem(value: &str, field: &str, maximum: usize, problems: &mut Vec<String>) {
    let length = value.trim().chars().count();
    if length == 0 || length > maximum {
        problems.push(format!("{field} must contain 1–{maximum} characters"));
    }
}

async fn scope_exists(
    conn: &mut SqliteConnection,
    scope: &HoldScope,
) -> Result<Option<RetentionRefusal>> {
    let (query, id, refusal) = match scope {
        HoldScope::Installation => return Ok(None),
        HoldScope::Enrollment { enrollment_id } => (
            "SELECT 1 FROM enrollment WHERE id = ?1",
            *enrollment_id,
            RetentionRefusal::NoSuchEnrollment,
        ),
        HoldScope::Record { record_id } => (
            "SELECT 1 FROM evaluation_record WHERE id = ?1",
            *record_id,
            RetentionRefusal::NoSuchRecord,
        ),
    };
    let exists: Option<i64> = sqlx::query_scalar(query)
        .bind(id)
        .fetch_optional(conn)
        .await?;
    Ok(exists.is_none().then_some(refusal))
}
