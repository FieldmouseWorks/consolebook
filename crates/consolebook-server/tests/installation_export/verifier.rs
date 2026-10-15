//! More negative controls for the verifier, table-driven: each case
//! changes one thing in a verified archive and names the exact finding.

use serde_json::Value;

use super::negative::{
    assert_finds, first_row, installation, with_document, with_manifest, with_rows,
};
use super::*;

type Entries = Vec<(String, Vec<u8>)>;

fn path(table: &str) -> String {
    installation_export::table_path(table)
}

fn out_of_order(table: &str, row: usize, key: &str) -> Finding {
    Finding::DocumentOutOfOrder {
        path: path(table),
        detail: format!(
            "row {} does not follow row {row} in strictly ascending ({key})",
            row + 1
        ),
    }
}

/// The index of the first row `pick` accepts.
fn index_of(rows: &[Value], pick: impl Fn(&Value) -> bool) -> usize {
    rows.iter().position(pick).expect("a matching row")
}

fn instance_rows(listed: &Entries, rows: usize) -> Entries {
    with_rows(listed, "instance", |all| {
        let row = all[0].clone();
        all.clear();
        for id in 1..=rows {
            let mut row = row.clone();
            row["id"] = Value::from(id);
            all.push(row);
        }
    })
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn every_further_check_names_its_finding() {
    let p = populate().await;
    let fx = &p.fx;
    let bytes = export(fx, fx.admin_id).await;
    assert!(export_verify::verify_archive(&bytes).verified());
    let listed = entries(&bytes);
    let versions = rows_of(&listed, "evaluation_version");
    let version = |record: i64, number: i64| {
        versions
            .iter()
            .find(|row| row["evaluation_record_id"] == record && row["version_number"] == number)
            .expect("version row")
            .clone()
    };
    let amended_v1 = version(p.amended_record_id, 1);
    let amended_v2 = version(p.amended_record_id, 2);
    let summary_v1 = version(p.summary_record_id, 1);
    let grants = rows_of(&listed, "capability_grant");
    let same_user = index_of(&grants[..grants.len() - 1], |row| {
        grants
            .iter()
            .filter(|other| other["user_id"] == row["user_id"])
            .count()
            > 1
    });
    let citations = rows_of(&listed, "standards_citation");
    let task_citation = index_of(&citations, |row| !row["task_id"].is_null());
    let audits = rows_of(&listed, "audit_event");
    let with_subject = index_of(&audits, |row| !row["subject_kind"].is_null());
    let row_of = |rows: &[Value], wanted: &Value| index_of(rows, |row| row["id"] == wanted["id"]);
    let disagrees = |row: &Value, member: &'static str| {
        installation(InstallationFinding::VersionDisagreesWithUnit {
            record_id: row["evaluation_record_id"].as_i64().expect("record"),
            version_number: row["version_number"].as_i64().expect("number"),
            member,
        })
    };
    let edit_version = |wanted: &Value, edit: &dyn Fn(&mut Value)| {
        with_rows(&listed, "evaluation_version", |rows| {
            let index = row_of(rows, wanted);
            edit(&mut rows[index]);
        })
    };
    let cell = |table: &'static str, index: usize, column: &'static str, value: Value| {
        with_rows(&listed, table, move |rows| {
            rows[index][column] = value;
        })
    };

    let cases: Vec<(&str, Entries, Finding)> =
        vec![
            (
                "ledger out of order",
                with_manifest(&listed, |manifest| {
                    manifest["tables"]
                        .as_array_mut()
                        .expect("tables")
                        .swap(0, 1);
                }),
                installation(InstallationFinding::LedgerOutOfOrder { list: "tables" }),
            ),
            (
                "unknown exclusion",
                with_manifest(&listed, |manifest| {
                    manifest["excluded"].as_array_mut().expect("excluded").insert(
                    4,
                    serde_json::json!({"name": "user.invented_column", "reason": "credential"}),
                );
                }),
                installation(InstallationFinding::LedgerUnknown {
                    list: "excluded",
                    name: "user.invented_column".to_owned(),
                }),
            ),
            (
                "ledger hash not canonical",
                with_manifest(&listed, |manifest| {
                    let agency = &mut manifest["tables"][1];
                    let upper = agency["sha256"].as_str().expect("hash").to_uppercase();
                    agency["sha256"] = Value::from(upper);
                }),
                installation(InstallationFinding::LedgerHash {
                    name: "agency".to_owned(),
                }),
            ),
            (
                "document path",
                with_manifest(
                    &listed
                        .iter()
                        .map(|(name, content)| {
                            let name = if *name == path("agency") {
                                "tables/Agency.json".to_owned()
                            } else {
                                name.clone()
                            };
                            (name, content.clone())
                        })
                        .collect::<Entries>(),
                    |manifest| manifest["tables"][1]["path"] = Value::from("tables/Agency.json"),
                ),
                Finding::DocumentPathUnexpected {
                    path: "tables/Agency.json".to_owned(),
                    expected: path("agency"),
                },
            ),
            (
                "document missing",
                listed
                    .iter()
                    .filter(|(name, _)| *name != path("agency"))
                    .cloned()
                    .collect(),
                Finding::MissingEntry {
                    path: path("agency"),
                },
            ),
            (
                "document not an array",
                with_document(&listed, "agency", b"{}", None),
                Finding::DocumentInvalid {
                    path: path("agency"),
                    detail: "not an array of rows".to_owned(),
                },
            ),
            (
                "row not an object",
                with_rows(&listed, "agency", |rows| rows[0] = Value::from(1)),
                installation(InstallationFinding::RowMembers {
                    path: path("agency"),
                    row: 0,
                    detail: "not an object".to_owned(),
                }),
            ),
            (
                "text as integer",
                with_rows(&listed, "user", |rows| {
                    first_row(rows, "user").insert("username".to_owned(), Value::from(5));
                }),
                installation(InstallationFinding::RowValue {
                    path: path("user"),
                    row: 0,
                    column: "username".to_owned(),
                    detail: "not a string".to_owned(),
                }),
            ),
            (
                "instance with no row",
                instance_rows(&listed, 0),
                installation(InstallationFinding::InstanceDisagrees {
                    detail: "it holds 0 rows, not exactly one".to_owned(),
                }),
            ),
            (
                "instance with two rows",
                instance_rows(&listed, 2),
                installation(InstallationFinding::InstanceDisagrees {
                    detail: "it holds 2 rows, not exactly one".to_owned(),
                }),
            ),
            (
                "composite key order",
                with_rows(&listed, "capability_grant", |rows| {
                    rows.swap(same_user, same_user + 1);
                }),
                out_of_order("capability_grant", same_user, "user_id, capability"),
            ),
            (
                "duplicate composite key",
                with_rows(&listed, "capability_grant", |rows| {
                    let row = rows[same_user].clone();
                    rows.insert(same_user + 1, row);
                }),
                out_of_order("capability_grant", same_user, "user_id, capability"),
            ),
            (
                "content_hash not in its text form",
                // The unit states the same non-hex hash, so the form rule
                // fires rather than a row/unit disagreement.
                {
                    let bad = "Z".repeat(64);
                    let edited = edit_version(&summary_v1, &|row| {
                        row["content_hash"] = Value::from("Z".repeat(64));
                    });
                    with_manifest(&edited, |manifest| {
                        for unit in manifest["units"].as_array_mut().expect("units") {
                            if unit["record_id"] == summary_v1["evaluation_record_id"] {
                                unit["content_hash"] = Value::from(bad.clone());
                            }
                        }
                    })
                },
                installation(InstallationFinding::RowValue {
                    path: path("evaluation_version"),
                    row: row_of(&versions, &summary_v1),
                    column: "content_hash".to_owned(),
                    detail: "not 64 lowercase hex".to_owned(),
                }),
            ),
            (
                "row and unit disagree on record_schema",
                edit_version(&amended_v1, &|row| row["record_schema"] = Value::from(1)),
                disagrees(&amended_v1, "record_schema"),
            ),
            (
                "row and unit disagree on content_hash",
                edit_version(&amended_v1, &|row| {
                    row["content_hash"] = row["chain_hash"].clone();
                }),
                disagrees(&amended_v1, "content_hash"),
            ),
            (
                "a predecessor the unit does not state",
                edit_version(&amended_v1, &|row| {
                    row["predecessor_id"] = summary_v1["id"].clone();
                }),
                disagrees(&amended_v1, "predecessor_content_hash"),
            ),
            (
                "no predecessor where the unit states one",
                edit_version(&amended_v2, &|row| row["predecessor_id"] = Value::Null),
                disagrees(&amended_v2, "predecessor_content_hash"),
            ),
            (
                "a unit with two rows",
                with_rows(&listed, "evaluation_version", |rows| {
                    let next = rows
                        .iter()
                        .filter_map(|row| row["id"].as_i64())
                        .max()
                        .expect("ids")
                        + 1;
                    let mut twin = summary_v1.clone();
                    twin["id"] = Value::from(next);
                    rows.push(twin);
                }),
                installation(InstallationFinding::UnitVersionRows {
                    path: format!("records/{}/v1", p.summary_record_id),
                    rows: 2,
                }),
            ),
            (
                "agreement through a nullable reference",
                cell(
                    "standards_citation",
                    task_citation,
                    "program_version_id",
                    Value::from(p.spare_version_id),
                ),
                installation(InstallationFinding::AgreementViolated {
                    path: path("standards_citation"),
                    row: task_citation,
                    column: "task_id".to_owned(),
                }),
            ),
            (
                "unknown audit event kind",
                cell("audit_event", 0, "kind", Value::from("invented_kind")),
                installation(InstallationFinding::RowVocabulary {
                    path: path("audit_event"),
                    row: 0,
                    column: "kind".to_owned(),
                    value: "invented_kind".to_owned(),
                }),
            ),
            (
                "unknown audit subject kind",
                cell(
                    "audit_event",
                    with_subject,
                    "subject_kind",
                    Value::from("invented_subject"),
                ),
                installation(InstallationFinding::RowVocabulary {
                    path: path("audit_event"),
                    row: with_subject,
                    column: "subject_kind".to_owned(),
                    value: "invented_subject".to_owned(),
                }),
            ),
        ];
    for (what, edited, expected) in cases {
        assert_finds(&edited, &expected, what);
    }
}
