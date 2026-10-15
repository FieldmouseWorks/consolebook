//! Negative controls: each starts from a verified archive, changes one
//! thing, keeps every other hash and count consistent where the targeted
//! check would otherwise be masked, and names the finding it must
//! produce.

use serde_json::Value;

use super::*;

fn installation(finding: InstallationFinding) -> Finding {
    Finding::Installation { finding }
}

fn edit_json(bytes: &[u8], edit: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(bytes).expect("json");
    edit(&mut value);
    canonical::canonical_bytes(&value).expect("canonical")
}

/// The archive with the manifest edited and re-serialized canonically.
fn with_manifest(
    listed: &[(String, Vec<u8>)],
    edit: impl FnOnce(&mut Value),
) -> Vec<(String, Vec<u8>)> {
    let mut edit = Some(edit);
    listed
        .iter()
        .map(|(name, content)| {
            if name == ARCHIVE_MANIFEST_PATH {
                let edit = edit.take().expect("one manifest");
                (name.clone(), edit_json(content, edit))
            } else {
                (name.clone(), content.clone())
            }
        })
        .collect()
}

/// The archive with one table document's rows edited; the manifest's
/// `sha256` and `rows` follow, so only what the rows say can be wrong.
fn with_rows(
    listed: &[(String, Vec<u8>)],
    table: &str,
    edit: impl FnOnce(&mut Vec<Value>),
) -> Vec<(String, Vec<u8>)> {
    let mut rows = rows_of(listed, table);
    edit(&mut rows);
    let bytes = canonical::canonical_bytes(&Value::Array(rows.clone())).expect("canonical");
    with_document(listed, table, &bytes, Some(rows.len()))
}

/// The archive with one document's bytes replaced; the manifest's hash
/// follows, and its row count too when `rows` is given.
fn with_document(
    listed: &[(String, Vec<u8>)],
    table: &str,
    bytes: &[u8],
    rows: Option<usize>,
) -> Vec<(String, Vec<u8>)> {
    let path = installation_export::table_path(table);
    let sha256 = canonical::content_hash_hex(bytes);
    let replaced: Vec<(String, Vec<u8>)> = listed
        .iter()
        .map(|(name, content)| {
            if *name == path {
                (name.clone(), bytes.to_vec())
            } else {
                (name.clone(), content.clone())
            }
        })
        .collect();
    with_manifest(&replaced, |manifest| {
        for stated in manifest["tables"].as_array_mut().expect("tables") {
            if stated["name"] == table {
                stated["sha256"] = Value::String(sha256.clone());
                if let Some(rows) = rows {
                    stated["rows"] = Value::from(rows);
                }
            }
        }
    })
}

/// The archive without one unit: its entries and its manifest listing.
fn without_unit(
    listed: &[(String, Vec<u8>)],
    record_id: i64,
    version_number: i64,
) -> Vec<(String, Vec<u8>)> {
    let prefix = format!("records/{record_id}/v{version_number}/");
    let kept: Vec<(String, Vec<u8>)> = listed
        .iter()
        .filter(|(name, _)| !name.starts_with(&prefix))
        .cloned()
        .collect();
    with_manifest(&kept, |manifest| {
        manifest["units"]
            .as_array_mut()
            .expect("units")
            .retain(|unit| {
                unit["record_id"] != record_id || unit["version_number"] != version_number
            });
    })
}

/// Verifies the edited archive and asserts it names `expected`.
fn assert_finds(edited: &[(String, Vec<u8>)], expected: &Finding, what: &str) {
    let report = export_verify::verify_archive(&repack(edited));
    let findings = all_findings(&report);
    assert!(!report.verified(), "{what}: verified");
    assert!(
        findings.contains(expected),
        "{what}: expected {expected:?} among {findings:#?}"
    );
}

fn first_row<'a>(rows: &'a mut [Value], what: &str) -> &'a mut serde_json::Map<String, Value> {
    rows.first_mut()
        .unwrap_or_else(|| panic!("{what} has a row"))
        .as_object_mut()
        .expect("object")
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn every_check_names_its_finding() {
    let p = populate().await;
    let fx = &p.fx;
    let bytes = export(fx, fx.admin_id).await;
    let report = export_verify::verify_archive(&bytes);
    // The baseline the controls depart from verifies: every finding below
    // is caused by the one change made.
    assert!(report.verified(), "{:#?}", all_findings(&report));
    let listed = entries(&bytes);
    let path = |table: &str| installation_export::table_path(table);

    // A document byte changed, the manifest's hash left alone.
    let altered: Vec<(String, Vec<u8>)> = listed
        .iter()
        .map(|(name, content)| {
            if *name == path("agency") {
                let text = String::from_utf8(content.clone()).expect("utf-8");
                (
                    name.clone(),
                    text.replace("Example County", "Example Countx")
                        .into_bytes(),
                )
            } else {
                (name.clone(), content.clone())
            }
        })
        .collect();
    assert_finds(
        &altered,
        &Finding::DocumentHashMismatch {
            path: path("agency"),
        },
        "document byte",
    );

    // A unit byte changed.
    let record = format!("records/{}/v1/record.json", p.amended_record_id);
    let altered: Vec<(String, Vec<u8>)> = listed
        .iter()
        .map(|(name, content)| {
            if *name == record {
                let text = String::from_utf8(content.clone()).expect("utf-8");
                (
                    name.clone(),
                    text.replace("Invented callback", "Invented callbacc")
                        .into_bytes(),
                )
            } else {
                (name.clone(), content.clone())
            }
        })
        .collect();
    assert_finds(&altered, &Finding::ContentHashMismatch, "unit byte");

    // Not canonical JSON, hash brought along.
    let pretty = serde_json::to_vec_pretty(&rows_of(&listed, "agency")).expect("pretty");
    assert_finds(
        &with_document(&listed, "agency", &pretty, None),
        &Finding::DocumentInvalid {
            path: path("agency"),
            detail: "not canonical JSON".to_owned(),
        },
        "non-canonical document",
    );

    // An extra member, and a missing one.
    let edited = with_rows(&listed, "user", |rows| {
        first_row(rows, "user").insert("nickname".to_owned(), Value::from("Ave"));
    });
    assert_finds(
        &edited,
        &installation(InstallationFinding::RowMembers {
            path: path("user"),
            row: 0,
            detail: "unexpected member nickname".to_owned(),
        }),
        "extra member",
    );
    let edited = with_rows(&listed, "user", |rows| {
        first_row(rows, "user").remove("title");
    });
    assert_finds(
        &edited,
        &installation(InstallationFinding::RowMembers {
            path: path("user"),
            row: 0,
            detail: "missing member title".to_owned(),
        }),
        "missing member",
    );

    // Wrong types: an integer as a string, a flag as an integer, a null
    // where none is carried, a hash in the wrong form.
    let edited = with_rows(&listed, "user", |rows| {
        first_row(rows, "user").insert("created_at".to_owned(), Value::from("yesterday"));
    });
    assert_finds(
        &edited,
        &installation(InstallationFinding::RowValue {
            path: path("user"),
            row: 0,
            column: "created_at".to_owned(),
            detail: "not an integer".to_owned(),
        }),
        "integer type",
    );
    let edited = with_rows(&listed, "form_narrative", |rows| {
        first_row(rows, "form_narrative").insert("required".to_owned(), Value::from(0));
    });
    assert_finds(
        &edited,
        &installation(InstallationFinding::RowValue {
            path: path("form_narrative"),
            row: 0,
            column: "required".to_owned(),
            detail: "not true or false".to_owned(),
        }),
        "flag type",
    );
    let edited = with_rows(&listed, "user", |rows| {
        first_row(rows, "user").insert("display_name".to_owned(), Value::Null);
    });
    assert_finds(
        &edited,
        &installation(InstallationFinding::RowValue {
            path: path("user"),
            row: 0,
            column: "display_name".to_owned(),
            detail: "null where the format carries no null".to_owned(),
        }),
        "nullability",
    );
    let edited = with_rows(&listed, "evaluation_version", |rows| {
        first_row(rows, "evaluation_version").insert("chain_hash".to_owned(), Value::from("ABC"));
    });
    assert_finds(
        &edited,
        &installation(InstallationFinding::RowValue {
            path: path("evaluation_version"),
            row: 0,
            column: "chain_hash".to_owned(),
            detail: "not 64 lowercase hex".to_owned(),
        }),
        "text form",
    );

    // Unknown vocabulary values, a CHECK list's and a module's.
    for (table, column) in [
        ("review_decision", "decision"),
        ("notice", "kind"),
        ("capability_grant", "capability"),
    ] {
        let edited = with_rows(&listed, table, |rows| {
            first_row(rows, table).insert(column.to_owned(), Value::from("invented_unknown"));
        });
        assert_finds(
            &edited,
            &installation(InstallationFinding::RowVocabulary {
                path: path(table),
                row: 0,
                column: column.to_owned(),
                value: "invented_unknown".to_owned(),
            }),
            "vocabulary",
        );
    }

    // Rows out of order.
    let edited = with_rows(&listed, "user", |rows| rows.reverse());
    let report = export_verify::verify_archive(&repack(&edited));
    assert!(
        all_findings(&report)
            .iter()
            .any(|finding| matches!(finding, Finding::DocumentOutOfOrder { path: p, .. } if *p == path("user"))),
        "order: {:#?}",
        all_findings(&report)
    );

    // A count mismatch: the ledger states one row more than is carried.
    let carried = rows_of(&listed, "user").len();
    let edited = with_manifest(&listed, |manifest| {
        for stated in manifest["tables"].as_array_mut().expect("tables") {
            if stated["name"] == "user" {
                stated["rows"] = Value::from(carried + 1);
            }
        }
    });
    assert_finds(
        &edited,
        &installation(InstallationFinding::RowCount {
            path: path("user"),
            stated: i64::try_from(carried + 1).expect("count"),
            carried,
        }),
        "count",
    );

    // A dangling reference.
    let edited = with_rows(&listed, "enrollment", |rows| {
        first_row(rows, "enrollment").insert("user_id".to_owned(), Value::from(9999));
    });
    assert_finds(
        &edited,
        &installation(InstallationFinding::DanglingReference {
            path: path("enrollment"),
            row: 0,
            column: "user_id".to_owned(),
            target: "user",
            id: 9999,
        }),
        "reference",
    );

    // An agreement violation: the task moved to the spare version, which
    // exists, while its competency stays in the published one.
    let edited = with_rows(&listed, "task", |rows| {
        first_row(rows, "task").insert(
            "program_version_id".to_owned(),
            Value::from(p.spare_version_id),
        );
    });
    assert_finds(
        &edited,
        &installation(InstallationFinding::AgreementViolated {
            path: path("task"),
            row: 0,
            column: "competency_id".to_owned(),
        }),
        "agreement",
    );

    // An audit subject with a kind and no id.
    let edited = with_rows(&listed, "audit_event", |rows| {
        let row = rows
            .iter_mut()
            .find(|row| !row["subject_kind"].is_null())
            .expect("an audit row with a subject");
        row["subject_id"] = Value::Null;
    });
    let report = export_verify::verify_archive(&repack(&edited));
    assert!(
        all_findings(&report).iter().any(|finding| matches!(
            finding,
            Finding::Installation {
                finding: InstallationFinding::SubjectShape { .. }
            }
        )),
        "subject shape: {:#?}",
        all_findings(&report)
    );

    // An evaluation_version row without its unit, and a unit without its
    // row.
    assert_finds(
        &without_unit(&listed, p.summary_record_id, 1),
        &installation(InstallationFinding::VersionWithoutUnit {
            record_id: p.summary_record_id,
            version_number: 1,
        }),
        "row without unit",
    );
    let summary = p.summary_record_id;
    let edited = with_rows(&listed, "evaluation_version", |rows| {
        rows.retain(|row| row["evaluation_record_id"] != summary);
    });
    assert_finds(
        &edited,
        &installation(InstallationFinding::UnitVersionRows {
            path: format!("records/{summary}/v1"),
            rows: 0,
        }),
        "unit without row",
    );
    let edited = with_rows(&listed, "evaluation_version", |rows| {
        let row = first_row(rows, "evaluation_version");
        let content = row["content_hash"].clone();
        row.insert("chain_hash".to_owned(), content);
    });
    let first_version: (i64, i64) = {
        let rows = rows_of(&listed, "evaluation_version");
        (
            rows[0]["evaluation_record_id"].as_i64().expect("record"),
            rows[0]["version_number"].as_i64().expect("number"),
        )
    };
    assert_finds(
        &edited,
        &installation(InstallationFinding::VersionDisagreesWithUnit {
            record_id: first_version.0,
            version_number: first_version.1,
            member: "chain_hash",
        }),
        "row and unit disagree",
    );

    // Every unit's predecessor is carried.
    assert_finds(
        &without_unit(&listed, p.amended_record_id, 1),
        &Finding::PredecessorNotCarried,
        "predecessor",
    );

    // The ledger: an unknown name, a missing one, a duplicated one, an
    // exclusion dropped, and a reason that is not the format's.
    let edited = with_manifest(&listed, |manifest| {
        let tables = manifest["tables"].as_array_mut().expect("tables");
        let at = tables
            .iter()
            .position(|t| t["name"].as_str() > Some("invented_table"))
            .expect("position");
        tables.insert(
            at,
            serde_json::json!({"name": "invented_table", "path": "tables/invented_table.json", "rows": 0, "sha256": "0".repeat(64)}),
        );
    });
    assert_finds(
        &edited,
        &installation(InstallationFinding::LedgerUnknown {
            list: "tables",
            name: "invented_table".to_owned(),
        }),
        "ledger unknown",
    );
    let edited = with_manifest(&listed, |manifest| {
        manifest["tables"]
            .as_array_mut()
            .expect("tables")
            .retain(|t| t["name"] != "notice");
    });
    assert_finds(
        &edited,
        &installation(InstallationFinding::LedgerMissing {
            name: "notice".to_owned(),
        }),
        "ledger missing",
    );
    let edited = with_manifest(&listed, |manifest| {
        let tables = manifest["tables"].as_array_mut().expect("tables");
        let agency = tables[1].clone();
        assert_eq!(agency["name"], "agency");
        tables.insert(1, agency);
    });
    assert_finds(
        &edited,
        &installation(InstallationFinding::LedgerDuplicated {
            name: "agency".to_owned(),
        }),
        "ledger duplicated",
    );
    let edited = with_manifest(&listed, |manifest| {
        manifest["excluded"]
            .as_array_mut()
            .expect("excluded")
            .retain(|t| t["name"] != "user.password_hash");
    });
    assert_finds(
        &edited,
        &installation(InstallationFinding::LedgerMissing {
            name: "user.password_hash".to_owned(),
        }),
        "exclusion missing",
    );
    let edited = with_manifest(&listed, |manifest| {
        manifest["excluded"][0]["reason"] = Value::from("invented");
    });
    assert_finds(
        &edited,
        &installation(InstallationFinding::LedgerReason {
            name: "password_reset_code".to_owned(),
            reason: "invented".to_owned(),
        }),
        "exclusion reason",
    );

    // The instance names another installation.
    let edited = with_rows(&listed, "instance", |rows| {
        first_row(rows, "instance").insert(
            "installation_id".to_owned(),
            Value::from("00000000-0000-4000-8000-000000000000"),
        );
    });
    assert_finds(
        &edited,
        &installation(InstallationFinding::InstanceDisagrees {
            detail: "its installation_id is not the manifest's".to_owned(),
        }),
        "instance",
    );

    // An entry the manifest does not name.
    let mut edited = listed.clone();
    edited.push(("tables/invented_table.json".to_owned(), b"[]".to_vec()));
    assert_finds(
        &edited,
        &Finding::UnlistedEntry {
            path: "tables/invented_table.json".to_owned(),
        },
        "unlisted entry",
    );

    // A format version this build does not know is refused by name.
    let edited = with_manifest(&listed, |manifest| {
        manifest["format_version"] = Value::from(2);
    });
    assert_finds(
        &edited,
        &Finding::UnsupportedFormat {
            format: INSTALLATION_FORMAT.to_owned(),
            format_version: 2,
        },
        "format version",
    );
    let _ = (p.taylor_id, p.jordan_id, p.rowan_id, p.casey_id);
}
