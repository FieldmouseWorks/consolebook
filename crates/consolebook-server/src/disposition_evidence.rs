//! Minimal disposition-event vocabulary and versioned portable disposition
//! evidence format (ADR 0030; docs/formats/disposition-evidence.md; #212).
//!
//! No destruction operation or metadata-expiry mutation lives here. This module
//! defines the closed vocabularies shared with migration 0018, the allowlisted
//! portable JSON document schema, its canonical serializer, and its independent
//! verifier (`verify`).

pub mod verify;

use std::fmt;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::canonical;
use crate::closed_vocabulary::closed_vocabulary;

/// Portable format discriminator; never changes.
pub const DISPOSITION_EVIDENCE_FORMAT: &str = "consolebook-disposition-evidence";
/// Bumped by any change to the portable disposition evidence schema or rules.
pub const DISPOSITION_EVIDENCE_FORMAT_VERSION: i64 = 1;
/// Honest authenticity declaration: Consolebook checks internal consistency
/// only and never claims cryptographic signatures or tamper-proof timestamps.
pub const AUTHENTICITY_SELF_CONSISTENCY_ONLY: &str = "self_consistency_only";

closed_vocabulary! {
    /// Durable execution state of a `disposition_run` row (ADR 0030 §5).
    pub enum DispositionRunState {
        DbCommittedVacuumPending => "db_committed_vacuum_pending",
        CopyCleanupPending => "copy_cleanup_pending",
        Completed => "completed",
    }
}

closed_vocabulary! {
    /// Closed diagnostic error codes recorded on an incomplete `disposition_run`.
    pub enum DispositionErrorCode {
        VacuumFailed => "vacuum_failed",
        LedgerWriteFailed => "ledger_write_failed",
        CleanSnapshotFailed => "clean_snapshot_failed",
        LocalCleanupFailed => "local_cleanup_failed",
        CopyDirUnreachable => "copy_dir_unreachable",
        CopyCleanupFailed => "copy_cleanup_failed",
    }
}

closed_vocabulary! {
    /// How disposition was performed.
    pub enum DispositionMethod {
        LawfulDestruction => "lawful_destruction",
        RestoreReconciliation => "restore_reconciliation",
    }
}

closed_vocabulary! {
    /// Outcome recorded on a disposition tombstone.
    pub enum DispositionOutcome {
        Disposed => "disposed",
    }
}

closed_vocabulary! {
    /// Closed reason codes for disposition runs, events, and boundary closures.
    /// Free-form reasons are deliberately excluded so destroyed content or
    /// personal details never leak into disposition metadata.
    pub enum DispositionReasonCode {
        ScheduledRetentionElapsed => "scheduled_retention_elapsed",
        RestoreLedgerReconciliation => "restore_ledger_reconciliation",
        MetadataRetentionElapsed => "metadata_retention_elapsed",
    }
}

closed_vocabulary! {
    /// Record classes eligible for evaluation-record disposition tombstones.
    pub enum DisposedRecordClass {
        DailyReport => "daily_report",
        WeeklySummary => "weekly_summary",
        PhaseEvaluation => "phase_evaluation",
    }
}

macro_rules! impl_serialize_as_str {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl Serialize for $ty {
                fn serialize<S: serde::Serializer>(
                    &self,
                    serializer: S,
                ) -> std::result::Result<S::Ok, S::Error> {
                    serializer.serialize_str(self.as_str())
                }
            }
        )+
    };
}

impl_serialize_as_str!(
    DispositionRunState,
    DispositionErrorCode,
    DispositionMethod,
    DispositionOutcome,
    DispositionReasonCode,
    DisposedRecordClass,
);

/// One retained version fingerprint inside a portable disposition entry.
/// Carries opaque hashes and UTC timestamps only — never narratives, ratings,
/// or presentation display names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableDisposedVersion {
    pub version_number: i64,
    pub record_schema: i64,
    pub content_hash: String,
    pub chain_hash: String,
    pub predecessor_content_hash: Option<String>,
    pub finalized_at: i64,
}

/// One disposed evaluation record's minimal portable tombstone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableDispositionEntry {
    pub evaluation_record_id: i64,
    pub enrollment_id: i64,
    pub record_class: String,
    pub record_policy_id: i64,
    pub metadata_policy_id: Option<i64>,
    pub version_count: i64,
    pub earliest_finalized_at: i64,
    pub latest_finalized_at: i64,
    pub hashes_retained: bool,
    pub authority_reference: String,
    pub reason_code: String,
    pub method: String,
    pub outcome: String,
    pub disposed_at: i64,
    pub versions: Vec<PortableDisposedVersion>,
}

impl fmt::Display for PortableDispositionEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "record {} ({}, {} version(s), policy {}, {}, hashes_retained={})",
            self.evaluation_record_id,
            self.record_class,
            self.version_count,
            self.record_policy_id,
            self.reason_code,
            self.hashes_retained
        )
    }
}

/// One policy-required metadata closure checkpoint left behind after expired
/// disposition tombstones are removed (`#218`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableBoundaryCheckpoint {
    pub checkpoint_id: i64,
    pub metadata_policy_id: i64,
    pub authority_reference: String,
    pub reason_code: String,
    pub expired_event_count: i64,
    pub boundary_through_disposed_at: i64,
    pub recorded_at: i64,
}

impl fmt::Display for PortableBoundaryCheckpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "checkpoint {} (policy {}, {} expired event(s) through {})",
            self.checkpoint_id,
            self.metadata_policy_id,
            self.expired_event_count,
            self.boundary_through_disposed_at
        )
    }
}

/// Versioned portable disposition evidence document (canonical JSON).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableDispositionEvidence {
    pub format: String,
    pub format_version: i64,
    pub installation_id: String,
    pub exported_at: i64,
    pub authenticity: String,
    pub dispositions: Vec<PortableDispositionEntry>,
    pub boundary_checkpoints: Vec<PortableBoundaryCheckpoint>,
}

/// Produces canonical JSON bytes for a validated portable disposition evidence
/// document. Refuses to emit a document that fails its own verifier rules.
pub fn produce(document: &PortableDispositionEvidence) -> Result<Vec<u8>> {
    let value = serde_json::to_value(document).context("serializing disposition evidence")?;
    let bytes = canonical::canonical_bytes(&value)?;
    let report = verify::verify_bytes(&bytes);
    if !report.consistent() {
        let reasons: Vec<String> = report.findings.iter().map(ToString::to_string).collect();
        anyhow::bail!(
            "refusing to emit invalid disposition evidence: {}",
            reasons.join("; ")
        );
    }
    Ok(bytes)
}

#[cfg(test)]
mod vocabulary_tests {
    use super::*;

    #[test]
    fn closed_vocabularies_round_trip() {
        for (all, parse) in [
            (
                DispositionRunState::ALL
                    .iter()
                    .map(|v| v.as_str())
                    .collect::<Vec<_>>(),
                (|s: &str| DispositionRunState::parse(s).is_some()) as fn(&str) -> bool,
            ),
            (
                DispositionErrorCode::ALL
                    .iter()
                    .map(|v| v.as_str())
                    .collect(),
                |s| DispositionErrorCode::parse(s).is_some(),
            ),
            (
                DispositionMethod::ALL.iter().map(|v| v.as_str()).collect(),
                |s| DispositionMethod::parse(s).is_some(),
            ),
            (
                DispositionOutcome::ALL.iter().map(|v| v.as_str()).collect(),
                |s| DispositionOutcome::parse(s).is_some(),
            ),
            (
                DispositionReasonCode::ALL
                    .iter()
                    .map(|v| v.as_str())
                    .collect(),
                |s| DispositionReasonCode::parse(s).is_some(),
            ),
            (
                DisposedRecordClass::ALL
                    .iter()
                    .map(|v| v.as_str())
                    .collect(),
                |s| DisposedRecordClass::parse(s).is_some(),
            ),
        ] {
            let mut dedup = all.clone();
            dedup.sort_unstable();
            dedup.dedup();
            assert_eq!(dedup.len(), all.len());
            for value in all {
                assert!(parse(value), "round-trip {value}");
            }
            assert!(!parse("invented_unknown_value"));
        }
    }
}
