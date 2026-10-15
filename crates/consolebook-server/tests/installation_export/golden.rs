//! The golden `format_version` 1 archive: one installation export of the
//! fully populated invented fixture, committed as bytes. It pins the
//! manifest shape and the verifier's rules for version 1, so a change
//! that would need a version bump fails here even if nothing else
//! notices.

use super::*;

/// The committed version-1 archive.
const GOLDEN_V1: &[u8] = include_bytes!("../fixtures/installation-export-v1.zip");
const GOLDEN_V1_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/installation-export-v1.zip"
);

/// Regenerates the golden archive from the populated fixture. Legitimate
/// only together with a new `format_version`, and then as a new file for
/// that version beside this one: never to make the version-1 test pass.
/// Run with `cargo test -p consolebook-server --test installation_export
/// -- --ignored golden::regenerate`.
#[tokio::test]
#[ignore = "writes the committed golden archive; only with a new format_version"]
async fn regenerate() {
    let p = populate().await;
    let bytes = export(&p.fx, p.fx.admin_id).await;
    std::fs::write(GOLDEN_V1_PATH, &bytes).expect("write golden archive");
}

/// Rows per carried table in the golden archive.
const GOLDEN_ROWS: &[(&str, usize)] = &[
    ("acknowledgment", 1),
    ("agency", 1),
    ("amendment", 1),
    ("audit_event", 50),
    ("capability_grant", 14),
    ("competency", 2),
    ("contributor_event", 13),
    ("draft_narrative", 2),
    ("draft_rating", 2),
    ("draft_rating_modifier", 1),
    ("draft_snapshot", 3),
    ("enrollment", 1),
    ("enrollment_event", 1),
    ("evaluation_form", 4),
    ("evaluation_record", 3),
    ("evaluation_session", 2),
    ("evaluation_version", 3),
    ("finalization_policy", 2),
    ("form_competency", 2),
    ("form_narrative", 4),
    ("hold_release", 2),
    ("instance", 1),
    ("notice", 14),
    ("phase", 4),
    ("phase_event", 1),
    ("phase_transition", 2),
    ("program", 2),
    ("program_version", 2),
    ("rating_anchor", 2),
    ("rating_modifier", 2),
    ("rating_scale", 2),
    ("record_hold", 2),
    ("retention_authority_event", 1),
    ("retention_policy", 1),
    ("review_decision", 2),
    ("session_trainer", 3),
    ("standards_citation", 6),
    ("summary_daily_link", 1),
    ("task", 2),
    ("task_signoff", 1),
    ("training_assignment", 2),
    ("training_session", 2),
    ("user", 5),
];

/// Every build verifies the committed version-1 archive. A change that
/// makes this fail is a format change: record a new `format_version`,
/// and its ADR decides how version-1 archives keep verifying (keep
/// verifying them, or refuse them by name). Never regenerate the
/// version-1 file to make this test pass.
#[test]
fn the_golden_version_1_archive_verifies() {
    let report = export_verify::verify_archive(GOLDEN_V1);
    assert!(report.verified(), "{:#?}", all_findings(&report));
    assert_eq!(report.kind, Some(ArchiveKind::InstallationExport));
    let listed = entries(GOLDEN_V1);
    let manifest = manifest_of(&listed);
    assert_eq!(manifest.format, INSTALLATION_FORMAT);
    assert_eq!(manifest.format_version, 1);
    // Two versions of one record: unit/row agreement and predecessor
    // linkage are exercised, not only single versions.
    let units: Vec<(i64, i64)> = manifest
        .units
        .iter()
        .map(|unit| (unit.record_id, unit.version_number))
        .collect();
    assert_eq!(units, [(1, 1), (1, 2), (2, 1)]);
    assert_eq!(report.units.len(), 3);
    assert_eq!(manifest.excluded.len(), 5);
    let rows: Vec<(&str, usize)> = report
        .tables
        .iter()
        .map(|table| (table.table.as_str(), table.rows.expect("rows")))
        .collect();
    assert_eq!(rows, GOLDEN_ROWS);
    assert!(
        rows.iter().all(|(_, count)| *count > 0),
        "every table has a row"
    );
}

/// The golden test reads the bytes: one document byte changed fails it.
#[test]
fn a_changed_golden_byte_fails_verification() {
    let listed = entries(GOLDEN_V1);
    let path = installation_export::table_path("agency");
    let altered: Vec<(String, Vec<u8>)> = listed
        .iter()
        .map(|(name, content)| {
            let mut content = content.clone();
            if *name == path {
                let at = content.iter().position(|b| *b == b'E').expect("a letter");
                content[at] = b'F';
            }
            (name.clone(), content)
        })
        .collect();
    let report = export_verify::verify_archive(&repack(&altered));
    assert!(!report.verified());
    assert!(
        all_findings(&report).contains(&Finding::DocumentHashMismatch { path }),
        "{:#?}",
        all_findings(&report)
    );
}
