//! Attributed hold lifecycle and exact scope matching, independent of policy age.
use super::{
    Hold, HoldInput, HoldKind, HoldRelease, HoldScope, RetentionRefusal, authorized_read,
    authorized_write, scope_exists, text_problem,
};
use crate::{
    audit::{self, EventKind, Subject},
    capabilities::Capability,
    storage,
};
use anyhow::{Context, Result};
use sqlx::{Row, SqliteConnection, SqlitePool};
use time::OffsetDateTime;

pub async fn create_hold(
    pool: &SqlitePool,
    actor: i64,
    input: &HoldInput,
) -> Result<Result<i64, RetentionRefusal>> {
    write_hold(pool, actor, input, None).await
}

pub async fn replace_hold(
    pool: &SqlitePool,
    actor: i64,
    hold_id: i64,
    input: &HoldInput,
) -> Result<Result<i64, RetentionRefusal>> {
    write_hold(pool, actor, input, Some(hold_id)).await
}

async fn active_refusal(conn: &mut SqliteConnection, id: i64) -> Result<Option<RetentionRefusal>> {
    let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM record_hold WHERE id = ?1")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    if exists.is_none() {
        return Ok(Some(RetentionRefusal::NoSuchHold));
    }
    let released: Option<i64> = sqlx::query_scalar("SELECT 1 FROM hold_release WHERE hold_id = ?1")
        .bind(id)
        .fetch_optional(conn)
        .await?;
    Ok(released.is_some().then_some(RetentionRefusal::HoldReleased))
}

async fn write_hold(
    pool: &SqlitePool,
    actor: i64,
    input: &HoldInput,
    replaces_id: Option<i64>,
) -> Result<Result<i64, RetentionRefusal>> {
    let mut tx = match authorized_write(pool, actor, Capability::ManageRetention).await? {
        Ok(tx) => tx,
        Err(r) => return Ok(Err(r)),
    };
    let mut problems = Vec::new();
    text_problem(&input.authority, "authority", 200, &mut problems);
    text_problem(&input.reason, "reason", 1000, &mut problems);
    if !problems.is_empty() {
        return storage::refuse(tx, RetentionRefusal::Invalid(problems)).await;
    }
    if let Some(r) = scope_exists(&mut tx, &input.scope).await? {
        return storage::refuse(tx, r).await;
    }
    if let Some(id) = replaces_id
        && let Some(r) = active_refusal(&mut tx, id).await?
    {
        return storage::refuse(tx, r).await;
    }
    let (enrollment_id, record_id) = match input.scope {
        HoldScope::Installation => (None, None),
        HoldScope::Enrollment { enrollment_id } => (Some(enrollment_id), None),
        HoldScope::Record { record_id } => (None, Some(record_id)),
    };
    // The insert trigger atomically releases the predecessor with these same
    // attributed fields. A failed successor insert cannot weaken the old hold.
    let id = sqlx::query("INSERT INTO record_hold (enrollment_id, evaluation_record_id, kind, authority, reason, created_by, created_at, replaces_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)")
        .bind(enrollment_id).bind(record_id).bind(input.kind.as_str()).bind(input.authority.trim()).bind(input.reason.trim())
        .bind(actor).bind(OffsetDateTime::now_utc().unix_timestamp()).bind(replaces_id)
        .execute(&mut *tx).await.context("recording hold")?.last_insert_rowid();
    let kind = if replaces_id.is_some() {
        EventKind::RecordHoldReplaced
    } else {
        EventKind::RecordHoldCreated
    };
    audit::record_for_subject(&mut *tx, kind, Some(actor), None, Subject::RecordHold(id)).await?;
    tx.commit().await?;
    Ok(Ok(id))
}

pub async fn release_hold(
    pool: &SqlitePool,
    actor: i64,
    hold_id: i64,
    reason: &str,
) -> Result<Result<(), RetentionRefusal>> {
    let mut tx = match authorized_write(pool, actor, Capability::ManageRetention).await? {
        Ok(tx) => tx,
        Err(r) => return Ok(Err(r)),
    };
    let mut problems = Vec::new();
    text_problem(reason, "reason", 1000, &mut problems);
    if !problems.is_empty() {
        return storage::refuse(tx, RetentionRefusal::Invalid(problems)).await;
    }
    if let Some(r) = active_refusal(&mut tx, hold_id).await? {
        return storage::refuse(tx, r).await;
    }
    sqlx::query("INSERT INTO hold_release (hold_id, released_by, released_at, reason) VALUES (?1, ?2, ?3, ?4)")
        .bind(hold_id).bind(actor).bind(OffsetDateTime::now_utc().unix_timestamp()).bind(reason.trim())
        .execute(&mut *tx).await.context("releasing hold")?;
    audit::record_for_subject(
        &mut *tx,
        EventKind::RecordHoldReleased,
        Some(actor),
        None,
        Subject::RecordHold(hold_id),
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

fn hold_from_row(r: &sqlx::sqlite::SqliteRow) -> Result<Hold> {
    let scope = match (
        r.get::<Option<i64>, _>("enrollment_id"),
        r.get::<Option<i64>, _>("evaluation_record_id"),
    ) {
        (None, None) => HoldScope::Installation,
        (Some(enrollment_id), None) => HoldScope::Enrollment { enrollment_id },
        (None, Some(record_id)) => HoldScope::Record { record_id },
        _ => anyhow::bail!("invalid stored hold scope"),
    };
    let release = r
        .get::<Option<i64>, _>("released_by")
        .map(|released_by| HoldRelease {
            released_by,
            released_at: r.get("released_at"),
            reason: r.get("release_reason"),
            replacement_id: r.get("replacement_id"),
        });
    Ok(Hold {
        id: r.get("id"),
        scope,
        kind: HoldKind::from_db(r.get("kind"))?,
        authority: r.get("authority"),
        reason: r.get("reason"),
        created_by: r.get("created_by"),
        created_at: r.get("created_at"),
        replaces_id: r.get("replaces_id"),
        release,
    })
}

const HOLD_QUERY: &str = "SELECT h.*, r.released_by, r.released_at, r.reason AS release_reason, r.replacement_id FROM record_hold h LEFT JOIN hold_release r ON r.hold_id = h.id";

pub async fn list_holds(
    pool: &SqlitePool,
    actor: i64,
) -> Result<Result<Vec<Hold>, RetentionRefusal>> {
    let mut tx = match authorized_read(pool, actor, Capability::ManageRetention).await? {
        Ok(tx) => tx,
        Err(r) => return Ok(Err(r)),
    };
    let rows = sqlx::query(&format!("{HOLD_QUERY} ORDER BY h.id DESC"))
        .fetch_all(&mut *tx)
        .await?;
    let holds = rows.iter().map(hold_from_row).collect::<Result<Vec<_>>>()?;
    tx.commit().await?;
    Ok(Ok(holds))
}

/// Resolve applicable active holds under the same snapshot as authorization.
/// This is a hold lookup, never a disposition-eligibility verdict.
pub async fn active_holds_for_record(
    pool: &SqlitePool,
    actor: i64,
    record_id: i64,
) -> Result<Result<Vec<Hold>, RetentionRefusal>> {
    let mut tx = match authorized_read(pool, actor, Capability::ManageRetention).await? {
        Ok(tx) => tx,
        Err(r) => return Ok(Err(r)),
    };
    let enrollment_id: Option<i64> =
        sqlx::query_scalar("SELECT enrollment_id FROM evaluation_record WHERE id = ?1")
            .bind(record_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(enrollment_id) = enrollment_id else {
        return storage::refuse(tx, RetentionRefusal::NoSuchRecord).await;
    };
    let rows = sqlx::query(&format!("{HOLD_QUERY} WHERE r.hold_id IS NULL AND ((h.enrollment_id IS NULL AND h.evaluation_record_id IS NULL) OR h.enrollment_id = ?1 OR h.evaluation_record_id = ?2) ORDER BY h.id"))
        .bind(enrollment_id).bind(record_id).fetch_all(&mut *tx).await?;
    let holds = rows.iter().map(hold_from_row).collect::<Result<Vec<_>>>()?;
    tx.commit().await?;
    Ok(Ok(holds))
}

#[derive(Debug, serde::Serialize)]
pub struct ScopeOption {
    pub id: i64,
    pub label: String,
}
#[derive(Debug, serde::Serialize)]
pub struct ScopeOptions {
    pub enrollments: Vec<ScopeOption>,
    pub records: Vec<ScopeOption>,
}

/// Human-readable scope selection; ids, never labels, establish scope.
pub async fn scope_options(
    pool: &SqlitePool,
    actor: i64,
) -> Result<Result<ScopeOptions, RetentionRefusal>> {
    let mut tx = match authorized_read(pool, actor, Capability::ManageRetention).await? {
        Ok(tx) => tx,
        Err(r) => return Ok(Err(r)),
    };
    let enrollments = sqlx::query("SELECT e.id, u.display_name, p.name FROM enrollment e JOIN user u ON u.id = e.user_id JOIN program_version p ON p.id = e.program_version_id ORDER BY e.id")
        .fetch_all(&mut *tx).await?.iter().map(|r| ScopeOption { id: r.get("id"), label: format!("{} — {} (enrollment {})", r.get::<String, _>("display_name"), r.get::<String, _>("name"), r.get::<i64, _>("id")) }).collect();
    let records = sqlx::query("SELECT r.id, u.display_name, f.name FROM evaluation_record r JOIN enrollment e ON e.id = r.enrollment_id JOIN user u ON u.id = e.user_id JOIN evaluation_form f ON f.id = r.evaluation_form_id ORDER BY r.id")
        .fetch_all(&mut *tx).await?.iter().map(|r| ScopeOption { id: r.get("id"), label: format!("{} — {} (record {})", r.get::<String, _>("display_name"), r.get::<String, _>("name"), r.get::<i64, _>("id")) }).collect();
    tx.commit().await?;
    Ok(Ok(ScopeOptions {
        enrollments,
        records,
    }))
}
