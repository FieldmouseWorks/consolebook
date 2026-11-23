//! Scope-bounded disposition evidence collection for record exports (and
//! shared with trainee packets in `#216`; ADR 0030 §7; #215).
//!
//! Reads minimal `disposition_event`, `disposition_version_hash`, and
//! `disposition_boundary_checkpoint` rows inside the caller's read snapshot
//! and encodes them through `disposition_evidence::produce` so every emitted
//! evidence document passes its own independent verifier before leaving the
//! installation.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection};

use super::Scope;
use crate::canonical;
use crate::disposition_evidence::{
    self, AUTHENTICITY_SELF_CONSISTENCY_ONLY, DISPOSITION_EVIDENCE_FORMAT,
    DISPOSITION_EVIDENCE_FORMAT_VERSION, PortableBoundaryCheckpoint, PortableDisposedVersion,
    PortableDispositionEntry, PortableDispositionEvidence,
};

/// The fixed archive entry path for embedded portable disposition evidence.
pub const DISPOSITION_EVIDENCE_PATH: &str = "disposition/evidence.json";

/// Manifest reference to an archive's embedded `disposition/evidence.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispositionEvidenceRef {
    pub path: String,
    pub sha256: String,
}

/// Serialized portable disposition evidence ready to write into an archive,
/// alongside its manifest reference and summary counts.
#[derive(Debug, Clone)]
pub(crate) struct PreparedDispositionEvidence {
    pub(crate) manifest_ref: DispositionEvidenceRef,
    pub(crate) bytes: Vec<u8>,
    pub(crate) disposed_record_count: usize,
    pub(crate) boundary_checkpoint_count: usize,
}

/// Lookup result for a lawfully disposed record requested by `Record` or
/// `Version` scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DisposedRecordTarget {
    pub(crate) enrollment_id: i64,
    pub(crate) version_count: i64,
}

/// Looks up a disposed record tombstone by `evaluation_record_id`.
pub(crate) async fn lookup_disposed_record(
    conn: &mut SqliteConnection,
    record_id: i64,
) -> Result<Option<DisposedRecordTarget>> {
    let row = sqlx::query(
        "SELECT enrollment_id, version_count
         FROM disposition_event
         WHERE evaluation_record_id = ?1",
    )
    .bind(record_id)
    .fetch_optional(&mut *conn)
    .await
    .context("looking up disposition_event for record scope")?;
    Ok(row.map(|r| DisposedRecordTarget {
        enrollment_id: r.get("enrollment_id"),
        version_count: r.get("version_count"),
    }))
}

/// Whether `scope` has any disposition tombstones or (for enrollment /
/// installation scopes) policy boundary checkpoints to report.
pub(crate) async fn scope_has_disposition_evidence(
    conn: &mut SqliteConnection,
    scope: Scope,
) -> Result<bool> {
    let found: Option<i64> = match scope {
        Scope::Version {
            record_id,
            version_number,
        } => sqlx::query_scalar(
            "SELECT id FROM disposition_event
             WHERE evaluation_record_id = ?1
               AND ?2 >= 1
               AND ?2 <= version_count
             LIMIT 1",
        )
        .bind(record_id)
        .bind(version_number)
        .fetch_optional(&mut *conn)
        .await
        .context("checking disposed version scope")?,
        Scope::Record { record_id } => sqlx::query_scalar(
            "SELECT id FROM disposition_event
             WHERE evaluation_record_id = ?1
             LIMIT 1",
        )
        .bind(record_id)
        .fetch_optional(&mut *conn)
        .await
        .context("checking disposed record scope")?,
        Scope::Enrollment { enrollment_id } => {
            let has_event: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM disposition_event
                 WHERE enrollment_id = ?1
                 LIMIT 1",
            )
            .bind(enrollment_id)
            .fetch_optional(&mut *conn)
            .await
            .context("checking enrollment disposition events")?;
            if has_event.is_some() {
                has_event
            } else {
                sqlx::query_scalar("SELECT id FROM disposition_boundary_checkpoint LIMIT 1")
                    .fetch_optional(&mut *conn)
                    .await
                    .context("checking disposition boundary checkpoints")?
            }
        }
        Scope::Installation => {
            let has_event: Option<i64> =
                sqlx::query_scalar("SELECT id FROM disposition_event LIMIT 1")
                    .fetch_optional(&mut *conn)
                    .await
                    .context("checking installation disposition events")?;
            if has_event.is_some() {
                has_event
            } else {
                sqlx::query_scalar("SELECT id FROM disposition_boundary_checkpoint LIMIT 1")
                    .fetch_optional(&mut *conn)
                    .await
                    .context("checking installation disposition boundary checkpoints")?
            }
        }
    };
    Ok(found.is_some())
}

/// Collects and validates the canonical portable disposition evidence document
/// for `scope` inside the caller's read transaction. Returns `Ok(None)` when
/// neither disposition tombstones nor applicable boundary checkpoints exist for
/// the scope.
pub(crate) async fn collect_disposition_evidence(
    conn: &mut SqliteConnection,
    installation_id: &str,
    exported_at: i64,
    scope: Scope,
) -> Result<Option<PreparedDispositionEvidence>> {
    let dispositions = collect_entries(conn, scope).await?;
    let boundary_checkpoints = match scope {
        Scope::Version { .. } | Scope::Record { .. } => Vec::new(),
        Scope::Enrollment { .. } | Scope::Installation => collect_checkpoints(conn).await?,
    };
    if dispositions.is_empty() && boundary_checkpoints.is_empty() {
        return Ok(None);
    }

    let disposed_record_count = dispositions.len();
    let boundary_checkpoint_count = boundary_checkpoints.len();
    let document = PortableDispositionEvidence {
        format: DISPOSITION_EVIDENCE_FORMAT.to_owned(),
        format_version: DISPOSITION_EVIDENCE_FORMAT_VERSION,
        installation_id: installation_id.to_owned(),
        exported_at,
        authenticity: AUTHENTICITY_SELF_CONSISTENCY_ONLY.to_owned(),
        dispositions,
        boundary_checkpoints,
    };
    let bytes = disposition_evidence::produce(&document)?;
    let sha256 = canonical::content_hash_hex(&bytes);
    Ok(Some(PreparedDispositionEvidence {
        manifest_ref: DispositionEvidenceRef {
            path: DISPOSITION_EVIDENCE_PATH.to_owned(),
            sha256,
        },
        bytes,
        disposed_record_count,
        boundary_checkpoint_count,
    }))
}

async fn collect_entries(
    conn: &mut SqliteConnection,
    scope: Scope,
) -> Result<Vec<PortableDispositionEntry>> {
    let rows = match scope {
        Scope::Version {
            record_id,
            version_number,
        } => sqlx::query(
            "SELECT id, evaluation_record_id, enrollment_id, record_class, record_policy_id,
                    metadata_policy_id, version_count, earliest_finalized_at,
                    latest_finalized_at, hashes_retained, authority_reference,
                    reason_code, method, outcome, disposed_at
             FROM disposition_event
             WHERE evaluation_record_id = ?1
               AND ?2 >= 1
               AND ?2 <= version_count
             ORDER BY evaluation_record_id ASC",
        )
        .bind(record_id)
        .bind(version_number)
        .fetch_all(&mut *conn)
        .await
        .context("reading disposed version evidence")?,
        Scope::Record { record_id } => sqlx::query(
            "SELECT id, evaluation_record_id, enrollment_id, record_class, record_policy_id,
                    metadata_policy_id, version_count, earliest_finalized_at,
                    latest_finalized_at, hashes_retained, authority_reference,
                    reason_code, method, outcome, disposed_at
             FROM disposition_event
             WHERE evaluation_record_id = ?1
             ORDER BY evaluation_record_id ASC",
        )
        .bind(record_id)
        .fetch_all(&mut *conn)
        .await
        .context("reading disposed record evidence")?,
        Scope::Enrollment { enrollment_id } => sqlx::query(
            "SELECT id, evaluation_record_id, enrollment_id, record_class, record_policy_id,
                    metadata_policy_id, version_count, earliest_finalized_at,
                    latest_finalized_at, hashes_retained, authority_reference,
                    reason_code, method, outcome, disposed_at
             FROM disposition_event
             WHERE enrollment_id = ?1
             ORDER BY evaluation_record_id ASC",
        )
        .bind(enrollment_id)
        .fetch_all(&mut *conn)
        .await
        .context("reading enrollment disposition evidence")?,
        Scope::Installation => sqlx::query(
            "SELECT id, evaluation_record_id, enrollment_id, record_class, record_policy_id,
                    metadata_policy_id, version_count, earliest_finalized_at,
                    latest_finalized_at, hashes_retained, authority_reference,
                    reason_code, method, outcome, disposed_at
             FROM disposition_event
             ORDER BY evaluation_record_id ASC",
        )
        .fetch_all(&mut *conn)
        .await
        .context("reading installation disposition evidence")?,
    };

    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        let event_id: i64 = row.get("id");
        let hashes_retained: i64 = row.get("hashes_retained");
        let versions = if hashes_retained != 0 {
            collect_version_hashes(conn, event_id).await?
        } else {
            Vec::new()
        };
        entries.push(PortableDispositionEntry {
            evaluation_record_id: row.get("evaluation_record_id"),
            enrollment_id: row.get("enrollment_id"),
            record_class: row.get("record_class"),
            record_policy_id: row.get("record_policy_id"),
            metadata_policy_id: row.get("metadata_policy_id"),
            version_count: row.get("version_count"),
            earliest_finalized_at: row.get("earliest_finalized_at"),
            latest_finalized_at: row.get("latest_finalized_at"),
            hashes_retained: hashes_retained != 0,
            authority_reference: row.get("authority_reference"),
            reason_code: row.get("reason_code"),
            method: row.get("method"),
            outcome: row.get("outcome"),
            disposed_at: row.get("disposed_at"),
            versions,
        });
    }
    Ok(entries)
}

async fn collect_version_hashes(
    conn: &mut SqliteConnection,
    disposition_event_id: i64,
) -> Result<Vec<PortableDisposedVersion>> {
    let rows = sqlx::query(
        "SELECT version_number, record_schema, content_hash, chain_hash,
                predecessor_content_hash, finalized_at
         FROM disposition_version_hash
         WHERE disposition_event_id = ?1
         ORDER BY version_number ASC",
    )
    .bind(disposition_event_id)
    .fetch_all(&mut *conn)
    .await
    .context("reading disposition_version_hash rows")?;

    Ok(rows
        .into_iter()
        .map(|row| PortableDisposedVersion {
            version_number: row.get("version_number"),
            record_schema: row.get("record_schema"),
            content_hash: row.get("content_hash"),
            chain_hash: row.get("chain_hash"),
            predecessor_content_hash: row.get("predecessor_content_hash"),
            finalized_at: row.get("finalized_at"),
        })
        .collect())
}

async fn collect_checkpoints(
    conn: &mut SqliteConnection,
) -> Result<Vec<PortableBoundaryCheckpoint>> {
    let rows = sqlx::query(
        "SELECT id, metadata_policy_id, authority_reference, reason_code,
                expired_event_count, boundary_through_disposed_at, recorded_at
         FROM disposition_boundary_checkpoint
         ORDER BY id ASC",
    )
    .fetch_all(&mut *conn)
    .await
    .context("reading disposition_boundary_checkpoint rows")?;

    Ok(rows
        .into_iter()
        .map(|row| PortableBoundaryCheckpoint {
            checkpoint_id: row.get("id"),
            metadata_policy_id: row.get("metadata_policy_id"),
            authority_reference: row.get("authority_reference"),
            reason_code: row.get("reason_code"),
            expired_event_count: row.get("expired_event_count"),
            boundary_through_disposed_at: row.get("boundary_through_disposed_at"),
            recorded_at: row.get("recorded_at"),
        })
        .collect())
}
