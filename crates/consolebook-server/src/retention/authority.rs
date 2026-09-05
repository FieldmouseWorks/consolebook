//! Explicit retention authority, administered under `manage_users` with reasons.
use super::{AuthorityEvent, RetentionRefusal, authorized_read, authorized_write, text_problem};
use crate::{
    audit::{self, EventKind},
    capabilities::{self, Capability},
    storage,
};
use anyhow::{Context, Result};
use sqlx::{Row, SqlitePool};
use time::OffsetDateTime;

pub async fn set_authority(
    pool: &SqlitePool,
    actor: i64,
    user_id: i64,
    granted: bool,
    reason: &str,
) -> Result<Result<(), RetentionRefusal>> {
    let mut tx = match authorized_write(pool, actor, Capability::ManageUsers).await? {
        Ok(tx) => tx,
        Err(r) => return Ok(Err(r)),
    };
    let mut problems = Vec::new();
    text_problem(reason, "reason", 1000, &mut problems);
    if !problems.is_empty() {
        return storage::refuse(tx, RetentionRefusal::Invalid(problems)).await;
    }
    let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM user WHERE id = ?1")
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
    if exists.is_none() {
        return storage::refuse(tx, RetentionRefusal::NoSuchUser).await;
    }
    if capabilities::user_has_on(&mut tx, user_id, Capability::ManageRetention).await? == granted {
        return storage::refuse(tx, RetentionRefusal::AuthorityUnchanged).await;
    }
    if granted {
        capabilities::grant_bundle(
            &mut tx,
            user_id,
            &[Capability::ManageRetention],
            Some(actor),
        )
        .await?;
    } else {
        sqlx::query(
            "DELETE FROM capability_grant WHERE user_id = ?1 AND capability = 'manage_retention'",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("INSERT INTO retention_authority_event (user_id, granted, actor_user_id, reason, recorded_at) VALUES (?1, ?2, ?3, ?4, ?5)")
        .bind(user_id).bind(granted).bind(actor).bind(reason.trim()).bind(OffsetDateTime::now_utc().unix_timestamp())
        .execute(&mut *tx).await.context("recording retention authority")?;
    let kind = if granted {
        EventKind::RetentionAuthorityGranted
    } else {
        EventKind::RetentionAuthorityRevoked
    };
    audit::record(&mut *tx, kind, Some(actor), Some(user_id)).await?;
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn authority_history(
    pool: &SqlitePool,
    actor: i64,
) -> Result<Result<Vec<AuthorityEvent>, RetentionRefusal>> {
    let mut tx = match authorized_read(pool, actor, Capability::ManageUsers).await? {
        Ok(tx) => tx,
        Err(r) => return Ok(Err(r)),
    };
    let rows = sqlx::query("SELECT * FROM retention_authority_event ORDER BY id DESC")
        .fetch_all(&mut *tx)
        .await?;
    let events = rows
        .iter()
        .map(|r| AuthorityEvent {
            id: r.get("id"),
            user_id: r.get("user_id"),
            granted: r.get("granted"),
            actor_user_id: r.get("actor_user_id"),
            reason: r.get("reason"),
            recorded_at: r.get("recorded_at"),
        })
        .collect();
    tx.commit().await?;
    Ok(Ok(events))
}
