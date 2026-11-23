//! Record-export verification (v1 compatibility and v2 disposition-aware
//! archives; docs/formats/record-export.md; ADR 0014; ADR 0030 §7; #215).
//!
//! File-only: opens no database or data directory. Reuses
//! `disposition_evidence::verify::verify_bytes` so every portable evidence rule
//! has one owner.

use std::collections::BTreeSet;

use serde::Serialize;

use super::{
    ArchiveKind, ArchiveReport, Finding, is_lowercase_hex_hash, unlisted_entries, verify_units,
};
use crate::canonical;
use crate::disposition_evidence::{
    PortableDispositionEvidence,
    verify::{VerifiedBoundarySummary, VerifiedDispositionSummary, verify_bytes},
};
use crate::record_export::{
    ARCHIVE_MANIFEST_PATH, ArchiveManifest, ArchiveManifestV1, DISPOSITION_EVIDENCE_PATH,
    DispositionEvidenceRef, Scope, UnitEntry, canonical_json,
};
use crate::zip_container::{Archive, read_entry};

/// Verification report for an archive's embedded `disposition/evidence.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DispositionReport {
    pub path: String,
    pub dispositions: Vec<VerifiedDispositionSummary>,
    pub boundary_checkpoints: Vec<VerifiedBoundarySummary>,
    pub findings: Vec<Finding>,
}

impl DispositionReport {
    #[must_use]
    pub fn verified(&self) -> bool {
        self.findings.is_empty()
    }
}

/// Honest content-coverage classification of a verified archive (ADR 0030 §7).
/// Never claims that destroyed record bytes were verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveContentStatus {
    /// Archive carries retained content units only, all internally consistent.
    RetainedContentOnly,
    /// Archive carries verified retained units alongside lawfully disposed
    /// record evidence (whose content is unavailable for verification).
    MixedRetainedAndDisposed,
    /// Archive carries zero retained content units and at least one lawfully
    /// disposed record: the requested scope is fully disposed (never an
    /// ordinary empty or complete-content export).
    FullyDisposed,
    /// Archive carries neither retained content units nor disposition
    /// tombstones, only intentional policy boundary checkpoints: whatever the
    /// scope held before the boundary is accounted for by policy-required
    /// metadata closure alone, never claimed as disposed records and never
    /// claimed as an empty scope.
    PolicyBoundaryOnly,
    /// Archive carries verified retained units alongside intentional policy
    /// boundary checkpoints marking expired disposition metadata.
    RetainedWithPolicyBoundary,
    /// One or more integrity, structure, scope, or evidence findings exist.
    Failed,
}

/// Verifies a legacy format-version 1 record export archive (no disposition
/// evidence support; preserved for compatibility under ADR 0030).
pub(super) fn verify_record_export_v1(
    archive: &mut Archive<'_>,
    names: &[String],
    manifest_bytes: &[u8],
    report: &mut ArchiveReport,
) {
    report.kind = Some(ArchiveKind::RecordExport);
    let manifest: ArchiveManifestV1 = match serde_json::from_slice(manifest_bytes) {
        Ok(manifest) => manifest,
        Err(err) => {
            report.findings.push(Finding::ArchiveManifestUnreadable {
                detail: err.to_string(),
            });
            return;
        }
    };
    if canonical_json(&manifest).ok().as_deref() != Some(manifest_bytes) {
        report.findings.push(Finding::ManifestNotCanonical {
            path: ARCHIVE_MANIFEST_PATH.to_owned(),
        });
    }
    report.installation_id = Some(manifest.installation_id.clone());
    report.exported_at = Some(manifest.exported_at);
    report.scope = Some(manifest.scope);
    report
        .findings
        .extend(v1_scope_findings(manifest.scope, &manifest.units));
    let mut listed: BTreeSet<String> = BTreeSet::new();
    listed.insert(ARCHIVE_MANIFEST_PATH.to_owned());
    verify_units(
        archive,
        &manifest.installation_id,
        manifest.exported_at,
        &manifest.units,
        &mut listed,
        report,
    );
    unlisted_entries(names, &listed, report);
}

/// Verifies a format-version 2 disposition-aware record export archive.
pub(super) fn verify_record_export_v2(
    archive: &mut Archive<'_>,
    names: &[String],
    manifest_bytes: &[u8],
    report: &mut ArchiveReport,
) {
    report.kind = Some(ArchiveKind::RecordExport);
    let manifest: ArchiveManifest = match serde_json::from_slice(manifest_bytes) {
        Ok(manifest) => manifest,
        Err(err) => {
            report.findings.push(Finding::ArchiveManifestUnreadable {
                detail: err.to_string(),
            });
            return;
        }
    };
    if canonical_json(&manifest).ok().as_deref() != Some(manifest_bytes) {
        report.findings.push(Finding::ManifestNotCanonical {
            path: ARCHIVE_MANIFEST_PATH.to_owned(),
        });
    }
    report.installation_id = Some(manifest.installation_id.clone());
    report.exported_at = Some(manifest.exported_at);
    report.scope = Some(manifest.scope);

    let mut listed: BTreeSet<String> = BTreeSet::new();
    listed.insert(ARCHIVE_MANIFEST_PATH.to_owned());
    verify_units(
        archive,
        &manifest.installation_id,
        manifest.exported_at,
        &manifest.units,
        &mut listed,
        report,
    );

    let parsed_evidence = if let Some(ev_ref) = &manifest.disposition_evidence {
        let (doc_report, parsed) = verify_embedded_disposition_evidence(
            archive,
            ev_ref,
            &manifest.installation_id,
            manifest.exported_at,
            &mut listed,
        );
        report.disposition = Some(doc_report);
        parsed
    } else {
        None
    };

    report.findings.extend(v2_scope_findings(
        manifest.scope,
        &manifest.units,
        manifest.disposition_evidence.as_ref(),
        parsed_evidence.as_ref(),
    ));

    unlisted_entries(names, &listed, report);
}

/// Verifies `disposition/evidence.json` against its manifest ref and the
/// standalone `disposition_evidence` verifier, returning both the structured
/// `DispositionReport` and (when syntactically valid) the parsed document for
/// scope cross-checks. Reusable across archive families (`#215`, `#216`).
pub(crate) fn verify_embedded_disposition_evidence(
    archive: &mut Archive<'_>,
    ev_ref: &DispositionEvidenceRef,
    installation_id: &str,
    exported_at: i64,
    listed: &mut BTreeSet<String>,
) -> (DispositionReport, Option<PortableDispositionEvidence>) {
    let mut findings = Vec::new();
    let path = ev_ref.path.clone();
    if path != DISPOSITION_EVIDENCE_PATH {
        findings.push(Finding::DocumentPathUnexpected {
            path: path.clone(),
            expected: DISPOSITION_EVIDENCE_PATH.to_owned(),
        });
    }
    if !is_lowercase_hex_hash(&ev_ref.sha256) {
        findings.push(Finding::HashNotCanonical {
            member: "disposition_evidence.sha256",
        });
    }
    listed.insert(path.clone());

    let bytes = match read_entry(archive, &path) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            findings.push(Finding::MissingEntry { path: path.clone() });
            return (
                DispositionReport {
                    path,
                    dispositions: Vec::new(),
                    boundary_checkpoints: Vec::new(),
                    findings,
                },
                None,
            );
        }
        Err(detail) => {
            findings.push(Finding::EntryUnreadable {
                path: path.clone(),
                detail,
            });
            return (
                DispositionReport {
                    path,
                    dispositions: Vec::new(),
                    boundary_checkpoints: Vec::new(),
                    findings,
                },
                None,
            );
        }
    };

    if canonical::content_hash_hex(&bytes) != ev_ref.sha256 {
        findings.push(Finding::DocumentHashMismatch { path: path.clone() });
    }

    let ev_report = verify_bytes(&bytes);
    for finding in &ev_report.findings {
        findings.push(Finding::DocumentInvalid {
            path: path.clone(),
            detail: finding.to_string(),
        });
    }

    let parsed: Option<PortableDispositionEvidence> = serde_json::from_slice(&bytes).ok();
    if let Some(doc) = &parsed {
        if doc.installation_id != installation_id {
            findings.push(Finding::DocumentDisagrees {
                path: path.clone(),
                member: "installation_id",
            });
        }
        if doc.exported_at != exported_at {
            findings.push(Finding::DocumentDisagrees {
                path: path.clone(),
                member: "exported_at",
            });
        }
        if doc.dispositions.is_empty() && doc.boundary_checkpoints.is_empty() {
            findings.push(Finding::DocumentInvalid {
                path: path.clone(),
                detail: "disposition evidence document carries neither dispositions nor boundary checkpoints; omit disposition_evidence (null) instead".to_owned(),
            });
        }
    }

    (
        DispositionReport {
            path,
            dispositions: ev_report.dispositions,
            boundary_checkpoints: ev_report.boundary_checkpoints,
            findings,
        },
        parsed,
    )
}

/// Legacy v1 scope rules: no empty archive, exact version cardinality, and
/// record-id match.
fn v1_scope_findings(scope: Scope, units: &[UnitEntry]) -> Vec<Finding> {
    let mut findings = Vec::new();
    if units.is_empty() {
        findings.push(Finding::NoUnits);
    }
    match scope {
        Scope::Version {
            record_id,
            version_number,
        } => {
            if units.len() != 1 {
                findings.push(Finding::ScopeCardinality {
                    expected: 1,
                    listed: units.len(),
                });
            }
            for entry in units {
                if (entry.record_id, entry.version_number) != (record_id, version_number) {
                    findings.push(Finding::UnitOutsideScope {
                        path: entry.path.clone(),
                    });
                }
            }
        }
        Scope::Record { record_id } => {
            for entry in units {
                if entry.record_id != record_id {
                    findings.push(Finding::UnitOutsideScope {
                        path: entry.path.clone(),
                    });
                }
            }
        }
        Scope::Enrollment { .. } | Scope::Installation => {}
    }
    findings
}

/// Version 2 scope and disposition cross-checks:
/// - A record cannot appear as both a retained unit and a disposed tombstone.
/// - Empty `units` is permitted ONLY when valid disposition evidence (or for
///   enrollment/installation scopes, boundary checkpoints) accounts for the
///   scope — never for an ordinary empty export.
/// - Every listed unit and disposed entry must belong to the declared scope.
#[allow(clippy::too_many_lines)]
fn v2_scope_findings(
    scope: Scope,
    units: &[UnitEntry],
    evidence_ref: Option<&DispositionEvidenceRef>,
    evidence: Option<&PortableDispositionEvidence>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let unit_record_ids: BTreeSet<i64> = units.iter().map(|u| u.record_id).collect();
    let dispositions = evidence.map_or(&[][..], |doc| doc.dispositions.as_slice());
    let checkpoints = evidence.map_or(&[][..], |doc| doc.boundary_checkpoints.as_slice());

    for entry in dispositions {
        if unit_record_ids.contains(&entry.evaluation_record_id) {
            findings.push(Finding::DispositionRecordConflict {
                record_id: entry.evaluation_record_id,
            });
        }
    }

    if units.is_empty() && evidence_ref.is_none() {
        findings.push(Finding::NoUnits);
    }

    let ev_path = evidence_ref.map_or(DISPOSITION_EVIDENCE_PATH, |r| r.path.as_str());
    match scope {
        Scope::Version {
            record_id,
            version_number,
        } => {
            for entry in units {
                if (entry.record_id, entry.version_number) != (record_id, version_number) {
                    findings.push(Finding::UnitOutsideScope {
                        path: entry.path.clone(),
                    });
                }
            }
            for entry in dispositions {
                if entry.evaluation_record_id != record_id {
                    findings.push(Finding::DocumentReference {
                        path: ev_path.to_owned(),
                        detail: format!(
                            "disposed record {} is outside version scope ({record_id}, v{version_number})",
                            entry.evaluation_record_id
                        ),
                    });
                } else if version_number < 1 || version_number > entry.version_count {
                    findings.push(Finding::DocumentReference {
                        path: ev_path.to_owned(),
                        detail: format!(
                            "requested version {version_number} is outside disposed record {record_id}'s version_count {}",
                            entry.version_count
                        ),
                    });
                }
            }
            if !checkpoints.is_empty() {
                findings.push(Finding::DocumentReference {
                    path: ev_path.to_owned(),
                    detail: "boundary checkpoints do not apply to a single-version scope"
                        .to_owned(),
                });
            }
            let matching_disposed = dispositions
                .iter()
                .filter(|d| {
                    d.evaluation_record_id == record_id
                        && version_number >= 1
                        && version_number <= d.version_count
                })
                .count();
            if units.is_empty() {
                if evidence.is_some() && matching_disposed != 1 {
                    findings.push(Finding::ScopeCardinality {
                        expected: 1,
                        listed: matching_disposed,
                    });
                }
            } else if units.len() != 1 {
                findings.push(Finding::ScopeCardinality {
                    expected: 1,
                    listed: units.len(),
                });
            }
        }
        Scope::Record { record_id } => {
            for entry in units {
                if entry.record_id != record_id {
                    findings.push(Finding::UnitOutsideScope {
                        path: entry.path.clone(),
                    });
                }
            }
            for entry in dispositions {
                if entry.evaluation_record_id != record_id {
                    findings.push(Finding::DocumentReference {
                        path: ev_path.to_owned(),
                        detail: format!(
                            "disposed record {} is outside record scope {record_id}",
                            entry.evaluation_record_id
                        ),
                    });
                }
            }
            if !checkpoints.is_empty() {
                findings.push(Finding::DocumentReference {
                    path: ev_path.to_owned(),
                    detail: "boundary checkpoints do not apply to a single-record scope".to_owned(),
                });
            }
            if units.is_empty()
                && evidence.is_some()
                && !dispositions
                    .iter()
                    .any(|d| d.evaluation_record_id == record_id)
            {
                findings.push(Finding::NoUnits);
            }
        }
        Scope::Enrollment { enrollment_id } => {
            for entry in dispositions {
                if entry.enrollment_id != enrollment_id {
                    findings.push(Finding::DocumentReference {
                        path: ev_path.to_owned(),
                        detail: format!(
                            "disposed record {} belongs to enrollment {}, outside scope enrollment {enrollment_id}",
                            entry.evaluation_record_id, entry.enrollment_id
                        ),
                    });
                }
            }
            if units.is_empty()
                && evidence.is_some()
                && dispositions.is_empty()
                && checkpoints.is_empty()
            {
                findings.push(Finding::NoUnits);
            }
        }
        Scope::Installation => {
            if units.is_empty()
                && evidence.is_some()
                && dispositions.is_empty()
                && checkpoints.is_empty()
            {
                findings.push(Finding::NoUnits);
            }
        }
    }
    findings
}
