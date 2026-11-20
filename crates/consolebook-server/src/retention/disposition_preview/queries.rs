//! Read-only SQLite queries and controlled-copy directory inspection for
//! disposition preview (`disposition_preview.rs`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result};
use sqlx::{Row, SqliteConnection};

use super::{
    ControlledCopyInventory, CopyDirectoryInventory, CopyFileEntry, DispositionActiveHold,
    DispositionAuthoritySnapshot, DispositionBlocker, DispositionDependencyCounts,
    DispositionPolicySummary, DispositionVersionSummary, SECONDS_PER_DAY, SummaryDependencyBinding,
};
use crate::{
    backup_copy::BackupSettings,
    retention::{HoldKind, HoldScope, RecordClass, RetentionAction, RetentionTrigger},
};

pub(super) struct TargetRow {
    pub record_id: i64,
    pub enrollment_id: i64,
    pub record_class: RecordClass,
    pub program_version_id: i64,
    pub evaluation_form_id: i64,
    pub revision: i64,
}

pub(super) struct StoredPolicy {
    pub id: i64,
    pub record_class: RecordClass,
    pub version_number: i64,
    pub authority: String,
    pub retention_trigger: RetentionTrigger,
    pub retention_days: i64,
    pub action: RetentionAction,
}

pub(super) async fn read_authority_snapshot(
    conn: &mut SqliteConnection,
    actor: i64,
) -> Result<DispositionAuthoritySnapshot> {
    let actor_latest_event_id: Option<i64> = sqlx::query_scalar(
        "SELECT MAX(id) FROM retention_authority_event
         WHERE user_id = ?1 AND capability = 'execute_disposition'",
    )
    .bind(actor)
    .fetch_one(&mut *conn)
    .await
    .context("reading actor disposition authority event")?;
    let latest_authority_event_id: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM retention_authority_event")
            .fetch_one(&mut *conn)
            .await
            .context("reading latest authority event")?;
    Ok(DispositionAuthoritySnapshot {
        actor_user_id: actor,
        actor_latest_event_id,
        latest_authority_event_id,
    })
}

pub(super) async fn read_current_policies(
    conn: &mut SqliteConnection,
) -> Result<(i64, BTreeMap<String, StoredPolicy>)> {
    let latest_policy_id: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM retention_policy")
            .fetch_one(&mut *conn)
            .await
            .context("reading latest policy id")?;
    let rows = sqlx::query(
        "SELECT p.id, p.record_class, p.version_number, p.authority, p.retention_trigger,
                p.retention_days, p.action
         FROM retention_policy p
         WHERE p.version_number = (
             SELECT MAX(p2.version_number)
             FROM retention_policy p2
             WHERE p2.record_class = p.record_class
         )
         ORDER BY p.record_class",
    )
    .fetch_all(&mut *conn)
    .await
    .context("reading current retention policies")?;
    let mut map = BTreeMap::new();
    for r in rows {
        let class_str: String = r.get("record_class");
        let policy = StoredPolicy {
            id: r.get("id"),
            record_class: RecordClass::from_db(&class_str)?,
            version_number: r.get("version_number"),
            authority: r.get("authority"),
            retention_trigger: RetentionTrigger::from_db(r.get("retention_trigger"))?,
            retention_days: r.get("retention_days"),
            action: RetentionAction::from_db(r.get("action"))?,
        };
        map.insert(class_str, policy);
    }
    Ok((latest_policy_id, map))
}

pub(super) async fn read_hold_watermarks(
    conn: &mut SqliteConnection,
) -> Result<(i64, i64, i64, i64)> {
    let latest_hold_id: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM record_hold")
        .fetch_one(&mut *conn)
        .await
        .context("reading latest hold id")?;
    let (release_count, latest_release_hold_id, latest_release_at): (i64, i64, i64) =
        sqlx::query_as(
            "SELECT COUNT(*), COALESCE(MAX(hold_id), 0), COALESCE(MAX(released_at), 0)
             FROM hold_release",
        )
        .fetch_one(&mut *conn)
        .await
        .context("reading hold release watermarks")?;
    Ok((
        latest_hold_id,
        release_count,
        latest_release_hold_id,
        latest_release_at,
    ))
}

pub(super) async fn read_target_row(
    conn: &mut SqliteConnection,
    record_id: i64,
) -> Result<Option<TargetRow>> {
    let row = sqlx::query(
        "SELECT r.id, r.enrollment_id, r.program_version_id, r.evaluation_form_id, r.revision,
                f.record_type
         FROM evaluation_record r
         JOIN evaluation_form f ON f.id = r.evaluation_form_id
         WHERE r.id = ?1",
    )
    .bind(record_id)
    .fetch_optional(&mut *conn)
    .await
    .context("reading target evaluation record")?;
    let Some(r) = row else {
        return Ok(None);
    };
    Ok(Some(TargetRow {
        record_id: r.get("id"),
        enrollment_id: r.get("enrollment_id"),
        record_class: RecordClass::from_db(r.get("record_type"))?,
        program_version_id: r.get("program_version_id"),
        evaluation_form_id: r.get("evaluation_form_id"),
        revision: r.get("revision"),
    }))
}

pub(super) async fn read_versions(
    conn: &mut SqliteConnection,
    record_id: i64,
) -> Result<Vec<DispositionVersionSummary>> {
    let rows = sqlx::query(
        "SELECT id, version_number, record_schema, content_hash, chain_hash, finalized_at
         FROM evaluation_version
         WHERE evaluation_record_id = ?1
         ORDER BY version_number ASC",
    )
    .bind(record_id)
    .fetch_all(&mut *conn)
    .await
    .context("reading finalized versions")?;
    Ok(rows
        .iter()
        .map(|r| DispositionVersionSummary {
            version_id: r.get("id"),
            version_number: r.get("version_number"),
            record_schema: r.get("record_schema"),
            content_hash: r.get("content_hash"),
            chain_hash: r.get("chain_hash"),
            finalized_at: r.get("finalized_at"),
        })
        .collect())
}

pub(super) async fn read_open_amendment(
    conn: &mut SqliteConnection,
    record_id: i64,
    latest_version: Option<&DispositionVersionSummary>,
) -> Result<Option<(i64, i64)>> {
    let Some(latest) = latest_version else {
        return Ok(None);
    };
    let row: Option<(i64, i64)> = sqlx::query_as(
        "SELECT id, predecessor_version_id
         FROM amendment
         WHERE evaluation_record_id = ?1 AND predecessor_version_id = ?2",
    )
    .bind(record_id)
    .bind(latest.version_id)
    .fetch_optional(&mut *conn)
    .await
    .context("checking open amendment")?;
    Ok(row)
}

pub(super) async fn read_dependency_counts(
    conn: &mut SqliteConnection,
    record_id: i64,
) -> Result<DispositionDependencyCounts> {
    let evaluation_versions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM evaluation_version WHERE evaluation_record_id = ?1",
    )
    .bind(record_id)
    .fetch_one(&mut *conn)
    .await?;
    let acknowledgments: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM acknowledgment a
         JOIN evaluation_version v ON v.id = a.evaluation_version_id
         WHERE v.evaluation_record_id = ?1",
    )
    .bind(record_id)
    .fetch_one(&mut *conn)
    .await?;
    let amendments: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM amendment WHERE evaluation_record_id = ?1")
            .bind(record_id)
            .fetch_one(&mut *conn)
            .await?;
    let draft_snapshots: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM draft_snapshot WHERE evaluation_record_id = ?1")
            .bind(record_id)
            .fetch_one(&mut *conn)
            .await?;
    let review_decisions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM review_decision WHERE evaluation_record_id = ?1")
            .bind(record_id)
            .fetch_one(&mut *conn)
            .await?;
    let contributor_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM contributor_event WHERE evaluation_record_id = ?1",
    )
    .bind(record_id)
    .fetch_one(&mut *conn)
    .await?;
    let draft_ratings: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM draft_rating WHERE evaluation_record_id = ?1")
            .bind(record_id)
            .fetch_one(&mut *conn)
            .await?;
    let draft_rating_modifiers: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM draft_rating_modifier m
         JOIN draft_rating r ON r.id = m.draft_rating_id
         WHERE r.evaluation_record_id = ?1",
    )
    .bind(record_id)
    .fetch_one(&mut *conn)
    .await?;
    let draft_narratives: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM draft_narrative WHERE evaluation_record_id = ?1")
            .bind(record_id)
            .fetch_one(&mut *conn)
            .await?;
    let evaluation_sessions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM evaluation_session WHERE evaluation_record_id = ?1",
    )
    .bind(record_id)
    .fetch_one(&mut *conn)
    .await?;
    let summary_daily_links: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM summary_daily_link
         WHERE summary_record_id = ?1
            OR daily_version_id IN (
                SELECT id FROM evaluation_version WHERE evaluation_record_id = ?1
            )",
    )
    .bind(record_id)
    .fetch_one(&mut *conn)
    .await?;
    Ok(DispositionDependencyCounts {
        evaluation_versions,
        acknowledgments,
        amendments,
        draft_snapshots,
        review_decisions,
        contributor_events,
        draft_ratings,
        draft_rating_modifiers,
        draft_narratives,
        evaluation_sessions,
        summary_daily_links,
    })
}

pub(super) async fn read_active_holds(
    conn: &mut SqliteConnection,
    enrollment_id: i64,
    record_id: i64,
) -> Result<Vec<DispositionActiveHold>> {
    let rows = sqlx::query(
        "SELECT h.id, h.enrollment_id, h.evaluation_record_id, h.kind, h.authority, h.reason, h.created_at
         FROM record_hold h
         LEFT JOIN hold_release r ON r.hold_id = h.id
         WHERE r.hold_id IS NULL
           AND ((h.enrollment_id IS NULL AND h.evaluation_record_id IS NULL)
                OR h.enrollment_id = ?1
                OR h.evaluation_record_id = ?2)
         ORDER BY h.id ASC",
    )
    .bind(enrollment_id)
    .bind(record_id)
    .fetch_all(&mut *conn)
    .await
    .context("reading active holds for preview")?;
    rows.iter()
        .map(|r| {
            let scope = match (
                r.get::<Option<i64>, _>("enrollment_id"),
                r.get::<Option<i64>, _>("evaluation_record_id"),
            ) {
                (None, None) => HoldScope::Installation,
                (Some(enrollment_id), None) => HoldScope::Enrollment { enrollment_id },
                (None, Some(record_id)) => HoldScope::Record { record_id },
                _ => anyhow::bail!("invalid stored hold scope"),
            };
            Ok(DispositionActiveHold {
                id: r.get("id"),
                scope,
                kind: HoldKind::from_db(r.get("kind"))?,
                authority: r.get("authority"),
                reason: r.get("reason"),
                created_at: r.get("created_at"),
            })
        })
        .collect()
}

pub(super) async fn read_referencing_summaries(
    conn: &mut SqliteConnection,
    record_id: i64,
) -> Result<Vec<i64>> {
    sqlx::query_scalar(
        "SELECT DISTINCT l.summary_record_id
         FROM summary_daily_link l
         JOIN evaluation_version v ON v.id = l.daily_version_id
         WHERE v.evaluation_record_id = ?1
         ORDER BY l.summary_record_id ASC",
    )
    .bind(record_id)
    .fetch_all(&mut *conn)
    .await
    .context("reading referencing weekly summaries")
}

pub(super) async fn read_target_watermarks(
    conn: &mut SqliteConnection,
    record_id: i64,
    enrollment_id: i64,
) -> Result<(i64, i64, i64, i64, i64, i64)> {
    let max_amendment_id: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(id), 0) FROM amendment WHERE evaluation_record_id = ?1",
    )
    .bind(record_id)
    .fetch_one(&mut *conn)
    .await?;
    let max_acknowledgment_id: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(a.id), 0) FROM acknowledgment a
         JOIN evaluation_version v ON v.id = a.evaluation_version_id
         WHERE v.evaluation_record_id = ?1",
    )
    .bind(record_id)
    .fetch_one(&mut *conn)
    .await?;
    let max_contributor_event_id: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(id), 0) FROM contributor_event WHERE evaluation_record_id = ?1",
    )
    .bind(record_id)
    .fetch_one(&mut *conn)
    .await?;
    let max_review_decision_id: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(id), 0) FROM review_decision WHERE evaluation_record_id = ?1",
    )
    .bind(record_id)
    .fetch_one(&mut *conn)
    .await?;
    let max_draft_snapshot_id: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(id), 0) FROM draft_snapshot WHERE evaluation_record_id = ?1",
    )
    .bind(record_id)
    .fetch_one(&mut *conn)
    .await?;
    let max_enrollment_event_id: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(id), 0) FROM enrollment_event WHERE enrollment_id = ?1",
    )
    .bind(enrollment_id)
    .fetch_one(&mut *conn)
    .await?;
    Ok((
        max_amendment_id,
        max_acknowledgment_id,
        max_contributor_event_id,
        max_review_decision_id,
        max_draft_snapshot_id,
        max_enrollment_event_id,
    ))
}

pub(super) async fn read_enrollment_summaries(
    conn: &mut SqliteConnection,
    enrollments: &BTreeSet<i64>,
) -> Result<Vec<SummaryDependencyBinding>> {
    let mut out = Vec::new();
    for &enrollment_id in enrollments {
        let summary_rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT r.id, r.revision
             FROM evaluation_record r
             JOIN evaluation_form f ON f.id = r.evaluation_form_id
             WHERE r.enrollment_id = ?1 AND f.record_type = 'weekly_summary'
             ORDER BY r.id ASC",
        )
        .bind(enrollment_id)
        .fetch_all(&mut *conn)
        .await?;
        for (summary_record_id, revision) in summary_rows {
            let max_contributor_event_id: i64 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(id), 0) FROM contributor_event WHERE evaluation_record_id = ?1",
            )
            .bind(summary_record_id)
            .fetch_one(&mut *conn)
            .await?;
            let max_version_id: i64 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(id), 0) FROM evaluation_version WHERE evaluation_record_id = ?1",
            )
            .bind(summary_record_id)
            .fetch_one(&mut *conn)
            .await?;
            let linked_daily_version_ids: Vec<i64> = sqlx::query_scalar(
                "SELECT daily_version_id FROM summary_daily_link
                 WHERE summary_record_id = ?1
                 ORDER BY daily_version_id ASC",
            )
            .bind(summary_record_id)
            .fetch_all(&mut *conn)
            .await?;
            out.push(SummaryDependencyBinding {
                summary_record_id,
                revision,
                max_contributor_event_id,
                max_version_id,
                linked_daily_version_ids,
            });
        }
    }
    Ok(out)
}

pub(super) async fn evaluate_policy(
    conn: &mut SqliteConnection,
    record_class: RecordClass,
    enrollment_id: i64,
    versions: &[DispositionVersionSummary],
    policy: Option<&StoredPolicy>,
    evaluated_at: i64,
    blockers: &mut Vec<DispositionBlocker>,
) -> Result<Option<DispositionPolicySummary>> {
    let Some(policy) = policy else {
        blockers.push(DispositionBlocker::NoPolicy { record_class });
        return Ok(None);
    };
    if policy.action == RetentionAction::Retain {
        blockers.push(DispositionBlocker::RetainPolicy {
            policy_id: policy.id,
            version_number: policy.version_number,
            authority: policy.authority.clone(),
        });
        return Ok(Some(DispositionPolicySummary {
            id: policy.id,
            record_class: policy.record_class,
            version_number: policy.version_number,
            authority: policy.authority.clone(),
            retention_trigger: policy.retention_trigger,
            retention_days: policy.retention_days,
            action: policy.action,
            trigger_at: None,
            eligible_at: None,
        }));
    }

    let trigger_at = match policy.retention_trigger {
        RetentionTrigger::FinalizedAt => versions.iter().map(|v| v.finalized_at).max(),
        RetentionTrigger::EnrollmentClosedAt => {
            let closed = enrollment_closed_at(conn, enrollment_id).await?;
            if closed.is_none() {
                blockers.push(DispositionBlocker::EnrollmentNotClosed { enrollment_id });
            }
            closed
        }
        RetentionTrigger::DisposedAt => None,
    };

    let eligible_at = trigger_at.and_then(|start| {
        policy
            .retention_days
            .checked_mul(SECONDS_PER_DAY)
            .and_then(|offset| start.checked_add(offset))
    });

    if let (Some(trigger_at), Some(eligible_at)) = (trigger_at, eligible_at)
        && evaluated_at < eligible_at
    {
        blockers.push(DispositionBlocker::WithinRetentionPeriod {
            policy_id: policy.id,
            trigger_at,
            eligible_at,
            remaining_seconds: eligible_at.saturating_sub(evaluated_at),
        });
    }

    Ok(Some(DispositionPolicySummary {
        id: policy.id,
        record_class: policy.record_class,
        version_number: policy.version_number,
        authority: policy.authority.clone(),
        retention_trigger: policy.retention_trigger,
        retention_days: policy.retention_days,
        action: policy.action,
        trigger_at,
        eligible_at,
    }))
}

pub(super) async fn enrollment_closed_at(
    conn: &mut SqliteConnection,
    enrollment_id: i64,
) -> Result<Option<i64>> {
    let latest_status: Option<(String, i64)> = sqlx::query_as(
        "SELECT kind, occurred_at
         FROM enrollment_event
         WHERE enrollment_id = ?1 AND kind IN ('complete', 'withdraw', 'reinstate')
         ORDER BY id DESC
         LIMIT 1",
    )
    .bind(enrollment_id)
    .fetch_optional(&mut *conn)
    .await
    .context("reading enrollment closure status")?;
    Ok(match latest_status {
        Some((kind, occurred_at)) if kind == "complete" || kind == "withdraw" => Some(occurred_at),
        _ => None,
    })
}

pub(super) fn inspect_controlled_copies(
    backups: Option<&BackupSettings>,
) -> ControlledCopyInventory {
    let Some(settings) = backups else {
        return ControlledCopyInventory::NotConfigured;
    };
    let db = settings.data_dir.database();
    let wal = db.with_extension("db-wal");
    ControlledCopyInventory::Inspected {
        database_present: db.exists(),
        wal_present: wal.exists(),
        local_backups: Box::new(inspect_copy_dir(
            &settings.data_dir.backups(),
            is_controlled_backup_file,
        )),
        local_exports: Box::new(inspect_copy_dir(&settings.data_dir.exports(), |_| true)),
        off_server_copy_dir: settings
            .copy_dir
            .as_deref()
            .map(|dir| Box::new(inspect_copy_dir(dir, is_controlled_backup_file))),
    }
}

fn is_controlled_backup_file(name: &str) -> bool {
    let ext = Path::new(name).extension().and_then(|e| e.to_str());
    (name.starts_with("consolebook-") && matches!(ext, Some("db" | "damaged")))
        || matches!(ext, Some("partial"))
}

fn inspect_copy_dir(dir: &Path, include: fn(&str) -> bool) -> CopyDirectoryInventory {
    let path_str = dir.display().to_string();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            return CopyDirectoryInventory {
                path: path_str,
                reachable: false,
                files: Vec::new(),
                error: Some(err.to_string()),
            };
        }
    };
    let mut files = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !include(&name) {
            continue;
        }
        let modified_at = meta.modified().ok().and_then(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .ok()
                .and_then(|d| i64::try_from(d.as_secs()).ok())
        });
        files.push(CopyFileEntry {
            name,
            size_bytes: meta.len(),
            modified_at,
        });
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    CopyDirectoryInventory {
        path: path_str,
        reachable: true,
        files,
        error: None,
    }
}
