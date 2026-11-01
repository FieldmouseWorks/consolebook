//! `cargo xtask drill compare`: whether the manifest collected after the
//! restore holds what the one before it held.

use serde_json::Value;

use super::{DrillError, described, field};

/// The verdict, and the lines that show it.
#[derive(Debug, PartialEq, Eq)]
pub struct Comparison {
    pub same: bool,
    pub report: Vec<String>,
}

/// Compares `before` and `after`, whole. The one allowance is for fields of
/// `/api/instance` that only `after` has: a newer version restoring an
/// older one's backup reports more about itself, and those fields are set
/// aside (and named) rather than counted as a difference.
pub fn compare(before: &Value, after: &Value) -> Result<Comparison, DrillError> {
    let mut after = after.clone();
    let ours = instance(before)?;
    let Some(theirs) = after.get_mut("instance").and_then(Value::as_object_mut) else {
        return Err(DrillError(
            "the second manifest's instance isn't an object".to_owned(),
        ));
    };
    let mut added: Vec<String> = theirs
        .keys()
        .filter(|key| !ours.contains_key(*key))
        .cloned()
        .collect();
    added.sort();
    for key in &added {
        theirs.remove(key);
    }
    let same = *before == after;
    let (records_before, records_after) = (records(before)?, records(&after)?);
    let mut report = vec![format!(
        "instance: {} | fields only the restoring version reports: {added:?} | records: {} vs {}",
        field(before, "instance")? == field(&after, "instance")?,
        records_before.len(),
        records_after.len()
    )];
    for (a, b) in records_before.iter().zip(records_after) {
        let meta = a.get("meta");
        let hash = |key: &str| -> String {
            meta.and_then(|m| m.get(key))
                .and_then(Value::as_str)
                .unwrap_or("?")
                .chars()
                .take(16)
                .collect()
        };
        report.push(format!(
            "  record {}: content {} chain {} meta equal={} ack equal={}",
            a.get("record_id")
                .map_or_else(|| "?".to_owned(), Value::to_string),
            hash("content_hash"),
            hash("chain_hash"),
            a.get("meta") == b.get("meta"),
            a.get("acknowledgment") == b.get("acknowledgment"),
        ));
    }
    report.push(
        if same {
            "MANIFESTS EQUAL"
        } else {
            "MANIFESTS DIFFER"
        }
        .to_owned(),
    );
    Ok(Comparison { same, report })
}

fn instance(manifest: &Value) -> Result<&serde_json::Map<String, Value>, DrillError> {
    field(manifest, "instance")?.as_object().ok_or_else(|| {
        let got = described(&manifest["instance"]);
        DrillError(format!("a manifest's `instance` is {got}, not an object"))
    })
}

fn records(manifest: &Value) -> Result<&Vec<Value>, DrillError> {
    field(manifest, "records")?.as_array().ok_or_else(|| {
        let got = described(&manifest["records"]);
        DrillError(format!("a manifest's `records` is {got}, not a list"))
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::compare;

    fn manifest() -> Value {
        json!({
            "instance": { "agency": "Example County Communications", "initialized": true },
            "records": [
                {
                    "record_id": 1,
                    "meta": { "content_hash": "1011bb41d385f76a0000", "chain_hash": "e07bb0303452b5550000" },
                    "acknowledgment": { "acknowledgment": null },
                    "export": { "units": [] },
                },
            ],
            "installation_export": { "units": [] },
        })
    }

    #[test]
    fn equal_manifests_compare_equal() {
        let seen = compare(&manifest(), &manifest()).expect("compare");
        assert!(seen.same);
        assert_eq!(
            seen.report,
            [
                "instance: true | fields only the restoring version reports: [] | records: 1 vs 1",
                "  record 1: content 1011bb41d385f76a chain e07bb0303452b555 meta equal=true ack equal=true",
                "MANIFESTS EQUAL",
            ]
        );
    }

    #[test]
    fn fields_only_the_restoring_version_reports_are_set_aside() {
        let mut after = manifest();
        after["instance"]["public_url"] = Value::Null;
        after["instance"]["build"] = json!("x");
        let seen = compare(&manifest(), &after).expect("compare");
        assert!(seen.same);
        assert!(
            seen.report[0].contains(r#"reports: ["build", "public_url"]"#),
            "{}",
            seen.report[0]
        );
        // A field the restored installation lost is a difference.
        let mut after = manifest();
        after["instance"]
            .as_object_mut()
            .expect("object")
            .remove("initialized");
        let seen = compare(&manifest(), &after).expect("compare");
        assert!(!seen.same);
        assert!(
            seen.report[0].starts_with("instance: false"),
            "{}",
            seen.report[0]
        );
    }

    #[test]
    fn any_other_difference_is_a_difference() {
        let changes: [fn(&mut Value); 5] = [
            |m| m["instance"]["agency"] = json!("Another County"),
            |m| m["records"][0]["meta"]["chain_hash"] = json!("changed"),
            |m| {
                m["records"][0]["acknowledgment"] =
                    json!({ "acknowledgment": { "kind": "acknowledged" } });
            },
            |m| m["records"][0]["export"]["units"] = json!([{ "version_number": 1 }]),
            |m| m["installation_export"]["units"] = json!([{ "version_number": 1 }]),
        ];
        for change in changes {
            let mut after = manifest();
            change(&mut after);
            let seen = compare(&manifest(), &after).expect("compare");
            assert!(!seen.same, "{after}");
            assert_eq!(
                seen.report.last().map(String::as_str),
                Some("MANIFESTS DIFFER")
            );
        }
        let mut after = manifest();
        after["records"][0]["meta"]["chain_hash"] = json!("changed");
        let seen = compare(&manifest(), &after).expect("compare");
        assert!(
            seen.report[1].ends_with("meta equal=false ack equal=true"),
            "{}",
            seen.report[1]
        );
        let mut fewer = manifest();
        fewer["records"] = json!([]);
        let seen = compare(&manifest(), &fewer).expect("compare");
        assert!(!seen.same);
        assert!(
            seen.report[0].ends_with("records: 1 vs 0"),
            "{}",
            seen.report[0]
        );
    }

    #[test]
    fn a_manifest_without_an_instance_or_records_is_refused() {
        assert!(compare(&json!({}), &manifest()).is_err());
        assert!(compare(&manifest(), &json!({ "records": [] })).is_err());
        assert!(compare(&json!({ "instance": {}, "records": {} }), &manifest()).is_err());
    }
}
