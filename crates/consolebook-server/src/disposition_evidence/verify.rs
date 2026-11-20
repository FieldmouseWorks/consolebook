//! Independent parser and verifier for portable disposition evidence documents
//! (`docs/formats/disposition-evidence.md`; ADR 0030; #212).
//!
//! Distinguishes:
//! - retained hash-lineage continuity (never content verification),
//! - unavailable content and hashes, and
//! - policy-required metadata boundary checkpoints (never silent omission).
//!
//! Never claims cryptographic signatures or authenticity Consolebook does not
//! provide.

use std::collections::BTreeSet;
use std::fmt;

use serde::Serialize;
use serde_json::{Map, Value};

use super::{
    AUTHENTICITY_SELF_CONSISTENCY_ONLY, DISPOSITION_EVIDENCE_FORMAT,
    DISPOSITION_EVIDENCE_FORMAT_VERSION, DisposedRecordClass, DispositionMethod,
    DispositionOutcome, DispositionReasonCode, PortableBoundaryCheckpoint,
    PortableDispositionEntry, PortableDispositionEvidence,
};
use crate::canonical;

const ROOT_ALLOWLIST: &[&str] = &[
    "authenticity",
    "boundary_checkpoints",
    "dispositions",
    "exported_at",
    "format",
    "format_version",
    "installation_id",
];

const ENTRY_ALLOWLIST: &[&str] = &[
    "authority_reference",
    "disposed_at",
    "earliest_finalized_at",
    "enrollment_id",
    "evaluation_record_id",
    "hashes_retained",
    "latest_finalized_at",
    "metadata_policy_id",
    "method",
    "outcome",
    "reason_code",
    "record_class",
    "record_policy_id",
    "version_count",
    "versions",
];

const VERSION_ALLOWLIST: &[&str] = &[
    "chain_hash",
    "content_hash",
    "finalized_at",
    "predecessor_content_hash",
    "record_schema",
    "version_number",
];

const CHECKPOINT_ALLOWLIST: &[&str] = &[
    "authority_reference",
    "boundary_through_disposed_at",
    "checkpoint_id",
    "expired_event_count",
    "metadata_policy_id",
    "reason_code",
    "recorded_at",
];

/// Honest verification state for one disposed record entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DisposedEntryVerdict {
    /// Version hashes were retained and form a continuous `1..=N` predecessor
    /// chain, but record content is lawfully disposed and unavailable for
    /// content verification.
    DisposedHashContinuityOnly,
    /// Record was lawfully disposed without retained version hashes; neither
    /// content nor hash lineage is available for verification.
    DisposedContentAndHashesUnavailable,
}

/// Honest verification state for a metadata boundary checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryCheckpointVerdict {
    /// Policy-required metadata closure marking lawfully expired tombstones;
    /// neither silent omission nor content verification.
    IntentionalPolicyBoundary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifiedDispositionSummary {
    pub evaluation_record_id: i64,
    pub version_count: i64,
    pub verdict: DisposedEntryVerdict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifiedBoundarySummary {
    pub checkpoint_id: i64,
    pub expired_event_count: i64,
    pub boundary_through_disposed_at: i64,
    pub verdict: BoundaryCheckpointVerdict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvidenceFinding {
    UnreadableJson {
        detail: String,
    },
    NotCanonical,
    UnsupportedFormat {
        format: String,
        format_version: i64,
    },
    UnsupportedAuthenticityClaim {
        authenticity: String,
    },
    DisallowedOrUnknownField {
        path: String,
        field: String,
    },
    InvalidShape {
        path: String,
        detail: String,
    },
    OutOfOrder {
        path: String,
        detail: String,
    },
    DuplicateId {
        path: String,
        id: i64,
    },
    HashLineageBroken {
        evaluation_record_id: i64,
        detail: String,
    },
}

impl fmt::Display for EvidenceFinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnreadableJson { detail } => write!(f, "unreadable JSON: {detail}"),
            Self::NotCanonical => f.write_str("document bytes are not canonical JSON"),
            Self::UnsupportedFormat {
                format,
                format_version,
            } => write!(f, "unsupported format '{format}' version {format_version}"),
            Self::UnsupportedAuthenticityClaim { authenticity } => write!(
                f,
                "unsupported authenticity claim '{authenticity}'; only '{AUTHENTICITY_SELF_CONSISTENCY_ONLY}' is supported"
            ),
            Self::DisallowedOrUnknownField { path, field } => {
                write!(f, "{path}: disallowed or unknown field '{field}'")
            }
            Self::InvalidShape { path, detail } | Self::OutOfOrder { path, detail } => {
                write!(f, "{path}: {detail}")
            }
            Self::DuplicateId { path, id } => write!(f, "{path}: duplicate id {id}"),
            Self::HashLineageBroken {
                evaluation_record_id,
                detail,
            } => write!(f, "record {evaluation_record_id}: {detail}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidenceVerificationReport {
    pub format: Option<String>,
    pub format_version: Option<i64>,
    pub installation_id: Option<String>,
    pub authenticity: Option<String>,
    pub dispositions: Vec<VerifiedDispositionSummary>,
    pub boundary_checkpoints: Vec<VerifiedBoundarySummary>,
    pub findings: Vec<EvidenceFinding>,
}

impl EvidenceVerificationReport {
    /// True iff the document is internally consistent with the portable
    /// disposition evidence specification. This NEVER means destroyed record
    /// content was verified or cryptographically signed.
    #[must_use]
    pub fn consistent(&self) -> bool {
        self.findings.is_empty()
    }
}

#[must_use]
pub fn verify_bytes(bytes: &[u8]) -> EvidenceVerificationReport {
    let mut report = EvidenceVerificationReport {
        format: None,
        format_version: None,
        installation_id: None,
        authenticity: None,
        dispositions: Vec::new(),
        boundary_checkpoints: Vec::new(),
        findings: Vec::new(),
    };

    let value: Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(err) => {
            report.findings.push(EvidenceFinding::UnreadableJson {
                detail: err.to_string(),
            });
            return report;
        }
    };

    let Some(root) = value.as_object() else {
        report.findings.push(EvidenceFinding::InvalidShape {
            path: "$".into(),
            detail: "root must be a JSON object".into(),
        });
        return report;
    };

    // Check format & version first so unknown versions fail by name even if
    // future versions introduce new fields.
    let format = root.get("format").and_then(Value::as_str);
    let format_version = root.get("format_version").and_then(Value::as_i64);
    report.format = format.map(ToOwned::to_owned);
    report.format_version = format_version;
    match (format, format_version) {
        (Some(DISPOSITION_EVIDENCE_FORMAT), Some(DISPOSITION_EVIDENCE_FORMAT_VERSION)) => {}
        (Some(f), Some(v)) => {
            report.findings.push(EvidenceFinding::UnsupportedFormat {
                format: f.to_owned(),
                format_version: v,
            });
            return report;
        }
        _ => {
            report.findings.push(EvidenceFinding::InvalidShape {
                path: "$".into(),
                detail: "format (string) and format_version (integer) are required".into(),
            });
            return report;
        }
    }

    if canonical::canonical_bytes(&value).ok().as_deref() != Some(bytes) {
        report.findings.push(EvidenceFinding::NotCanonical);
    }

    check_all_allowlists(root, &mut report.findings);

    let parsed: PortableDispositionEvidence = match serde_json::from_value(value) {
        Ok(doc) => doc,
        Err(err) => {
            if report.findings.is_empty() {
                report.findings.push(EvidenceFinding::InvalidShape {
                    path: "$".into(),
                    detail: err.to_string(),
                });
            }
            return report;
        }
    };

    report.installation_id = Some(parsed.installation_id.clone());
    report.authenticity = Some(parsed.authenticity.clone());
    if parsed.authenticity != AUTHENTICITY_SELF_CONSISTENCY_ONLY {
        report
            .findings
            .push(EvidenceFinding::UnsupportedAuthenticityClaim {
                authenticity: parsed.authenticity.clone(),
            });
    }
    if parsed.installation_id.trim().is_empty() {
        report.findings.push(EvidenceFinding::InvalidShape {
            path: "$.installation_id".into(),
            detail: "installation_id must not be blank".into(),
        });
    }

    verify_dispositions(&parsed.dispositions, &mut report);
    verify_checkpoints(&parsed.boundary_checkpoints, &mut report);
    report
}

fn check_all_allowlists(root: &Map<String, Value>, findings: &mut Vec<EvidenceFinding>) {
    check_allowlist(root, "$", ROOT_ALLOWLIST, findings);
    if let Some(Value::Array(items)) = root.get("dispositions") {
        for (i, item) in items.iter().enumerate() {
            if let Some(obj) = item.as_object() {
                let path = format!("$.dispositions[{i}]");
                check_allowlist(obj, &path, ENTRY_ALLOWLIST, findings);
                if let Some(Value::Array(versions)) = obj.get("versions") {
                    for (j, v) in versions.iter().enumerate() {
                        if let Some(vobj) = v.as_object() {
                            check_allowlist(
                                vobj,
                                &format!("{path}.versions[{j}]"),
                                VERSION_ALLOWLIST,
                                findings,
                            );
                        }
                    }
                }
            }
        }
    }
    if let Some(Value::Array(items)) = root.get("boundary_checkpoints") {
        for (i, item) in items.iter().enumerate() {
            if let Some(obj) = item.as_object() {
                check_allowlist(
                    obj,
                    &format!("$.boundary_checkpoints[{i}]"),
                    CHECKPOINT_ALLOWLIST,
                    findings,
                );
            }
        }
    }
}

fn check_allowlist(
    obj: &Map<String, Value>,
    path: &str,
    allowlist: &[&str],
    findings: &mut Vec<EvidenceFinding>,
) {
    for key in obj.keys() {
        if !allowlist.contains(&key.as_str()) {
            findings.push(EvidenceFinding::DisallowedOrUnknownField {
                path: path.to_owned(),
                field: key.clone(),
            });
        }
    }
}

fn verify_dispositions(
    entries: &[PortableDispositionEntry],
    report: &mut EvidenceVerificationReport,
) {
    let mut seen = BTreeSet::new();
    let mut prev_id: Option<i64> = None;
    for (idx, entry) in entries.iter().enumerate() {
        let path = format!("$.dispositions[{idx}]");
        if let Some(prev) = prev_id
            && entry.evaluation_record_id <= prev
        {
            report.findings.push(EvidenceFinding::OutOfOrder {
                path: path.clone(),
                detail: "dispositions must be strictly ascending by evaluation_record_id".into(),
            });
        }
        prev_id = Some(entry.evaluation_record_id);
        if !seen.insert(entry.evaluation_record_id) {
            report.findings.push(EvidenceFinding::DuplicateId {
                path: path.clone(),
                id: entry.evaluation_record_id,
            });
        }
        check_entry_fields(entry, &path, &mut report.findings);
        check_entry_versions(entry, &mut report.findings);

        let verdict = if entry.hashes_retained {
            DisposedEntryVerdict::DisposedHashContinuityOnly
        } else {
            DisposedEntryVerdict::DisposedContentAndHashesUnavailable
        };
        report.dispositions.push(VerifiedDispositionSummary {
            evaluation_record_id: entry.evaluation_record_id,
            version_count: entry.version_count,
            verdict,
        });
    }
}

fn check_entry_fields(
    entry: &PortableDispositionEntry,
    path: &str,
    findings: &mut Vec<EvidenceFinding>,
) {
    if entry.evaluation_record_id <= 0 || entry.enrollment_id <= 0 || entry.record_policy_id <= 0 {
        findings.push(EvidenceFinding::InvalidShape {
            path: path.to_owned(),
            detail: "record, enrollment, and policy ids must be positive".into(),
        });
    }
    if let Some(meta_id) = entry.metadata_policy_id
        && meta_id <= 0
    {
        findings.push(EvidenceFinding::InvalidShape {
            path: path.to_owned(),
            detail: "metadata_policy_id must be positive when present".into(),
        });
    }
    if DisposedRecordClass::parse(&entry.record_class).is_none() {
        findings.push(EvidenceFinding::InvalidShape {
            path: path.to_owned(),
            detail: format!("invalid record_class '{}'", entry.record_class),
        });
    }
    if !matches!(
        DispositionReasonCode::parse(&entry.reason_code),
        Some(
            DispositionReasonCode::ScheduledRetentionElapsed
                | DispositionReasonCode::RestoreLedgerReconciliation
        )
    ) {
        findings.push(EvidenceFinding::InvalidShape {
            path: path.to_owned(),
            detail: format!("invalid disposition reason_code '{}'", entry.reason_code),
        });
    }
    if DispositionMethod::parse(&entry.method).is_none() {
        findings.push(EvidenceFinding::InvalidShape {
            path: path.to_owned(),
            detail: format!("invalid method '{}'", entry.method),
        });
    }
    if DispositionOutcome::parse(&entry.outcome).is_none() {
        findings.push(EvidenceFinding::InvalidShape {
            path: path.to_owned(),
            detail: format!("invalid outcome '{}'", entry.outcome),
        });
    }
    let auth_len = entry.authority_reference.trim().chars().count();
    if auth_len == 0 || auth_len > 200 {
        findings.push(EvidenceFinding::InvalidShape {
            path: path.to_owned(),
            detail: "authority_reference must contain 1..=200 non-blank characters".into(),
        });
    }
    if entry.version_count <= 0
        || entry.latest_finalized_at < entry.earliest_finalized_at
        || entry.disposed_at < entry.latest_finalized_at
    {
        findings.push(EvidenceFinding::InvalidShape {
            path: path.to_owned(),
            detail: "version_count must be positive and timestamps monotonic".into(),
        });
    }
}

fn check_entry_versions(entry: &PortableDispositionEntry, findings: &mut Vec<EvidenceFinding>) {
    if !entry.hashes_retained {
        if !entry.versions.is_empty() {
            findings.push(EvidenceFinding::HashLineageBroken {
                evaluation_record_id: entry.evaluation_record_id,
                detail: "versions must be empty when hashes_retained is false".into(),
            });
        }
        return;
    }
    if i64::try_from(entry.versions.len()).ok() != Some(entry.version_count) {
        findings.push(EvidenceFinding::HashLineageBroken {
            evaluation_record_id: entry.evaluation_record_id,
            detail: format!(
                "hashes_retained is true with version_count={}, but {} version(s) were carried",
                entry.version_count,
                entry.versions.len()
            ),
        });
        return;
    }
    let mut prev_content_hash: Option<&str> = None;
    for (idx, version) in entry.versions.iter().enumerate() {
        let expected_num = i64::try_from(idx + 1).unwrap_or(i64::MAX);
        if version.version_number != expected_num {
            findings.push(EvidenceFinding::HashLineageBroken {
                evaluation_record_id: entry.evaluation_record_id,
                detail: format!(
                    "expected version_number {expected_num}, got {}",
                    version.version_number
                ),
            });
        }
        if version.record_schema <= 0 {
            findings.push(EvidenceFinding::HashLineageBroken {
                evaluation_record_id: entry.evaluation_record_id,
                detail: format!("version {expected_num} has non-positive record_schema"),
            });
        }
        if !is_sha256_hex(&version.content_hash) || !is_sha256_hex(&version.chain_hash) {
            findings.push(EvidenceFinding::HashLineageBroken {
                evaluation_record_id: entry.evaluation_record_id,
                detail: format!("version {expected_num} carries malformed hash"),
            });
        }
        if version.predecessor_content_hash.as_deref() != prev_content_hash {
            findings.push(EvidenceFinding::HashLineageBroken {
                evaluation_record_id: entry.evaluation_record_id,
                detail: format!(
                    "version {expected_num} predecessor_content_hash disagrees with predecessor"
                ),
            });
        }
        prev_content_hash = Some(version.content_hash.as_str());
    }
}

fn verify_checkpoints(
    checkpoints: &[PortableBoundaryCheckpoint],
    report: &mut EvidenceVerificationReport,
) {
    let mut seen = BTreeSet::new();
    let mut prev_id: Option<i64> = None;
    for (idx, cp) in checkpoints.iter().enumerate() {
        let path = format!("$.boundary_checkpoints[{idx}]");
        if let Some(prev) = prev_id
            && cp.checkpoint_id <= prev
        {
            report.findings.push(EvidenceFinding::OutOfOrder {
                path: path.clone(),
                detail: "boundary_checkpoints must be strictly ascending by checkpoint_id".into(),
            });
        }
        prev_id = Some(cp.checkpoint_id);
        if !seen.insert(cp.checkpoint_id) {
            report.findings.push(EvidenceFinding::DuplicateId {
                path: path.clone(),
                id: cp.checkpoint_id,
            });
        }
        if cp.checkpoint_id <= 0
            || cp.metadata_policy_id <= 0
            || cp.expired_event_count <= 0
            || cp.recorded_at < cp.boundary_through_disposed_at
        {
            report.findings.push(EvidenceFinding::InvalidShape {
                path: path.clone(),
                detail: "checkpoint ids/counts must be positive and timestamps monotonic".into(),
            });
        }
        if DispositionReasonCode::parse(&cp.reason_code)
            != Some(DispositionReasonCode::MetadataRetentionElapsed)
        {
            report.findings.push(EvidenceFinding::InvalidShape {
                path: path.clone(),
                detail: format!("invalid checkpoint reason_code '{}'", cp.reason_code),
            });
        }
        let auth_len = cp.authority_reference.trim().chars().count();
        if auth_len == 0 || auth_len > 200 {
            report.findings.push(EvidenceFinding::InvalidShape {
                path: path.clone(),
                detail: "authority_reference must contain 1..=200 non-blank characters".into(),
            });
        }
        report.boundary_checkpoints.push(VerifiedBoundarySummary {
            checkpoint_id: cp.checkpoint_id,
            expired_event_count: cp.expired_event_count,
            boundary_through_disposed_at: cp.boundary_through_disposed_at,
            verdict: BoundaryCheckpointVerdict::IntentionalPolicyBoundary,
        });
    }
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
