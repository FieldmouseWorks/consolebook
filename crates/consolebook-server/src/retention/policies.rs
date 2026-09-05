//! Immutable policy revisions, selected by record class.
use super::{
    Policy, PolicyInput, RecordClass, RetentionAction, RetentionRefusal, RetentionTrigger,
    authorized_read, authorized_write, text_problem,
};
use crate::{
    audit::{self, EventKind, Subject},
    capabilities::Capability,
    storage,
};
use anyhow::{Context, Result};
use sqlx::{Row, SqlitePool};
use time::OffsetDateTime;

pub async fn create_policy(
    pool: &SqlitePool,
    actor: i64,
    input: &PolicyInput,
) -> Result<Result<i64, RetentionRefusal>> {
    let mut tx = match authorized_write(pool, actor, Capability::ManageRetention).await? {
        Ok(tx) => tx,
        Err(r) => return Ok(Err(r)),
    };
    let mut problems = Vec::new();
    text_problem(&input.authority, "authority", 200, &mut problems);
    text_problem(&input.reason, "reason", 1000, &mut problems);
    if !(0..=365_250).contains(&input.retention_days) {
        problems.push("retention days must be between 0 and 365250".into());
    }
    if (input.record_class == RecordClass::DispositionEvent)
        != (input.retention_trigger == RetentionTrigger::DisposedAt)
    {
        problems.push("disposition events use disposed_at; evaluations use finalized_at or enrollment_closed_at".into());
    }
    if input.action == RetentionAction::Retain && input.retention_days != 0 {
        problems.push("retain has no destruction period; use zero days".into());
    }
    if !problems.is_empty() {
        return storage::refuse(tx, RetentionRefusal::Invalid(problems)).await;
    }
    let current: Option<(i64, i64)> = sqlx::query_as("SELECT id, version_number FROM retention_policy WHERE record_class = ?1 ORDER BY version_number DESC LIMIT 1")
        .bind(input.record_class.as_str()).fetch_optional(&mut *tx).await.context("reading current retention policy")?;
    if current.map(|(id, _)| id) != input.expected_current_id {
        return storage::refuse(tx, RetentionRefusal::StalePolicy).await;
    }
    let id = sqlx::query("INSERT INTO retention_policy (record_class, version_number, supersedes_id, authority, retention_trigger, retention_days, action, reason, created_by, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)")
        .bind(input.record_class.as_str()).bind(current.map_or(1, |(_, n)| n + 1)).bind(input.expected_current_id)
        .bind(input.authority.trim()).bind(input.retention_trigger.as_str()).bind(input.retention_days)
        .bind(input.action.as_str()).bind(input.reason.trim()).bind(actor).bind(OffsetDateTime::now_utc().unix_timestamp())
        .execute(&mut *tx).await.context("creating retention policy")?.last_insert_rowid();
    audit::record_for_subject(
        &mut *tx,
        EventKind::RetentionPolicyCreated,
        Some(actor),
        None,
        Subject::RetentionPolicy(id),
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(id))
}

pub async fn list_policies(
    pool: &SqlitePool,
    actor: i64,
) -> Result<Result<Vec<Policy>, RetentionRefusal>> {
    let mut tx = match authorized_read(pool, actor, Capability::ManageRetention).await? {
        Ok(tx) => tx,
        Err(r) => return Ok(Err(r)),
    };
    let rows =
        sqlx::query("SELECT * FROM retention_policy ORDER BY record_class, version_number DESC")
            .fetch_all(&mut *tx)
            .await?;
    let policies = rows
        .iter()
        .map(|row| {
            Ok(Policy {
                id: row.get("id"),
                record_class: RecordClass::from_db(row.get("record_class"))?,
                version_number: row.get("version_number"),
                supersedes_id: row.get("supersedes_id"),
                authority: row.get("authority"),
                retention_trigger: RetentionTrigger::from_db(row.get("retention_trigger"))?,
                retention_days: row.get("retention_days"),
                action: RetentionAction::from_db(row.get("action"))?,
                reason: row.get("reason"),
                created_by: row.get("created_by"),
                created_at: row.get("created_at"),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    tx.commit().await?;
    Ok(Ok(policies))
}
