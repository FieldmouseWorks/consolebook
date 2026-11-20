//! Explicit retention administration (`manage_retention`) and disposition
//! execution (`execute_disposition`) authority, administered independently
//! under `manage_users` with attributed reasons (ADR 0020; ADR 0030; #211).
use super::{
    AuthorityEvent, RetentionCapability, RetentionRefusal, authorized_read, authorized_write,
    text_problem,
};
use crate::{
    audit::{self, EventKind},
    capabilities::{self, Capability},
    storage,
};
use anyhow::{Context, Result};
use sqlx::{Row, SqlitePool};
use time::OffsetDateTime;

impl RetentionCapability {
    fn capability(self) -> Capability {
        match self {
            Self::ManageRetention => Capability::ManageRetention,
            Self::ExecuteDisposition => Capability::ExecuteDisposition,
        }
    }

    fn audit_kind(self, granted: bool) -> EventKind {
        match (self, granted) {
            (Self::ManageRetention, true) => EventKind::RetentionAuthorityGranted,
            (Self::ManageRetention, false) => EventKind::RetentionAuthorityRevoked,
            (Self::ExecuteDisposition, true) => EventKind::DispositionAuthorityGranted,
            (Self::ExecuteDisposition, false) => EventKind::DispositionAuthorityRevoked,
        }
    }
}

pub async fn set_authority(
    pool: &SqlitePool,
    actor: i64,
    user_id: i64,
    granted: bool,
    reason: &str,
) -> Result<Result<(), RetentionRefusal>> {
    set_capability_authority(
        pool,
        actor,
        user_id,
        RetentionCapability::ManageRetention,
        granted,
        reason,
    )
    .await
}

pub async fn set_disposition_authority(
    pool: &SqlitePool,
    actor: i64,
    user_id: i64,
    granted: bool,
    reason: &str,
) -> Result<Result<(), RetentionRefusal>> {
    set_capability_authority(
        pool,
        actor,
        user_id,
        RetentionCapability::ExecuteDisposition,
        granted,
        reason,
    )
    .await
}

async fn set_capability_authority(
    pool: &SqlitePool,
    actor: i64,
    user_id: i64,
    target: RetentionCapability,
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
    let cap = target.capability();
    if capabilities::user_has_on(&mut tx, user_id, cap).await? == granted {
        return storage::refuse(tx, RetentionRefusal::AuthorityUnchanged).await;
    }
    if granted {
        capabilities::grant_bundle(&mut tx, user_id, &[cap], Some(actor)).await?;
    } else {
        sqlx::query("DELETE FROM capability_grant WHERE user_id = ?1 AND capability = ?2")
            .bind(user_id)
            .bind(cap.as_str())
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        "INSERT INTO retention_authority_event (user_id, capability, granted, actor_user_id, reason, recorded_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(user_id)
    .bind(target.as_str())
    .bind(granted)
    .bind(actor)
    .bind(reason.trim())
    .bind(OffsetDateTime::now_utc().unix_timestamp())
    .execute(&mut *tx)
    .await
    .context("recording retention authority")?;
    audit::record(
        &mut *tx,
        target.audit_kind(granted),
        Some(actor),
        Some(user_id),
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn authority_history(
    pool: &SqlitePool,
    actor: i64,
) -> Result<Result<Vec<AuthorityEvent>, RetentionRefusal>> {
    capability_authority_history(pool, actor, RetentionCapability::ManageRetention).await
}

pub async fn disposition_authority_history(
    pool: &SqlitePool,
    actor: i64,
) -> Result<Result<Vec<AuthorityEvent>, RetentionRefusal>> {
    capability_authority_history(pool, actor, RetentionCapability::ExecuteDisposition).await
}

async fn capability_authority_history(
    pool: &SqlitePool,
    actor: i64,
    target: RetentionCapability,
) -> Result<Result<Vec<AuthorityEvent>, RetentionRefusal>> {
    let mut tx = match authorized_read(pool, actor, Capability::ManageUsers).await? {
        Ok(tx) => tx,
        Err(r) => return Ok(Err(r)),
    };
    let rows = sqlx::query(
        "SELECT id, user_id, capability, granted, actor_user_id, reason, recorded_at
         FROM retention_authority_event
         WHERE capability = ?1
         ORDER BY id DESC",
    )
    .bind(target.as_str())
    .fetch_all(&mut *tx)
    .await?;
    let events = rows
        .iter()
        .map(|r| {
            Ok(AuthorityEvent {
                id: r.get("id"),
                user_id: r.get("user_id"),
                capability: RetentionCapability::from_db(r.get("capability"))?,
                granted: r.get("granted"),
                actor_user_id: r.get("actor_user_id"),
                reason: r.get("reason"),
                recorded_at: r.get("recorded_at"),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    tx.commit().await?;
    Ok(Ok(events))
}
