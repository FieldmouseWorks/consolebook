//! Read-only exact-scope disposition preview and confirmation digest binding
//! (ADR 0020; ADR 0030; #211).
//!
//! This module performs no writes and exposes no destruction path. Every
//! evaluation runs inside a single read transaction after checking the
//! separate `execute_disposition` capability.

mod queries;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{SqliteConnection, SqlitePool};
use time::OffsetDateTime;

use super::{
    HoldKind, HoldScope, RecordClass, RetentionAction, RetentionRefusal, RetentionTrigger,
    authorized_read,
};
use crate::{backup_copy::BackupSettings, canonical, capabilities::Capability, storage};
use queries::{
    evaluate_policy, inspect_controlled_copies, read_active_holds, read_authority_snapshot,
    read_current_policies, read_dependency_counts, read_enrollment_summaries, read_hold_watermarks,
    read_open_amendment, read_referencing_summaries, read_target_row, read_target_watermarks,
    read_versions,
};

/// Confirmation digests expire 15 minutes after preview (ADR 0030 §4).
pub const PREVIEW_VALIDITY_SECONDS: i64 = 900;

const SECONDS_PER_DAY: i64 = 86_400;
const CONFIRMATION_DOMAIN: &str = "consolebook-disposition-confirmation-v1";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispositionPreviewInput {
    pub record_ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DispositionAuthoritySnapshot {
    pub actor_user_id: i64,
    pub actor_latest_event_id: Option<i64>,
    pub latest_authority_event_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DispositionVersionSummary {
    pub version_id: i64,
    pub version_number: i64,
    pub record_schema: i64,
    pub content_hash: String,
    pub chain_hash: String,
    pub finalized_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DispositionDependencyCounts {
    pub evaluation_versions: i64,
    pub acknowledgments: i64,
    pub amendments: i64,
    pub draft_snapshots: i64,
    pub review_decisions: i64,
    pub contributor_events: i64,
    pub draft_ratings: i64,
    pub draft_rating_modifiers: i64,
    pub draft_narratives: i64,
    pub evaluation_sessions: i64,
    pub summary_daily_links: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DispositionPolicySummary {
    pub id: i64,
    pub record_class: RecordClass,
    pub version_number: i64,
    pub authority: String,
    pub retention_trigger: RetentionTrigger,
    pub retention_days: i64,
    pub action: RetentionAction,
    pub trigger_at: Option<i64>,
    pub eligible_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DispositionActiveHold {
    pub id: i64,
    pub scope: HoldScope,
    pub kind: HoldKind,
    pub authority: String,
    pub reason: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DispositionBlocker {
    UnfinalizedRecord,
    OpenAmendment {
        amendment_id: i64,
        predecessor_version_id: i64,
    },
    NoPolicy {
        record_class: RecordClass,
    },
    RetainPolicy {
        policy_id: i64,
        version_number: i64,
        authority: String,
    },
    EnrollmentNotClosed {
        enrollment_id: i64,
    },
    WithinRetentionPeriod {
        policy_id: i64,
        trigger_at: i64,
        eligible_at: i64,
        remaining_seconds: i64,
    },
    ActiveHold {
        hold_id: i64,
        hold_kind: HoldKind,
        scope: HoldScope,
        authority: String,
    },
    DependentSummaryRetained {
        summary_record_ids: Vec<i64>,
    },
    DependentSummaryBlocked {
        summary_record_ids: Vec<i64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DispositionTargetPreview {
    pub record_id: i64,
    pub enrollment_id: i64,
    pub record_class: RecordClass,
    pub program_version_id: i64,
    pub evaluation_form_id: i64,
    pub revision: i64,
    pub versions: Vec<DispositionVersionSummary>,
    pub dependencies: DispositionDependencyCounts,
    pub policy: Option<DispositionPolicySummary>,
    pub active_holds: Vec<DispositionActiveHold>,
    pub referencing_summary_ids: Vec<i64>,
    pub blockers: Vec<DispositionBlocker>,
    pub eligible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CopyFileEntry {
    pub name: String,
    pub size_bytes: u64,
    pub modified_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CopyDirectoryInventory {
    pub path: String,
    pub reachable: bool,
    pub files: Vec<CopyFileEntry>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ControlledCopyInventory {
    NotConfigured,
    Inspected {
        database_present: bool,
        wal_present: bool,
        local_backups: Box<CopyDirectoryInventory>,
        local_exports: Box<CopyDirectoryInventory>,
        off_server_copy_dir: Option<Box<CopyDirectoryInventory>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UncontrolledCopyCategory {
    DownloadedRecordExports,
    DownloadedTraineePackets,
    DownloadedInstallationExports,
    ExternalBackupsAndHostSnapshots,
    PreMigrationUnlinkedNotices,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryObligation {
    WalCheckpointAndVacuum,
    DurableRecoveryLedgerUpdate,
    FreshPostDispositionSnapshotBeforeCleanup,
    ControlledBackupCopyCleanup,
    RestoreNonResurrectionEnforcement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DispositionPreview {
    pub evaluated_at: i64,
    pub expires_at: i64,
    pub eligible: bool,
    pub authority: DispositionAuthoritySnapshot,
    pub disposition_event_policy: Option<DispositionPolicySummary>,
    pub targets: Vec<DispositionTargetPreview>,
    pub controlled_copies: ControlledCopyInventory,
    pub uncontrolled_copies: Vec<UncontrolledCopyCategory>,
    pub recovery_obligations: Vec<RecoveryObligation>,
    pub confirmation_digest: String,
}

const UNCONTROLLED_COPIES: [UncontrolledCopyCategory; 5] = [
    UncontrolledCopyCategory::DownloadedRecordExports,
    UncontrolledCopyCategory::DownloadedTraineePackets,
    UncontrolledCopyCategory::DownloadedInstallationExports,
    UncontrolledCopyCategory::ExternalBackupsAndHostSnapshots,
    UncontrolledCopyCategory::PreMigrationUnlinkedNotices,
];

const RECOVERY_OBLIGATIONS: [RecoveryObligation; 5] = [
    RecoveryObligation::WalCheckpointAndVacuum,
    RecoveryObligation::DurableRecoveryLedgerUpdate,
    RecoveryObligation::FreshPostDispositionSnapshotBeforeCleanup,
    RecoveryObligation::ControlledBackupCopyCleanup,
    RecoveryObligation::RestoreNonResurrectionEnforcement,
];

/// Read-only disposition preview evaluated at the current UTC instant.
pub async fn preview_disposition(
    pool: &SqlitePool,
    actor: i64,
    input: &DispositionPreviewInput,
    backups: Option<&BackupSettings>,
) -> Result<Result<DispositionPreview, RetentionRefusal>> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    preview_disposition_at(pool, actor, input, backups, now).await
}

/// Read-only disposition preview evaluated at an explicit UTC instant `evaluated_at`.
pub async fn preview_disposition_at(
    pool: &SqlitePool,
    actor: i64,
    input: &DispositionPreviewInput,
    backups: Option<&BackupSettings>,
    evaluated_at: i64,
) -> Result<Result<DispositionPreview, RetentionRefusal>> {
    let mut tx = match authorized_read(pool, actor, Capability::ExecuteDisposition).await? {
        Ok(tx) => tx,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let result = build_preview(&mut tx, actor, input, backups, evaluated_at).await?;
    match result {
        Ok(preview) => {
            tx.commit().await?;
            Ok(Ok(preview))
        }
        Err(refusal) => storage::refuse(tx, refusal).await,
    }
}

/// Read-only confirmation verification against the current committed state.
/// Performs no mutations; proves whether a confirmation digest remains valid
/// and eligible before any future execution path can act on it.
pub async fn verify_disposition_preview(
    pool: &SqlitePool,
    actor: i64,
    input: &DispositionPreviewInput,
    confirmation_digest: &str,
    expires_at: i64,
    backups: Option<&BackupSettings>,
) -> Result<Result<DispositionPreview, RetentionRefusal>> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    verify_disposition_preview_at(
        pool,
        actor,
        input,
        confirmation_digest,
        expires_at,
        backups,
        now,
    )
    .await
}

/// Read-only confirmation verification evaluated at `evaluated_at`.
pub async fn verify_disposition_preview_at(
    pool: &SqlitePool,
    actor: i64,
    input: &DispositionPreviewInput,
    confirmation_digest: &str,
    expires_at: i64,
    backups: Option<&BackupSettings>,
    evaluated_at: i64,
) -> Result<Result<DispositionPreview, RetentionRefusal>> {
    let mut tx = match authorized_read(pool, actor, Capability::ExecuteDisposition).await? {
        Ok(tx) => tx,
        Err(refusal) => return Ok(Err(refusal)),
    };
    if evaluated_at > expires_at {
        return storage::refuse(tx, RetentionRefusal::ConfirmationExpired).await;
    }
    let preview = match build_preview(&mut tx, actor, input, backups, evaluated_at).await? {
        Ok(preview) => preview,
        Err(refusal) => return storage::refuse(tx, refusal).await,
    };
    if preview.confirmation_digest != confirmation_digest {
        return storage::refuse(tx, RetentionRefusal::StaleConfirmation).await;
    }
    if !preview.eligible {
        return storage::refuse(tx, RetentionRefusal::DispositionNotEligible).await;
    }
    tx.commit().await?;
    Ok(Ok(preview))
}

#[derive(Serialize)]
struct ConfirmationBinding {
    domain: &'static str,
    actor_user_id: i64,
    actor_execute_disposition_event_id: Option<i64>,
    latest_authority_event_id: i64,
    latest_policy_id: i64,
    current_policies: Vec<(String, i64, i64)>,
    latest_hold_id: i64,
    hold_release_count: i64,
    latest_hold_release_hold_id: i64,
    latest_hold_release_at: i64,
    scope_record_ids: Vec<i64>,
    targets: Vec<TargetBinding>,
    enrollment_summaries: Vec<SummaryDependencyBinding>,
    controlled_copies: ControlledCopyInventory,
}

#[derive(Serialize)]
struct TargetBinding {
    record_id: i64,
    enrollment_id: i64,
    record_class: String,
    revision: i64,
    versions: Vec<(i64, i64, String, String)>,
    max_amendment_id: i64,
    max_acknowledgment_id: i64,
    max_contributor_event_id: i64,
    max_review_decision_id: i64,
    max_draft_snapshot_id: i64,
    max_enrollment_event_id: i64,
    active_hold_ids: Vec<i64>,
    referencing_summary_ids: Vec<i64>,
    eligible: bool,
}

#[derive(Serialize)]
struct SummaryDependencyBinding {
    summary_record_id: i64,
    revision: i64,
    max_contributor_event_id: i64,
    max_version_id: i64,
    linked_daily_version_ids: Vec<i64>,
}

type TargetWatermarks = (i64, i64, i64, i64, i64, i64);

async fn evaluate_target(
    conn: &mut SqliteConnection,
    record_id: i64,
    selected: &BTreeSet<i64>,
    current_policies: &BTreeMap<String, queries::StoredPolicy>,
    evaluated_at: i64,
) -> Result<Option<(DispositionTargetPreview, TargetWatermarks)>> {
    let Some(row) = read_target_row(conn, record_id).await? else {
        return Ok(None);
    };
    let versions = read_versions(conn, record_id).await?;
    let dependencies = read_dependency_counts(conn, record_id).await?;
    let active_holds = read_active_holds(conn, row.enrollment_id, record_id).await?;
    let referencing_summary_ids = read_referencing_summaries(conn, record_id).await?;
    let open_amendment = read_open_amendment(conn, record_id, versions.last()).await?;
    let watermarks = read_target_watermarks(conn, record_id, row.enrollment_id).await?;

    let mut blockers = Vec::new();
    if versions.is_empty() {
        blockers.push(DispositionBlocker::UnfinalizedRecord);
    }
    if let Some((amendment_id, predecessor_version_id)) = open_amendment {
        blockers.push(DispositionBlocker::OpenAmendment {
            amendment_id,
            predecessor_version_id,
        });
    }

    let policy_summary = evaluate_policy(
        conn,
        row.record_class,
        row.enrollment_id,
        &versions,
        current_policies.get(row.record_class.as_str()),
        evaluated_at,
        &mut blockers,
    )
    .await?;

    for hold in &active_holds {
        blockers.push(DispositionBlocker::ActiveHold {
            hold_id: hold.id,
            hold_kind: hold.kind,
            scope: hold.scope.clone(),
            authority: hold.authority.clone(),
        });
    }

    let retained_summaries: Vec<i64> = referencing_summary_ids
        .iter()
        .copied()
        .filter(|id| !selected.contains(id))
        .collect();
    if !retained_summaries.is_empty() {
        blockers.push(DispositionBlocker::DependentSummaryRetained {
            summary_record_ids: retained_summaries,
        });
    }

    let target = DispositionTargetPreview {
        record_id: row.record_id,
        enrollment_id: row.enrollment_id,
        record_class: row.record_class,
        program_version_id: row.program_version_id,
        evaluation_form_id: row.evaluation_form_id,
        revision: row.revision,
        versions,
        dependencies,
        policy: policy_summary,
        active_holds,
        referencing_summary_ids,
        eligible: blockers.is_empty(),
        blockers,
    };
    Ok(Some((target, watermarks)))
}

#[allow(clippy::too_many_arguments)]
fn compute_confirmation_digest(
    actor: i64,
    authority: &DispositionAuthoritySnapshot,
    latest_policy_id: i64,
    current_policies: &BTreeMap<String, queries::StoredPolicy>,
    hold_watermarks: (i64, i64, i64, i64),
    selected: &BTreeSet<i64>,
    targets: &[DispositionTargetPreview],
    target_watermarks: &BTreeMap<i64, TargetWatermarks>,
    enrollment_summaries: Vec<SummaryDependencyBinding>,
    controlled_copies: ControlledCopyInventory,
) -> Result<String> {
    let (latest_hold_id, hold_release_count, latest_hold_release_hold_id, latest_hold_release_at) =
        hold_watermarks;
    let binding = ConfirmationBinding {
        domain: CONFIRMATION_DOMAIN,
        actor_user_id: actor,
        actor_execute_disposition_event_id: authority.actor_latest_event_id,
        latest_authority_event_id: authority.latest_authority_event_id,
        latest_policy_id,
        current_policies: current_policies
            .values()
            .map(|p| (p.record_class.as_str().to_owned(), p.id, p.version_number))
            .collect(),
        latest_hold_id,
        hold_release_count,
        latest_hold_release_hold_id,
        latest_hold_release_at,
        scope_record_ids: selected.iter().copied().collect(),
        targets: targets
            .iter()
            .map(|t| {
                let (
                    max_amendment_id,
                    max_acknowledgment_id,
                    max_contributor_event_id,
                    max_review_decision_id,
                    max_draft_snapshot_id,
                    max_enrollment_event_id,
                ) = target_watermarks[&t.record_id];
                TargetBinding {
                    record_id: t.record_id,
                    enrollment_id: t.enrollment_id,
                    record_class: t.record_class.as_str().to_owned(),
                    revision: t.revision,
                    versions: t
                        .versions
                        .iter()
                        .map(|v| {
                            (
                                v.version_id,
                                v.version_number,
                                v.content_hash.clone(),
                                v.chain_hash.clone(),
                            )
                        })
                        .collect(),
                    max_amendment_id,
                    max_acknowledgment_id,
                    max_contributor_event_id,
                    max_review_decision_id,
                    max_draft_snapshot_id,
                    max_enrollment_event_id,
                    active_hold_ids: t.active_holds.iter().map(|h| h.id).collect(),
                    referencing_summary_ids: t.referencing_summary_ids.clone(),
                    eligible: t.eligible,
                }
            })
            .collect(),
        enrollment_summaries,
        controlled_copies,
    };
    let binding_value =
        serde_json::to_value(&binding).context("serializing disposition confirmation binding")?;
    let canonical = canonical::canonical_bytes(&binding_value)?;
    Ok(canonical::content_hash_hex(&canonical))
}

async fn build_preview(
    conn: &mut SqliteConnection,
    actor: i64,
    input: &DispositionPreviewInput,
    backups: Option<&BackupSettings>,
    evaluated_at: i64,
) -> Result<Result<DispositionPreview, RetentionRefusal>> {
    if input.record_ids.is_empty() {
        return Ok(Err(RetentionRefusal::Invalid(vec![
            "select at least one record id".into(),
        ])));
    }
    let mut selected = BTreeSet::new();
    for &id in &input.record_ids {
        if id <= 0 || !selected.insert(id) {
            return Ok(Err(RetentionRefusal::Invalid(vec![
                "record ids must be positive and unique".into(),
            ])));
        }
    }

    let authority = read_authority_snapshot(conn, actor).await?;
    let (latest_policy_id, current_policies) = read_current_policies(conn).await?;
    let hold_watermarks = read_hold_watermarks(conn).await?;

    let mut targets = Vec::with_capacity(selected.len());
    let mut target_watermarks: BTreeMap<i64, TargetWatermarks> = BTreeMap::new();
    let mut enrollments = BTreeSet::new();

    for &record_id in &selected {
        let Some((target, watermarks)) =
            evaluate_target(conn, record_id, &selected, &current_policies, evaluated_at).await?
        else {
            return Ok(Err(RetentionRefusal::NoSuchRecord));
        };
        enrollments.insert(target.enrollment_id);
        target_watermarks.insert(record_id, watermarks);
        targets.push(target);
    }

    // Second pass: if a daily report's referencing summary is co-selected in the
    // batch, that summary must also be eligible; otherwise the daily report is
    // blocked so execution can never remove a daily report whose summary stays.
    let blocked_selected: BTreeSet<i64> = targets
        .iter()
        .filter(|t| !t.blockers.is_empty())
        .map(|t| t.record_id)
        .collect();
    for target in &mut targets {
        let blocked_summaries: Vec<i64> = target
            .referencing_summary_ids
            .iter()
            .copied()
            .filter(|id| selected.contains(id) && blocked_selected.contains(id))
            .collect();
        if !blocked_summaries.is_empty() {
            target
                .blockers
                .push(DispositionBlocker::DependentSummaryBlocked {
                    summary_record_ids: blocked_summaries,
                });
            target.eligible = false;
        }
    }

    let eligible = targets.iter().all(|t| t.eligible);
    let enrollment_summaries = read_enrollment_summaries(conn, &enrollments).await?;
    let controlled_copies = inspect_controlled_copies(backups);
    let confirmation_digest = compute_confirmation_digest(
        actor,
        &authority,
        latest_policy_id,
        &current_policies,
        hold_watermarks,
        &selected,
        &targets,
        &target_watermarks,
        enrollment_summaries,
        controlled_copies.clone(),
    )?;

    let disposition_event_policy = current_policies
        .get(RecordClass::DispositionEvent.as_str())
        .map(|p| DispositionPolicySummary {
            id: p.id,
            record_class: p.record_class,
            version_number: p.version_number,
            authority: p.authority.clone(),
            retention_trigger: p.retention_trigger,
            retention_days: p.retention_days,
            action: p.action,
            trigger_at: None,
            eligible_at: None,
        });

    let expires_at = evaluated_at.saturating_add(PREVIEW_VALIDITY_SECONDS);

    Ok(Ok(DispositionPreview {
        evaluated_at,
        expires_at,
        eligible,
        authority,
        disposition_event_policy,
        targets,
        controlled_copies,
        uncontrolled_copies: UNCONTROLLED_COPIES.to_vec(),
        recovery_obligations: RECOVERY_OBLIGATIONS.to_vec(),
        confirmation_digest,
    }))
}
