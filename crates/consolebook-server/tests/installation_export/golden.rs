//! The golden `format_version` 1 archive: one installation export of the
//! fully populated invented fixture, committed as bytes. It pins the
//! manifest shape and the verifier's rules for version 1, so a change
//! that would need a version bump fails here even if nothing else
//! notices.

use super::*;

/// The committed version-1 archive.
const GOLDEN_V1: &[u8] = include_bytes!("../fixtures/installation-export-v1.zip");
/// Set to `1` to let [`regenerate`] write a golden archive.
const REGENERATE_ENV: &str = "CONSOLEBOOK_REGENERATE_GOLDEN";

/// Writes the golden archive for the current `INSTALLATION_FORMAT_VERSION`,
/// `tests/fixtures/installation-export-v{N}.zip`, from the populated
/// fixture. Legitimate only when a new `format_version` is defined, to
/// create that version's archive beside the earlier ones; never to make an
/// existing version's test pass.
///
/// Three guards keep a habit from rewriting history: it is ignored by
/// default; even when ignored tests run (`--include-ignored`) it does
/// nothing unless `CONSOLEBOOK_REGENERATE_GOLDEN=1` is set; and it refuses
/// to overwrite a file that exists. Run with
/// `CONSOLEBOOK_REGENERATE_GOLDEN=1 cargo test -p consolebook-server
/// --test installation_export -- --ignored golden::regenerate`.
#[tokio::test]
#[ignore = "writes a new golden archive; only with a new format_version"]
async fn regenerate() {
    if std::env::var(REGENERATE_ENV).as_deref() != Ok("1") {
        println!(
            "skipped: set {REGENERATE_ENV}=1 to write a golden archive for a new format_version"
        );
        return;
    }
    let path = format!(
        "{}/tests/fixtures/installation-export-v{INSTALLATION_FORMAT_VERSION}.zip",
        env!("CARGO_MANIFEST_DIR")
    );
    assert!(
        !std::path::Path::new(&path).exists(),
        "{path} exists; a golden archive is never regenerated. Define a new format_version first."
    );
    let p = populate().await;
    let bytes = export(&p.fx, p.fx.admin_id).await;
    std::fs::write(&path, &bytes).expect("write golden archive");
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
