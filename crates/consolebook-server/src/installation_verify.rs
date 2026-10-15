//! Installation-export verification (ADR 0028;
//! docs/formats/installation-export.md, Verification).
//!
//! `export_verify` dispatches here when an archive's manifest names the
//! installation-export format. The container and unit checks are the
//! record export's, shared through `export_verify`; this module owns what
//! the installation export adds: the ledger as a closed set, the
//! `instance` row, every table document checked against the one table
//! specification (`installation_export::spec`) the producer reads, every
//! reference resolving and every agreement holding inside the archive,
//! and the `evaluation_version` rows agreeing with the units both ways.

mod rows;

pub(crate) use rows::row_findings;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::export_verify::{
    ArchiveKind, ArchiveReport, Finding, PredecessorLink, TableReport, is_lowercase_hex_hash,
    unlisted_entries, verify_units,
};
use crate::installation_export::{InstallationManifest, spec, table_path};
use crate::record_export::{ARCHIVE_MANIFEST_PATH, UnitEntry, canonical_json};
use crate::zip_container::Archive;

/// What only an installation export can get wrong. Wording is
/// presentation; the variant is the finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstallationFinding {
    /// A ledger list names something this format version does not put
    /// in that list.
    LedgerUnknown { list: &'static str, name: String },
    /// The ledger omits a table or exclusion this format version has.
    LedgerMissing { name: String },
    /// The ledger names one table or exclusion more than once.
    LedgerDuplicated { name: String },
    /// A ledger list is not strictly ascending by name.
    LedgerOutOfOrder { list: &'static str },
    /// A carried table's ledger `sha256` is not 64 lowercase hex
    /// characters.
    LedgerHash { name: String },
    /// An exclusion gives a reason other than the format's.
    LedgerReason { name: String, reason: String },
    /// `tables/instance.json` is not exactly one row naming the
    /// manifest's installation.
    InstanceDisagrees { detail: String },
    /// A row is not an object with exactly its table's members.
    RowMembers {
        path: String,
        row: usize,
        detail: String,
    },
    /// A member's value is not of its column's type, nullability, or
    /// fixed text form.
    RowValue {
        path: String,
        row: usize,
        column: String,
        detail: String,
    },
    /// A member's value is outside its column's closed vocabulary.
    RowVocabulary {
        path: String,
        row: usize,
        column: String,
        value: String,
    },
    /// The document carries a different number of rows than the ledger
    /// states.
    RowCount {
        path: String,
        stated: i64,
        carried: usize,
    },
    /// A reference names a row the archive does not carry.
    DanglingReference {
        path: String,
        row: usize,
        column: String,
        target: &'static str,
        id: i64,
    },
    /// A row's `program_version_id` differs from that of the row a
    /// pinned column names.
    AgreementViolated {
        path: String,
        row: usize,
        column: String,
    },
    /// `audit_event.subject_kind` and `subject_id` are not both null or
    /// both present with a positive id.
    SubjectShape { path: String, row: usize },
    /// An `evaluation_version` row has no unit.
    VersionWithoutUnit { record_id: i64, version_number: i64 },
    /// A unit has no `evaluation_version` row, or more than one.
    UnitVersionRows { path: String, rows: usize },
    /// An `evaluation_version` row and its unit disagree on a member.
    VersionDisagreesWithUnit {
        record_id: i64,
        version_number: i64,
        member: &'static str,
    },
}

impl fmt::Display for InstallationFinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LedgerUnknown { list, name } => write!(
                f,
                "the ledger's {list} list names {name}, which this format version does not put there"
            ),
            Self::LedgerMissing { name } => write!(f, "the ledger omits {name}"),
            Self::LedgerDuplicated { name } => write!(f, "the ledger names {name} more than once"),
            Self::LedgerOutOfOrder { list } => {
                write!(
                    f,
                    "the ledger's {list} list is not strictly ascending by name"
                )
            }
            Self::LedgerHash { name } => write!(
                f,
                "the ledger's sha256 for {name} is not 64 lowercase hex characters"
            ),
            Self::LedgerReason { name, reason } => write!(
                f,
                "the ledger excludes {name} for '{reason}', which is not the format's reason"
            ),
            Self::InstanceDisagrees { detail } => {
                write!(
                    f,
                    "the instance document disagrees with the manifest: {detail}"
                )
            }
            Self::RowMembers { path, row, detail } => write!(f, "{path} row {row}: {detail}"),
            Self::RowValue {
                path,
                row,
                column,
                detail,
            } => write!(f, "{path} row {row}, {column}: {detail}"),
            Self::RowVocabulary {
                path,
                row,
                column,
                value,
            } => write!(
                f,
                "{path} row {row}, {column}: '{value}' is not in the column's vocabulary"
            ),
            Self::RowCount {
                path,
                stated,
                carried,
            } => write!(
                f,
                "{path} carries {carried} row(s); the manifest states {stated}"
            ),
            Self::DanglingReference {
                path,
                row,
                column,
                target,
                id,
            } => write!(
                f,
                "{path} row {row}, {column}: {target} {id} is not carried"
            ),
            Self::AgreementViolated { path, row, column } => write!(
                f,
                "{path} row {row}: program_version_id disagrees with the row {column} names"
            ),
            Self::SubjectShape { path, row } => write!(
                f,
                "{path} row {row}: subject_kind and subject_id are not both null or both present with a positive id"
            ),
            Self::VersionWithoutUnit {
                record_id,
                version_number,
            } => write!(
                f,
                "evaluation_version for record {record_id} version {version_number} has no unit"
            ),
            Self::UnitVersionRows { path, rows } => write!(
                f,
                "unit {path} has {rows} evaluation_version row(s), not exactly one"
            ),
            Self::VersionDisagreesWithUnit {
                record_id,
                version_number,
                member,
            } => write!(
                f,
                "evaluation_version for record {record_id} version {version_number} disagrees with its unit on {member}"
            ),
        }
    }
}

fn finding(finding: InstallationFinding) -> Finding {
    Finding::Installation { finding }
}

/// One table document's rows, as far as they read as objects.
type Rows = Vec<Map<String, Value>>;

pub(crate) fn verify_installation(
    archive: &mut Archive<'_>,
    names: &[String],
    manifest_bytes: &[u8],
    report: &mut ArchiveReport,
) {
    report.kind = Some(ArchiveKind::InstallationExport);
    let manifest: InstallationManifest = match serde_json::from_slice(manifest_bytes) {
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
    report.findings.extend(ledger_findings(&manifest));
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
    // The archive carries every finalized version, so a unit whose
    // predecessor is not carried is a hole in the lineage.
    for unit in &mut report.units {
        if unit.predecessor == PredecessorLink::NotInExport {
            unit.findings.push(Finding::PredecessorNotCarried);
        }
    }

    // Pass one: each listed document, read and checked on its own.
    let mut documents: BTreeMap<&'static str, (TableReport, Option<Rows>)> = BTreeMap::new();
    for entry in &manifest.tables {
        listed.insert(entry.path.clone());
        let Some(table) = spec::table(&entry.name) else {
            continue;
        };
        if documents.contains_key(table.name) {
            continue;
        }
        let expected = table.path();
        if entry.path != expected {
            report.findings.push(Finding::DocumentPathUnexpected {
                path: entry.path.clone(),
                expected,
            });
        }
        documents.insert(table.name, rows::read_document(archive, table, entry));
    }

    // Pass two: what the documents say about one another and the units.
    let index = rows::Index::of(&documents);
    for (name, (document, rows)) in &mut documents {
        let Some(rows) = rows else { continue };
        let table = spec::table(name).expect("only specified tables are read");
        document
            .findings
            .extend(index.reference_findings(table, &document.path, rows));
    }
    report
        .findings
        .extend(instance_findings(&manifest, &documents));
    if let Some((document, Some(versions))) = documents.get_mut("evaluation_version") {
        document
            .findings
            .extend(unit_agreement_findings(&manifest.units, versions));
    }
    report
        .tables
        .extend(documents.into_values().map(|(document, _)| document));
    unlisted_entries(names, &listed, report);
}

/// The ledger as the format version's closed set: every carried table and
/// every exclusion named once, each in its own list, ascending, with the
/// format's reasons and well-formed hashes.
fn ledger_findings(manifest: &InstallationManifest) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut occurrences: BTreeMap<&str, usize> = BTreeMap::new();
    if !manifest
        .tables
        .windows(2)
        .all(|pair| pair[0].name < pair[1].name)
    {
        findings.push(finding(InstallationFinding::LedgerOutOfOrder {
            list: "tables",
        }));
    }
    if !manifest
        .excluded
        .windows(2)
        .all(|pair| pair[0].name < pair[1].name)
    {
        findings.push(finding(InstallationFinding::LedgerOutOfOrder {
            list: "excluded",
        }));
    }
    for entry in &manifest.tables {
        *occurrences.entry(&entry.name).or_default() += 1;
        if spec::table(&entry.name).is_none() {
            findings.push(finding(InstallationFinding::LedgerUnknown {
                list: "tables",
                name: entry.name.clone(),
            }));
        }
        if !is_lowercase_hex_hash(&entry.sha256) {
            findings.push(finding(InstallationFinding::LedgerHash {
                name: entry.name.clone(),
            }));
        }
    }
    for entry in &manifest.excluded {
        *occurrences.entry(&entry.name).or_default() += 1;
        match spec::exclusion(&entry.name) {
            None => findings.push(finding(InstallationFinding::LedgerUnknown {
                list: "excluded",
                name: entry.name.clone(),
            })),
            Some(exclusion) if exclusion.reason.as_str() != entry.reason => {
                findings.push(finding(InstallationFinding::LedgerReason {
                    name: entry.name.clone(),
                    reason: entry.reason.clone(),
                }));
            }
            Some(_) => {}
        }
    }
    for (name, count) in &occurrences {
        if *count > 1 {
            findings.push(finding(InstallationFinding::LedgerDuplicated {
                name: (*name).to_owned(),
            }));
        }
    }
    let carried: BTreeSet<&str> = manifest.tables.iter().map(|e| e.name.as_str()).collect();
    let excluded: BTreeSet<&str> = manifest.excluded.iter().map(|e| e.name.as_str()).collect();
    for table in &spec::TABLES {
        if !carried.contains(table.name) {
            findings.push(finding(InstallationFinding::LedgerMissing {
                name: table.name.to_owned(),
            }));
        }
    }
    for exclusion in &spec::EXCLUSIONS {
        if !excluded.contains(exclusion.name) {
            findings.push(finding(InstallationFinding::LedgerMissing {
                name: exclusion.name.to_owned(),
            }));
        }
    }
    findings
}

/// `tables/instance.json` holds exactly one row, and its
/// `installation_id` is the manifest's.
fn instance_findings(
    manifest: &InstallationManifest,
    documents: &BTreeMap<&'static str, (TableReport, Option<Rows>)>,
) -> Vec<Finding> {
    let detail = match documents.get("instance") {
        None | Some((_, None)) => {
            Some(format!("{} is not carried as rows", table_path("instance")))
        }
        Some((_, Some(rows))) if rows.len() != 1 => {
            Some(format!("it holds {} rows, not exactly one", rows.len()))
        }
        Some((_, Some(rows))) => match rows[0].get("installation_id") {
            Some(Value::String(id)) if *id == manifest.installation_id => None,
            _ => Some("its installation_id is not the manifest's".to_owned()),
        },
    };
    detail
        .map(|detail| finding(InstallationFinding::InstanceDisagrees { detail }))
        .into_iter()
        .collect()
}

/// One `evaluation_version` row's members the units must agree with.
struct VersionRow<'a> {
    record_id: i64,
    version_number: i64,
    record_schema: Option<i64>,
    content_hash: Option<&'a str>,
    chain_hash: Option<&'a str>,
    predecessor_id: Option<i64>,
}

/// The `evaluation_version` rows and the units, both ways: each row has
/// exactly one unit at its identity and each unit exactly one row; the
/// unit's schema and hashes are the row's; and the unit's predecessor
/// hash is the content hash of the row `predecessor_id` names, `null`
/// exactly when it is `null`.
fn unit_agreement_findings(units: &[UnitEntry], versions: &Rows) -> Vec<Finding> {
    let mut findings = Vec::new();
    let content_by_id: BTreeMap<i64, &str> = versions
        .iter()
        .filter_map(|row| Some((row.get("id")?.as_i64()?, row.get("content_hash")?.as_str()?)))
        .collect();
    let parsed: Vec<VersionRow<'_>> = versions
        .iter()
        .filter_map(|row| {
            Some(VersionRow {
                record_id: row.get("evaluation_record_id")?.as_i64()?,
                version_number: row.get("version_number")?.as_i64()?,
                record_schema: row.get("record_schema").and_then(Value::as_i64),
                content_hash: row.get("content_hash").and_then(Value::as_str),
                chain_hash: row.get("chain_hash").and_then(Value::as_str),
                predecessor_id: row.get("predecessor_id").and_then(Value::as_i64),
            })
        })
        .collect();
    let by_identity: BTreeMap<(i64, i64), &UnitEntry> = units
        .iter()
        .map(|unit| ((unit.record_id, unit.version_number), unit))
        .collect();
    for row in &parsed {
        let identity = (row.record_id, row.version_number);
        let Some(unit) = by_identity.get(&identity) else {
            findings.push(finding(InstallationFinding::VersionWithoutUnit {
                record_id: row.record_id,
                version_number: row.version_number,
            }));
            continue;
        };
        let mut disagree = |member: &'static str| {
            findings.push(finding(InstallationFinding::VersionDisagreesWithUnit {
                record_id: row.record_id,
                version_number: row.version_number,
                member,
            }));
        };
        if row.record_schema != Some(unit.record_schema) {
            disagree("record_schema");
        }
        if row.content_hash != Some(unit.content_hash.as_str()) {
            disagree("content_hash");
        }
        if row.chain_hash != Some(unit.chain_hash.as_str()) {
            disagree("chain_hash");
        }
        let expected = match row.predecessor_id {
            None => Some(None),
            // A predecessor that does not resolve is a dangling reference,
            // reported as one.
            Some(id) => content_by_id.get(&id).map(|hash| Some(*hash)),
        };
        if let Some(expected) = expected
            && expected != unit.predecessor_content_hash.as_deref()
        {
            disagree("predecessor_content_hash");
        }
    }
    // Rows per identity, counted once: linear in rows and units.
    let mut rows_by_identity: BTreeMap<(i64, i64), usize> = BTreeMap::new();
    for row in &parsed {
        *rows_by_identity
            .entry((row.record_id, row.version_number))
            .or_default() += 1;
    }
    for unit in units {
        let rows = rows_by_identity
            .get(&(unit.record_id, unit.version_number))
            .copied()
            .unwrap_or(0);
        if rows != 1 {
            findings.push(finding(InstallationFinding::UnitVersionRows {
                path: unit.path.clone(),
                rows,
            }));
        }
    }
    findings
}
