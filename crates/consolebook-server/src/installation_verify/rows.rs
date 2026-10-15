//! One table document's own checks — entry, hash, canonical form, row
//! members, types, nullability, vocabularies, text forms, key order, and
//! count — and the index the cross-document checks resolve references
//! and agreements through. Every rule is read from the table
//! specification the producer reads.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{InstallationFinding, Rows, finding};
use crate::canonical;
use crate::export_verify::{Finding, TableReport};
use crate::installation_export::TableEntry;
use crate::installation_export::spec::{Column, ColumnType, Table};
use crate::zip_container::{Archive, read_entry};

/// Reads one listed document and checks it on its own, returning the
/// rows that read as objects for the cross-document checks.
pub(super) fn read_document(
    archive: &mut Archive<'_>,
    table: &'static Table,
    entry: &TableEntry,
) -> (TableReport, Option<Rows>) {
    let path = entry.path.clone();
    let mut findings = Vec::new();
    let mut rows = None;
    match read_entry(archive, &path) {
        Ok(Some(bytes)) => {
            if canonical::content_hash_hex(&bytes) != entry.sha256 {
                findings.push(Finding::DocumentHashMismatch { path: path.clone() });
            }
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(document) => {
                    match canonical::canonical_bytes(&document) {
                        Ok(again) if again == bytes => {}
                        _ => findings.push(Finding::DocumentInvalid {
                            path: path.clone(),
                            detail: "not canonical JSON".to_owned(),
                        }),
                    }
                    match document {
                        Value::Array(items) => {
                            rows = Some(check_rows(table, &path, entry, items, &mut findings));
                        }
                        _ => findings.push(Finding::DocumentInvalid {
                            path: path.clone(),
                            detail: "not an array of rows".to_owned(),
                        }),
                    }
                }
                Err(err) => findings.push(Finding::DocumentInvalid {
                    path: path.clone(),
                    detail: format!("not JSON: {err}"),
                }),
            }
        }
        Ok(None) => findings.push(Finding::MissingEntry { path: path.clone() }),
        Err(detail) => findings.push(Finding::EntryUnreadable {
            path: path.clone(),
            detail,
        }),
    }
    let carried = rows.as_ref().map(Vec::len);
    (
        TableReport {
            path,
            table: table.name.to_owned(),
            rows: carried,
            findings,
        },
        rows,
    )
}

/// Each row's members and values, the key order, and the count.
fn check_rows(
    table: &Table,
    path: &str,
    entry: &TableEntry,
    items: Vec<Value>,
    findings: &mut Vec<Finding>,
) -> Rows {
    let carried = items.len();
    if i64::try_from(carried).ok() != Some(entry.rows) {
        findings.push(finding(InstallationFinding::RowCount {
            path: path.to_owned(),
            stated: entry.rows,
            carried,
        }));
    }
    let mut rows = Vec::with_capacity(carried);
    for (index, item) in items.into_iter().enumerate() {
        let Value::Object(row) = item else {
            findings.push(finding(InstallationFinding::RowMembers {
                path: path.to_owned(),
                row: index,
                detail: "not an object".to_owned(),
            }));
            continue;
        };
        findings.extend(row_findings(table, path, index, &row));
        rows.push(row);
    }
    if let Some(index) = rows
        .windows(2)
        .position(|pair| compare_keys(table, &pair[0], &pair[1]) != Ordering::Less)
    {
        findings.push(Finding::DocumentOutOfOrder {
            path: path.to_owned(),
            detail: format!(
                "row {} does not follow row {index} in strictly ascending ({})",
                index + 1,
                table.key.join(", ")
            ),
        });
    }
    rows
}

/// A row's exact members, each value's type, nullability, vocabulary,
/// and text form, and the audit subject's shape: the one row rule the
/// producer applies before writing and the verifier applies on reading.
pub(crate) fn row_findings(
    table: &Table,
    path: &str,
    row: usize,
    object: &Map<String, Value>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for name in object.keys() {
        if table.column(name).is_none() {
            findings.push(finding(InstallationFinding::RowMembers {
                path: path.to_owned(),
                row,
                detail: format!("unexpected member {name}"),
            }));
        }
    }
    for column in table.columns {
        match object.get(column.name) {
            None => findings.push(finding(InstallationFinding::RowMembers {
                path: path.to_owned(),
                row,
                detail: format!("missing member {}", column.name),
            })),
            Some(value) => findings.extend(value_finding(column, path, row, value)),
        }
    }
    if table.name == "audit_event" {
        let kind = object.get("subject_kind");
        let id = object.get("subject_id");
        let shaped = match (kind, id) {
            (Some(Value::Null), Some(Value::Null)) => true,
            (Some(Value::String(_)), Some(id)) => id.as_i64().is_some_and(|id| id > 0),
            _ => false,
        };
        if !shaped {
            findings.push(finding(InstallationFinding::SubjectShape {
                path: path.to_owned(),
                row,
            }));
        }
    }
    findings
}

fn value_finding(column: &Column, path: &str, row: usize, value: &Value) -> Option<Finding> {
    let wrong = |detail: &str| {
        Some(finding(InstallationFinding::RowValue {
            path: path.to_owned(),
            row,
            column: column.name.to_owned(),
            detail: detail.to_owned(),
        }))
    };
    match (column.ty, value) {
        (_, Value::Null) if column.nullable => None,
        (_, Value::Null) => wrong("null where the format carries no null"),
        (ColumnType::Integer, Value::Number(number)) if number.as_i64().is_some() => None,
        (ColumnType::Integer, _) => wrong("not an integer"),
        (ColumnType::Flag, Value::Bool(_)) => None,
        (ColumnType::Flag, _) => wrong("not true or false"),
        (ColumnType::Text, Value::String(text)) => {
            if let Some(vocabulary) = column.vocabulary
                && !vocabulary.admits(text)
            {
                return Some(finding(InstallationFinding::RowVocabulary {
                    path: path.to_owned(),
                    row,
                    column: column.name.to_owned(),
                    value: text.clone(),
                }));
            }
            match column.form {
                Some(form) if !form.admits(text) => wrong(&format!("not {}", form.as_str())),
                _ => None,
            }
        }
        (ColumnType::Text, _) => wrong("not a string"),
    }
}

/// One key member, ordered as the format orders it: integers
/// numerically, strings in code-point order. A member of the wrong type
/// or missing is already a finding; it orders first so the order check
/// stays total.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum KeyPart<'a> {
    Other,
    Integer(i64),
    Text(&'a str),
}

fn key_of<'a>(table: &Table, row: &'a Map<String, Value>) -> Vec<KeyPart<'a>> {
    table
        .key
        .iter()
        .map(|name| match row.get(*name) {
            Some(Value::Number(number)) => number.as_i64().map_or(KeyPart::Other, KeyPart::Integer),
            // Rust's `str` ordering is byte order, which for UTF-8 is
            // code-point order.
            Some(Value::String(text)) => KeyPart::Text(text),
            _ => KeyPart::Other,
        })
        .collect()
}

fn compare_keys(table: &Table, left: &Map<String, Value>, right: &Map<String, Value>) -> Ordering {
    key_of(table, left).cmp(&key_of(table, right))
}

/// Every carried row by table and `id`, with its `program_version_id`
/// when it has one: what references resolve into and agreements compare.
pub(super) struct Index {
    rows: BTreeMap<&'static str, BTreeMap<i64, Option<i64>>>,
}

impl Index {
    pub(super) fn of(documents: &BTreeMap<&'static str, (TableReport, Option<Rows>)>) -> Self {
        let rows = documents
            .iter()
            .filter_map(|(name, (_, rows))| {
                let rows = rows.as_ref()?;
                let ids = rows
                    .iter()
                    .filter_map(|row| {
                        let id = row.get("id")?.as_i64()?;
                        Some((id, row.get("program_version_id").and_then(Value::as_i64)))
                    })
                    .collect();
                Some((*name, ids))
            })
            .collect();
        Self { rows }
    }

    /// Every non-null reference of every row resolves to a carried row,
    /// and every pinned reference names a row of the same program
    /// version. A reference into a document that is not carried as rows
    /// is not resolved: that document's own finding says why.
    pub(super) fn reference_findings(
        &self,
        table: &Table,
        path: &str,
        rows: &Rows,
    ) -> Vec<Finding> {
        let mut findings = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            for column in table.columns {
                let (Some(target), Some(id)) = (
                    column.references,
                    row.get(column.name).and_then(Value::as_i64),
                ) else {
                    continue;
                };
                let Some(carried) = self.rows.get(target) else {
                    continue;
                };
                match carried.get(&id) {
                    None => findings.push(finding(InstallationFinding::DanglingReference {
                        path: path.to_owned(),
                        row: index,
                        column: column.name.to_owned(),
                        target,
                        id,
                    })),
                    Some(target_version) => {
                        let own = row.get("program_version_id").and_then(Value::as_i64);
                        if column.agrees && *target_version != own {
                            findings.push(finding(InstallationFinding::AgreementViolated {
                                path: path.to_owned(),
                                row: index,
                                column: column.name.to_owned(),
                            }));
                        }
                    }
                }
            }
        }
        findings
    }
}
