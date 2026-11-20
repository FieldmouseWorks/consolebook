//! Confirmed lawful disposition execution and idempotent post-commit retry
//! (ADR 0030 §3–§5; #214).
//!
//! This module is the single owning typed execution API. It is intentionally
//! NOT wired into HTTP or the operator UI in `#214`. Every execution re-verifies
//! `execute_disposition` capability, policy eligibility, holds, dependency
//! closure, and the exact confirmation digest inside `storage::write_tx`
//! (`BEGIN IMMEDIATE`) before committing minimal tombstones and deleting
//! contracted in-scope content, then drives residual-byte `VACUUM`, recovery
//! ledger persistence, safe clean-snapshot creation, and application-controlled
//! copy cleanup through the durable `disposition_run` state machine.

mod cleanup;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sqlx::{Sqlite, SqlitePool, Transaction};
use time::OffsetDateTime;

use super::disposition_preview::{DispositionPreview, DispositionPreviewInput, build_preview};
use super::{RetentionRefusal, authorized_write, text_problem};
use crate::audit::{self, EventKind, Subject};
use crate::backup_copy::BackupSettings;
use crate::capabilities::{self, Capability};
use crate::disposition_evidence::{
    DispositionErrorCode, DispositionMethod, DispositionOutcome, DispositionReasonCode,
    DispositionRunState,
};
use crate::disposition_recovery::{self, EXTERNAL_COPIES_NON_GUARANTEE};
use crate::storage;

/// Deterministic fault-injection points for crash/recovery boundary tests.
/// Production entrypoints always pass `DispositionExecutionFault::None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DispositionExecutionFault {
    #[default]
    None,
    /// Fail inside `BEGIN IMMEDIATE` right before commit (proves full rollback).
    BeforeDbCommit,
    /// Stop immediately after DB commit while `state = db_committed_vacuum_pending`.
    AfterDbCommitBeforeVacuum,
    /// Fail during residual-byte `VACUUM` reclamation.
    DuringVacuum,
    /// Fail during durable local recovery ledger persistence.
    DuringLedgerWrite,
    /// Fail while creating the fresh post-disposition clean snapshot.
    DuringCleanSnapshot,
    /// Fail while deleting pre-disposition local snapshots/artifacts.
    DuringLocalCleanup,
    /// Fail while cleaning pre-disposition off-server copies.
    DuringCopyCleanup,
}

/// Exact-scope confirmation input for executing lawful disposition.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispositionExecutionInput {
    pub record_ids: Vec<i64>,
    pub confirmation_digest: String,
    pub expires_at: i64,
    pub authority_reference: String,
    pub reason_code: DispositionReasonCode,
    pub retain_version_hashes: bool,
}

/// Structured, content-free outcome of executing or retrying a disposition run.
/// `is_completed()` is true ONLY when `state == DispositionRunState::Completed`
/// after `VACUUM`, durable recovery ledger sync, clean-snapshot replacement,
/// and all controlled copy cleanup have succeeded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DispositionExecutionReport {
    pub run_id: i64,
    pub state: DispositionRunState,
    pub disposed_record_ids: Vec<i64>,
    pub last_error_code: Option<DispositionErrorCode>,
    pub vacuum_completed: bool,
    pub local_ledger_updated: bool,
    pub clean_snapshot: Option<PathBuf>,
    pub removed_local_files: Vec<PathBuf>,
    pub removed_copy_files: Vec<PathBuf>,
    pub copy_cleanup_detail: Option<String>,
    pub external_copies_note: &'static str,
}

impl DispositionExecutionReport {
    #[must_use]
    pub fn is_completed(&self) -> bool {
        self.state == DispositionRunState::Completed && self.last_error_code.is_none()
    }
}

/// Executes a confirmed lawful disposition at the current UTC instant.
pub async fn execute_disposition(
    pool: &SqlitePool,
    actor: i64,
    input: &DispositionExecutionInput,
    backups: &BackupSettings,
) -> Result<Result<DispositionExecutionReport, RetentionRefusal>> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    execute_disposition_with_fault(
        pool,
        actor,
        input,
        backups,
        now,
        DispositionExecutionFault::None,
    )
    .await
}

/// Executes a confirmed lawful disposition at an explicit UTC instant `executed_at`.
pub async fn execute_disposition_at(
    pool: &SqlitePool,
    actor: i64,
    input: &DispositionExecutionInput,
    backups: &BackupSettings,
    executed_at: i64,
) -> Result<Result<DispositionExecutionReport, RetentionRefusal>> {
    execute_disposition_with_fault(
        pool,
        actor,
        input,
        backups,
        executed_at,
        DispositionExecutionFault::None,
    )
    .await
}

/// Executes a confirmed lawful disposition with an explicit fault-injection
/// point for crash/retry boundary verification.
pub async fn execute_disposition_with_fault(
    pool: &SqlitePool,
    actor: i64,
    input: &DispositionExecutionInput,
    backups: &BackupSettings,
    executed_at: i64,
    fault: DispositionExecutionFault,
) -> Result<Result<DispositionExecutionReport, RetentionRefusal>> {
    let mut tx = match authorized_write(pool, actor, Capability::ExecuteDisposition).await? {
        Ok(tx) => tx,
        Err(refusal) => return Ok(Err(refusal)),
    };

    if let Some(problems) = validate_execution_input(input) {
        return storage::refuse(tx, RetentionRefusal::Invalid(problems)).await;
    }
    if executed_at > input.expires_at {
        return storage::refuse(tx, RetentionRefusal::ConfirmationExpired).await;
    }

    let preview_input = DispositionPreviewInput {
        record_ids: input.record_ids.clone(),
    };
    let preview = match build_preview(
        &mut tx,
        actor,
        &preview_input,
        Some(backups),
        executed_at,
        input.expires_at,
    )
    .await?
    {
        Ok(preview) => preview,
        Err(refusal) => return storage::refuse(tx, refusal).await,
    };

    if preview.confirmation_digest != input.confirmation_digest {
        return storage::refuse(tx, RetentionRefusal::StaleConfirmation).await;
    }
    if !preview.eligible {
        return storage::refuse(tx, RetentionRefusal::DispositionNotEligible).await;
    }

    let (run_id, disposed_record_ids) =
        commit_db_disposition(&mut tx, actor, input, &preview, executed_at).await?;

    if fault == DispositionExecutionFault::BeforeDbCommit {
        tx.rollback()
            .await
            .context("rolling back on injected fault")?;
        bail!("injected fault before disposition DB commit");
    }

    tx.commit()
        .await
        .context("committing lawful disposition database phase")?;

    if fault == DispositionExecutionFault::AfterDbCommitBeforeVacuum {
        return Ok(Ok(DispositionExecutionReport {
            run_id,
            state: DispositionRunState::DbCommittedVacuumPending,
            disposed_record_ids,
            last_error_code: None,
            vacuum_completed: false,
            local_ledger_updated: false,
            clean_snapshot: None,
            removed_local_files: Vec::new(),
            removed_copy_files: Vec::new(),
            copy_cleanup_detail: Some(
                "stopped after DB commit before VACUUM (injected boundary)".to_owned(),
            ),
            external_copies_note: EXTERNAL_COPIES_NON_GUARANTEE,
        }));
    }

    let report = cleanup::drive_post_commit_steps(
        pool,
        actor,
        run_id,
        disposed_record_ids,
        backups,
        executed_at,
        fault,
    )
    .await?;
    Ok(Ok(report))
}

/// Idempotently retries an incomplete `disposition_run` at the current UTC
/// instant, resuming from its persisted state (`db_committed_vacuum_pending`
/// or `copy_cleanup_pending`) until all residual-byte reclamation, recovery
/// ledger persistence, clean snapshot creation, and controlled copy cleanup
/// succeed.
pub async fn retry_disposition_run(
    pool: &SqlitePool,
    actor: i64,
    run_id: i64,
    backups: &BackupSettings,
) -> Result<Result<DispositionExecutionReport, RetentionRefusal>> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    retry_disposition_run_with_fault(
        pool,
        actor,
        run_id,
        backups,
        now,
        DispositionExecutionFault::None,
    )
    .await
}

/// Idempotently retries an incomplete `disposition_run` at an explicit UTC
/// instant `retried_at`.
pub async fn retry_disposition_run_at(
    pool: &SqlitePool,
    actor: i64,
    run_id: i64,
    backups: &BackupSettings,
    retried_at: i64,
) -> Result<Result<DispositionExecutionReport, RetentionRefusal>> {
    retry_disposition_run_with_fault(
        pool,
        actor,
        run_id,
        backups,
        retried_at,
        DispositionExecutionFault::None,
    )
    .await
}

/// Idempotently retries an incomplete `disposition_run` with an explicit
/// fault-injection point for boundary verification.
pub async fn retry_disposition_run_with_fault(
    pool: &SqlitePool,
    actor: i64,
    run_id: i64,
    backups: &BackupSettings,
    retried_at: i64,
    fault: DispositionExecutionFault,
) -> Result<Result<DispositionExecutionReport, RetentionRefusal>> {
    if !capabilities::user_has(pool, actor, Capability::ExecuteDisposition).await? {
        return Ok(Err(RetentionRefusal::CapabilityRequired));
    }

    let all_runs = disposition_recovery::list_all_runs(pool).await?;
    let Some(run) = all_runs.into_iter().find(|r| r.id == run_id) else {
        return Ok(Err(RetentionRefusal::NoSuchDispositionRun));
    };
    if run.state == DispositionRunState::Completed {
        return Ok(Err(RetentionRefusal::DispositionAlreadyCompleted));
    }

    let disposed_record_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT evaluation_record_id FROM disposition_event \
         WHERE disposition_run_id = ?1 \
         ORDER BY evaluation_record_id ASC",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .context("reading disposed record ids for disposition_run")?;

    let report = cleanup::drive_post_commit_steps(
        pool,
        actor,
        run_id,
        disposed_record_ids,
        backups,
        retried_at,
        fault,
    )
    .await?;
    Ok(Ok(report))
}

fn validate_execution_input(input: &DispositionExecutionInput) -> Option<Vec<String>> {
    let mut problems = Vec::new();
    text_problem(
        &input.authority_reference,
        "authority_reference",
        200,
        &mut problems,
    );
    if input.reason_code != DispositionReasonCode::ScheduledRetentionElapsed {
        problems.push(format!(
            "unsupported disposition reason_code '{}'",
            input.reason_code.as_str()
        ));
    }
    if input.confirmation_digest.len() != 64
        || !input
            .confirmation_digest
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        problems.push("confirmation_digest must be a 64-character lowercase hex SHA-256".into());
    }
    (!problems.is_empty()).then_some(problems)
}

async fn commit_db_disposition(
    tx: &mut Transaction<'static, Sqlite>,
    actor: i64,
    input: &DispositionExecutionInput,
    preview: &DispositionPreview,
    executed_at: i64,
) -> Result<(i64, Vec<i64>)> {
    let authority_ref = input.authority_reference.trim();
    let run_id: i64 = sqlx::query(
        "INSERT INTO disposition_run \
         (actor_user_id, confirmation_digest, authority_reference, reason_code, method, state, started_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(actor)
    .bind(&input.confirmation_digest)
    .bind(authority_ref)
    .bind(input.reason_code.as_str())
    .bind(DispositionMethod::LawfulDestruction.as_str())
    .bind(DispositionRunState::DbCommittedVacuumPending.as_str())
    .bind(executed_at)
    .execute(&mut **tx)
    .await
    .context("inserting disposition_run")?
    .last_insert_rowid();

    let metadata_policy_id = preview.disposition_event_policy.as_ref().map(|p| p.id);
    let mut disposed_record_ids = Vec::with_capacity(preview.targets.len());

    // 1. Record minimal tombstones (and optional version hashes) FIRST so the
    //    schema-gated delete triggers authorize deleting exactly these records.
    for target in &preview.targets {
        insert_target_tombstone(
            tx,
            run_id,
            actor,
            authority_ref,
            input,
            metadata_policy_id,
            target,
            executed_at,
        )
        .await?;
        disposed_record_ids.push(target.record_id);
    }

    // 2. Delete all in-scope content rows across the confirmed batch in strict
    //    FK-safe order (batch-wide passes so co-selected weekly summaries break
    //    summary_daily_link before daily evaluation_version rows are deleted).
    delete_in_scope_records(tx, &disposed_record_ids).await?;

    // 3. Minimal, content-free audit event naming only the opaque run ID.
    audit::record_for_subject(
        &mut **tx,
        EventKind::DispositionExecuted,
        Some(actor),
        None,
        Subject::DispositionRun(run_id),
    )
    .await?;

    Ok((run_id, disposed_record_ids))
}

#[allow(clippy::too_many_arguments)]
async fn insert_target_tombstone(
    tx: &mut Transaction<'static, Sqlite>,
    run_id: i64,
    actor: i64,
    authority_ref: &str,
    input: &DispositionExecutionInput,
    metadata_policy_id: Option<i64>,
    target: &super::DispositionTargetPreview,
    executed_at: i64,
) -> Result<()> {
    let policy = target
        .policy
        .as_ref()
        .context("eligible disposition target must carry a policy")?;
    let earliest_finalized_at = target
        .versions
        .iter()
        .map(|v| v.finalized_at)
        .min()
        .context("eligible disposition target must have at least one version")?;
    let latest_finalized_at = target
        .versions
        .iter()
        .map(|v| v.finalized_at)
        .max()
        .context("eligible disposition target must have at least one version")?;
    let version_count =
        i64::try_from(target.versions.len()).context("version_count exceeds i64")?;

    let event_id: i64 = sqlx::query(
        "INSERT INTO disposition_event \
         (disposition_run_id, evaluation_record_id, enrollment_id, record_class, \
          record_policy_id, metadata_policy_id, version_count, earliest_finalized_at, \
          latest_finalized_at, hashes_retained, actor_user_id, authority_reference, \
          reason_code, method, outcome, disposed_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
    )
    .bind(run_id)
    .bind(target.record_id)
    .bind(target.enrollment_id)
    .bind(target.record_class.as_str())
    .bind(policy.id)
    .bind(metadata_policy_id)
    .bind(version_count)
    .bind(earliest_finalized_at)
    .bind(latest_finalized_at)
    .bind(i64::from(input.retain_version_hashes))
    .bind(actor)
    .bind(authority_ref)
    .bind(input.reason_code.as_str())
    .bind(DispositionMethod::LawfulDestruction.as_str())
    .bind(DispositionOutcome::Disposed.as_str())
    .bind(executed_at)
    .execute(&mut **tx)
    .await
    .with_context(|| {
        format!(
            "inserting disposition_event for record {}",
            target.record_id
        )
    })?
    .last_insert_rowid();

    if input.retain_version_hashes {
        let mut prev_hash: Option<&str> = None;
        for version in &target.versions {
            sqlx::query(
                "INSERT INTO disposition_version_hash \
                 (disposition_event_id, version_number, record_schema, content_hash, \
                  chain_hash, predecessor_content_hash, finalized_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .bind(event_id)
            .bind(version.version_number)
            .bind(version.record_schema)
            .bind(&version.content_hash)
            .bind(&version.chain_hash)
            .bind(prev_hash)
            .bind(version.finalized_at)
            .execute(&mut **tx)
            .await
            .with_context(|| {
                format!(
                    "inserting disposition_version_hash for record {} v{}",
                    target.record_id, version.version_number
                )
            })?;
            prev_hash = Some(version.content_hash.as_str());
        }
    }
    Ok(())
}

async fn delete_in_scope_records(
    tx: &mut Transaction<'static, Sqlite>,
    record_ids: &[i64],
) -> Result<()> {
    // Pass 1: cross-record summary links, acknowledgments, and closed amendments.
    for &record_id in record_ids {
        sqlx::query("DELETE FROM summary_daily_link WHERE summary_record_id = ?1")
            .bind(record_id)
            .execute(&mut **tx)
            .await
            .context("deleting summary_daily_link rows")?;
        sqlx::query(
            "DELETE FROM acknowledgment \
             WHERE evaluation_version_id IN ( \
                 SELECT id FROM evaluation_version WHERE evaluation_record_id = ?1 \
             )",
        )
        .bind(record_id)
        .execute(&mut **tx)
        .await
        .context("deleting acknowledgment rows")?;
        sqlx::query("DELETE FROM amendment WHERE evaluation_record_id = ?1")
            .bind(record_id)
            .execute(&mut **tx)
            .await
            .context("deleting amendment rows")?;
    }

    // Pass 2: evaluation_version rows in descending version_number order so
    // predecessor_id self-references are never violated even row-by-row.
    for &record_id in record_ids {
        let version_ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM evaluation_version \
             WHERE evaluation_record_id = ?1 \
             ORDER BY version_number DESC",
        )
        .bind(record_id)
        .fetch_all(&mut **tx)
        .await
        .context("listing evaluation_version ids for deletion")?;
        for version_id in version_ids {
            sqlx::query("DELETE FROM evaluation_version WHERE id = ?1")
                .bind(version_id)
                .execute(&mut **tx)
                .await
                .context("deleting evaluation_version row")?;
        }
    }

    // Pass 3: working copy, review snapshots/decisions, contributor attribution,
    // session links, record-linked notices, and finally evaluation_record.
    for &record_id in record_ids {
        sqlx::query(
            "DELETE FROM draft_rating_modifier \
             WHERE draft_rating_id IN ( \
                 SELECT id FROM draft_rating WHERE evaluation_record_id = ?1 \
             )",
        )
        .bind(record_id)
        .execute(&mut **tx)
        .await
        .context("deleting draft_rating_modifier rows")?;
        for (sql, label) in [
            (
                "DELETE FROM draft_rating WHERE evaluation_record_id = ?1",
                "draft_rating",
            ),
            (
                "DELETE FROM draft_narrative WHERE evaluation_record_id = ?1",
                "draft_narrative",
            ),
            (
                "DELETE FROM draft_snapshot WHERE evaluation_record_id = ?1",
                "draft_snapshot",
            ),
            (
                "DELETE FROM review_decision WHERE evaluation_record_id = ?1",
                "review_decision",
            ),
            (
                "DELETE FROM contributor_event WHERE evaluation_record_id = ?1",
                "contributor_event",
            ),
            (
                "DELETE FROM evaluation_session WHERE evaluation_record_id = ?1",
                "evaluation_session",
            ),
            (
                "DELETE FROM notice WHERE evaluation_record_id = ?1",
                "notice",
            ),
            (
                "DELETE FROM evaluation_record WHERE id = ?1",
                "evaluation_record",
            ),
        ] {
            sqlx::query(sql)
                .bind(record_id)
                .execute(&mut **tx)
                .await
                .with_context(|| format!("deleting {label} rows for record {record_id}"))?;
        }
    }

    Ok(())
}
