//! `cargo xtask drill collect`: what an installation holds, read back over
//! its HTTP API as its administrator into `manifest.json`. That is every
//! finalized record's version metadata (content and chain hashes) and
//! acknowledgment, and what each record's export and the installation's
//! export declare. The exports are saved beside it. Collect before and
//! after a restore, then [`compare`](super::compare) the manifests.

use std::io::{Cursor, Read as _};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use ureq::http::Method;

use super::account::Account;
use super::client::Client;
use super::{DrillError, described, field, shown};
use crate::proof::host::hex;

/// Record ids are small whole numbers: the walk ends after this many in a
/// row aren't finalized records.
const MISSES: u32 = 5;

/// What [`collect`] wrote.
#[derive(Debug)]
pub struct Collected {
    pub records: usize,
    pub manifest: PathBuf,
    /// The SHA-256 of `manifest.json`, in hex.
    pub sha256: String,
}

/// Signs in to `base` as `account` and writes `out/manifest.json`, with
/// `record-N.zip` and `installation.zip` beside it. Stops at the first call
/// the server refuses, other than a record id that isn't a finalized record,
/// and at the first export that doesn't hold what the API says it should.
pub fn collect(base: &str, account: &Account, out: &Path) -> Result<Collected, DrillError> {
    std::fs::create_dir_all(out).map_err(|e| DrillError(format!("{}: {e}", out.display())))?;
    let mut admin = Client::sign_in(base, account)?;
    let instance = admin.call(&Method::GET, "/api/instance", None)?;
    let mut records = Vec::new();
    let mut record_id = 0;
    let mut misses = 0;
    while misses < MISSES {
        record_id += 1;
        let Some(version) = admin.get_or_miss(&format!("/api/drafts/{record_id}/version"))? else {
            misses += 1;
            continue;
        };
        misses = 0;
        // No acknowledgment reads as {"acknowledgment": null}.
        let acknowledgment = admin.call(
            &Method::GET,
            &format!("/api/drafts/{record_id}/acknowledgment"),
            None,
        )?;
        let archive = admin.bytes(
            &Method::GET,
            &format!("/api/drafts/{record_id}/export"),
            None,
        )?;
        let path = out.join(format!("record-{record_id}.zip"));
        write(&path, &archive)?;
        let export = declared(&path, &archive)?;
        records.push(record_entry(record_id, &version, &acknowledgment, &export)?);
    }
    let everything = admin.bytes(&Method::GET, "/api/exports/records", None)?;
    let path = out.join("installation.zip");
    write(&path, &everything)?;
    let installation = declared(&path, &everything)?;
    let count = records.len();
    let text = pretty(&manifest(&instance, &records, &installation)?)?;
    let path = out.join("manifest.json");
    write(&path, text.as_bytes())?;
    Ok(Collected {
        records: count,
        manifest: path,
        sha256: hex(&Sha256::digest(text.as_bytes())),
    })
}

/// What an export says it holds: its installation, its scope, and every
/// unit's record, version and hashes. Compared before and after, it shows
/// each export came back for the same records, not only that it verifies.
pub fn declared(path: &Path, archive: &[u8]) -> Result<Value, DrillError> {
    let failed = |e: &dyn std::fmt::Display| DrillError(format!("{}: {e}", path.display()));
    let mut zip = zip::ZipArchive::new(Cursor::new(archive)).map_err(|e| failed(&e))?;
    let mut text = Vec::new();
    zip.by_name("manifest.json")
        .map_err(|e| failed(&e))?
        .read_to_end(&mut text)
        .map_err(|e| failed(&e))?;
    let manifest: Value = serde_json::from_slice(&text).map_err(|e| failed(&e))?;
    let mut held = Map::new();
    for key in ["installation_id", "scope", "units"] {
        let value = manifest
            .get(key)
            .ok_or_else(|| failed(&format!("its manifest.json has no `{key}`")))?;
        held.insert(key.to_owned(), value.clone());
    }
    Ok(Value::Object(held))
}

/// The manifest's entry for a record: the version metadata the API reports
/// for it (`version`), its acknowledgment, and what its export declares.
/// It's made only once the export is shown to be the record's, with the
/// API's version as its newest.
pub fn record_entry(
    record_id: i64,
    version: &Value,
    acknowledgment: &Value,
    export: &Value,
) -> Result<Value, DrillError> {
    let meta = field(version, "meta")?;
    let scope = field(export, "scope")?;
    if *scope != json!({ "kind": "record", "record_id": record_id }) {
        return Err(DrillError(format!(
            "record {record_id}'s export declares {scope}"
        )));
    }
    // The first of the highest version numbers, as a max would take it.
    let mut newest: Option<(&Value, i64)> = None;
    for unit in units(export)? {
        let (_, number) = unit_key(unit)?;
        if newest.is_none_or(|(_, best)| number > best) {
            newest = Some((unit, number));
        }
    }
    let Some((newest, _)) = newest else {
        return Err(DrillError(format!(
            "record {record_id}'s export holds no versions"
        )));
    };
    for key in ["version_number", "content_hash", "chain_hash"] {
        let (theirs, ours) = (field(newest, key)?, field(meta, key)?);
        if theirs != ours {
            return Err(DrillError(format!(
                "record {record_id}'s export has {key} {}, the record {}",
                shown(theirs),
                shown(ours)
            )));
        }
    }
    Ok(json!({
        "record_id": record_id,
        "meta": meta,
        "acknowledgment": acknowledgment,
        "export": export,
    }))
}

/// The manifest: the installation as `/api/instance` describes it, less its
/// build (`commit` and `version`, which the restoring server may not share),
/// every record's [entry](record_entry), and what the installation's export
/// declares. It's made only once that export is shown to be the
/// installation's, holding exactly the versions the records' own exports
/// hold.
pub fn manifest(
    instance: &Value,
    records: &[Value],
    installation: &Value,
) -> Result<Value, DrillError> {
    let mut instance = instance.clone();
    let Some(fields) = instance.as_object_mut() else {
        let got = described(&instance);
        return Err(DrillError(format!(
            "GET /api/instance: expected an object, not {got}"
        )));
    };
    fields.remove("commit");
    fields.remove("version");
    let scope = field(installation, "scope")?;
    if *scope != json!({ "kind": "installation" }) {
        return Err(DrillError(format!(
            "the installation export declares {scope}"
        )));
    }
    let held = ordered(units(installation)?)?;
    let mut every: Vec<&Value> = Vec::new();
    for record in records {
        every.extend(units(field(record, "export")?)?);
    }
    if held != ordered(every)? {
        return Err(DrillError(
            "the installation export does not hold exactly every record's versions".to_owned(),
        ));
    }
    Ok(json!({
        "instance": instance,
        "records": records,
        "installation_export": installation,
    }))
}

fn units(export: &Value) -> Result<&Vec<Value>, DrillError> {
    field(export, "units")?.as_array().ok_or_else(|| {
        let got = described(&export["units"]);
        DrillError(format!("an export's `units` is {got}, not a list"))
    })
}

/// A unit's record and version. Every unit has both, as whole numbers, or
/// the export is wrong: one without them can't be ordered or compared.
fn unit_key(unit: &Value) -> Result<(i64, i64), DrillError> {
    let whole = |key: &str| {
        let value = field(unit, key)?;
        value.as_i64().ok_or_else(|| {
            DrillError(format!(
                "an export's unit has a `{key}` that isn't a whole number but {}",
                described(value)
            ))
        })
    };
    Ok((whole("record_id")?, whole("version_number")?))
}

/// `units` with their keys, in key order (and in their own order where
/// keys tie, as a stable sort leaves them).
fn ordered<'a>(units: impl IntoIterator<Item = &'a Value>) -> Result<Vec<&'a Value>, DrillError> {
    let mut keyed = units
        .into_iter()
        .map(|unit| Ok((unit_key(unit)?, unit)))
        .collect::<Result<Vec<_>, DrillError>>()?;
    keyed.sort_by_key(|(key, _)| *key);
    Ok(keyed.into_iter().map(|(_, unit)| unit).collect())
}

/// `value` as `manifest.json` holds it: keys sorted (`serde_json`'s map is
/// ordered by key), two-space indents, and a newline at the end.
pub fn pretty(value: &Value) -> Result<String, DrillError> {
    let mut text = serde_json::to_string_pretty(value).map_err(|e| DrillError(e.to_string()))?;
    text.push('\n');
    Ok(text)
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), DrillError> {
    std::fs::write(path, bytes).map_err(|e| DrillError(format!("{}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::path::Path;

    use serde_json::{Value, json};

    use super::{declared, manifest, pretty, record_entry};

    fn unit(record: i64, version: i64) -> Value {
        json!({
            "record_id": record,
            "version_number": version,
            "content_hash": format!("c{record}.{version}"),
            "chain_hash": format!("h{record}.{version}"),
        })
    }

    /// `GET /api/drafts/N/version`'s answer.
    fn version(record: i64, version: i64) -> Value {
        let mut meta = unit(record, version);
        meta["finalized_at"] = json!("2026-06-02T22:00:00Z");
        json!({ "meta": meta, "record": { "id": record } })
    }

    fn export(record: i64, units: &[Value]) -> Value {
        json!({
            "installation_id": "invented",
            "scope": { "kind": "record", "record_id": record },
            "units": units,
        })
    }

    fn ack() -> Value {
        json!({ "acknowledgment": null })
    }

    fn check(record: i64, version: &Value, export: &Value) -> Result<(), String> {
        record_entry(record, version, &ack(), export)
            .map(|_| ())
            .map_err(|e| e.0)
    }

    #[test]
    fn a_records_entry_holds_its_meta_acknowledgment_and_export() {
        let good = export(2, &[unit(2, 1), unit(2, 2)]);
        let entry = record_entry(2, &version(2, 2), &ack(), &good).expect("entry");
        assert_eq!(
            entry,
            json!({
                "record_id": 2,
                "meta": version(2, 2)["meta"],
                "acknowledgment": ack(),
                "export": good,
            })
        );
        assert!(record_entry(2, &json!({}), &ack(), &good).is_err());
    }

    #[test]
    fn a_records_export_is_for_it_and_ends_at_its_version() {
        let good = export(2, &[unit(2, 1), unit(2, 2)]);
        assert_eq!(check(2, &version(2, 2), &good), Ok(()));
        // Out of order, the highest version is still the one compared.
        let shuffled = export(2, &[unit(2, 2), unit(2, 1)]);
        assert_eq!(check(2, &version(2, 2), &shuffled), Ok(()));

        let other = export(3, &[unit(2, 1), unit(2, 2)]);
        let why = check(2, &version(2, 2), &other).unwrap_err();
        assert!(why.starts_with("record 2's export declares"), "{why}");
        let mut installation = good.clone();
        installation["scope"] = json!({ "kind": "installation" });
        assert!(check(2, &version(2, 2), &installation).is_err());

        let behind = export(2, &[unit(2, 1)]);
        let why = check(2, &version(2, 2), &behind).unwrap_err();
        assert_eq!(why, "record 2's export has version_number 1, the record 2");
        for key in ["content_hash", "chain_hash"] {
            let mut changed = version(2, 2);
            changed["meta"][key] = json!("changed");
            let why = check(2, &changed, &good).unwrap_err();
            assert!(
                why.starts_with(&format!("record 2's export has {key} ")),
                "{why}"
            );
            assert!(why.ends_with(", the record changed"), "{why}");
        }
        let empty = export(2, &[]);
        let why = check(2, &version(2, 2), &empty).unwrap_err();
        assert_eq!(why, "record 2's export holds no versions");
    }

    #[test]
    fn every_unit_has_its_record_and_version() {
        // Missing or not a whole number, on the newest unit, an older one,
        // or the only one: the export is wrong.
        for key in ["record_id", "version_number"] {
            for bad in [Value::Null, json!("2"), json!(2.5)] {
                let mut broken = unit(2, 1);
                if bad.is_null() {
                    broken.as_object_mut().unwrap().remove(key);
                } else {
                    broken[key] = bad.clone();
                }
                for units in [
                    vec![broken.clone(), unit(2, 2)],
                    vec![unit(2, 2), broken.clone()],
                    vec![broken.clone()],
                ] {
                    let why = check(2, &version(2, 2), &export(2, &units)).unwrap_err();
                    assert!(why.contains(&format!("`{key}`")), "{key} {bad}: {why}");
                }
                // The same broken unit on both sides of the installation's
                // comparison is still broken.
                let mut records = records();
                records[0]["export"]["units"] = json!([broken.clone()]);
                let why = manifest(&instance(), &records, &installation(&[broken.clone()]))
                    .unwrap_err()
                    .0;
                assert!(why.contains(&format!("`{key}`")), "{key} {bad}: {why}");
            }
        }
    }

    fn installation(units: &[Value]) -> Value {
        json!({ "installation_id": "invented", "scope": { "kind": "installation" }, "units": units })
    }

    fn records() -> Vec<Value> {
        vec![
            record_entry(1, &version(1, 1), &ack(), &export(1, &[unit(1, 1)])).expect("entry"),
            record_entry(
                2,
                &version(2, 2),
                &ack(),
                &export(2, &[unit(2, 1), unit(2, 2)]),
            )
            .expect("entry"),
        ]
    }

    fn instance() -> Value {
        json!({ "agency": "Example County Communications", "commit": "abc", "version": "0.3.0" })
    }

    #[test]
    fn the_manifest_holds_the_installation_less_its_build() {
        let all = installation(&[unit(2, 2), unit(1, 1), unit(2, 1)]);
        assert_eq!(
            manifest(&instance(), &records(), &all),
            Ok(json!({
                "instance": { "agency": "Example County Communications" },
                "records": records(),
                "installation_export": all,
            }))
        );
        assert!(manifest(&json!([]), &records(), &all).is_err());
    }

    #[test]
    fn the_installations_export_holds_exactly_every_records_versions() {
        let held = |units: &[Value]| manifest(&instance(), &records(), &installation(units));
        let short = held(&[unit(1, 1), unit(2, 1)]);
        assert_eq!(
            short.unwrap_err().0,
            "the installation export does not hold exactly every record's versions"
        );
        assert!(held(&[unit(1, 1), unit(2, 1), unit(2, 2), unit(3, 1)]).is_err());
        let mut changed = unit(2, 2);
        changed["chain_hash"] = json!("changed");
        assert!(held(&[unit(1, 1), unit(2, 1), changed]).is_err());
        let mut scoped = installation(&[unit(1, 1), unit(2, 1), unit(2, 2)]);
        scoped["scope"] = json!({ "kind": "record", "record_id": 1 });
        let why = manifest(&instance(), &records(), &scoped).unwrap_err().0;
        assert!(why.starts_with("the installation export declares"), "{why}");
    }

    fn archive(manifest: &Value) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("records/1.json", options).expect("entry");
        zip.write_all(b"{}").expect("write");
        zip.start_file("manifest.json", options).expect("entry");
        zip.write_all(manifest.to_string().as_bytes())
            .expect("write");
        zip.finish().expect("finish").into_inner()
    }

    #[test]
    fn an_export_declares_its_installation_scope_and_units() {
        let mut manifest = export(1, &[unit(1, 1)]);
        manifest["format"] = json!("consolebook-record-export/1");
        let path = Path::new("record-1.zip");
        assert_eq!(
            declared(path, &archive(&manifest)),
            Ok(export(1, &[unit(1, 1)]))
        );
        manifest.as_object_mut().expect("object").remove("units");
        let why = declared(path, &archive(&manifest)).unwrap_err().0;
        assert_eq!(why, "record-1.zip: its manifest.json has no `units`");
        assert!(declared(path, b"not a zip").is_err());
    }

    #[test]
    fn a_manifest_is_written_with_its_keys_in_order() {
        let value = json!({ "b": [{ "z": 1, "a": null }], "a": {} });
        assert_eq!(
            pretty(&value),
            Ok("{\n  \"a\": {},\n  \"b\": [\n    {\n      \"a\": null,\n      \"z\": 1\n    }\n  ]\n}\n".to_owned())
        );
    }
}
