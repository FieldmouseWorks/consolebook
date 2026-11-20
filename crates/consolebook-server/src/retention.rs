//! Versioned policies, attributed holds, explicit administration authority,
//! and read-only disposition preview (ADR 0020; ADR 0030; #211).
//! No destruction operation is exposed here.
mod authority;
mod disposition_execute;
mod disposition_preview;
mod holds;
mod policies;
mod types;

pub use authority::{
    authority_history, disposition_authority_history, set_authority, set_disposition_authority,
};
pub use disposition_execute::{
    DispositionExecutionFault, DispositionExecutionInput, DispositionExecutionReport,
    execute_disposition, execute_disposition_at, execute_disposition_with_fault,
    retry_disposition_run, retry_disposition_run_at, retry_disposition_run_with_fault,
};
pub use disposition_preview::{
    ControlledCopyInventory, CopyDirectoryInventory, CopyFileEntry, DispositionActiveHold,
    DispositionAuthoritySnapshot, DispositionBlocker, DispositionDependencyCounts,
    DispositionPolicySummary, DispositionPreview, DispositionPreviewInput,
    DispositionTargetPreview, DispositionVersionSummary, PREVIEW_VALIDITY_SECONDS,
    RecoveryObligation, UncontrolledCopyCategory, preview_disposition, preview_disposition_at,
    verify_disposition_preview, verify_disposition_preview_at,
};
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
