//! Golden record-export archives, committed as bytes (#215). The
//! version-1 archive was produced by the format-version-1 build, before
//! format version 2 existed, so its verification here proves the explicit
//! compatibility dispatch against that build's real output rather than an
//! archive rewritten through this build's own types. The version-2
//! archives pin the disposition-aware shapes: mixed retained and disposed
//! lineage with a policy boundary, a fully disposed record scope, and an
//! enrollment scope closed by a policy boundary alone. A change that makes
//! any of them read differently is a format change: define a new
//! `format_version`, whose ADR decides how these keep verifying. Never
//! regenerate a committed file to make a test pass.

use consolebook_server::disposition_evidence::verify::{
    BoundaryCheckpointVerdict, DisposedEntryVerdict,
};
use consolebook_server::export_verify::{self, ArchiveContentStatus, ArchiveKind, ArchiveReport};
use consolebook_server::record_export::{self, Scope};

use super::*;

/// Produced by the format-version-1 build: an installation scope of two
/// records, two versions each.
const GOLDEN_V1: &[u8] = include_bytes!("../fixtures/record-export-v1.zip");
/// An installation scope: one retained record of two versions, one record
/// disposed with its version hashes, one disposed without, and one policy
/// boundary checkpoint.
const GOLDEN_V2_MIXED: &[u8] = include_bytes!("../fixtures/record-export-v2-mixed.zip");
/// A record scope whose record was disposed with its version hashes.
const GOLDEN_V2_FULLY_DISPOSED: &[u8] =
    include_bytes!("../fixtures/record-export-v2-fully-disposed.zip");
/// An enrollment scope with no retained record and no tombstone, closed by
/// the installation's policy boundary alone.
const GOLDEN_V2_BOUNDARY_ONLY: &[u8] =
    include_bytes!("../fixtures/record-export-v2-boundary-only.zip");
/// Set to `1` to let [`regenerate_v2`] write the version-2 archives.
const REGENERATE_ENV: &str = "CONSOLEBOOK_REGENERATE_GOLDEN";

/// Writes the three version-2 golden archives from the invented fixture.
/// Legitimate only when they do not exist yet. Ignored by default, inert
/// without `CONSOLEBOOK_REGENERATE_GOLDEN=1`, and refuses to overwrite:
/// `CONSOLEBOOK_REGENERATE_GOLDEN=1 cargo test -p consolebook-server
/// --test record_export_disposition -- --ignored golden::regenerate_v2`.
#[tokio::test]
#[ignore = "writes new golden archives; only with a new format_version"]
async fn regenerate_v2() {
    if std::env::var(REGENERATE_ENV).as_deref() != Ok("1") {
        println!("skipped: set {REGENERATE_ENV}=1 to write golden archives");
        return;
    }
    let dir = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
    let paths = [
        format!("{dir}/record-export-v2-mixed.zip"),
        format!("{dir}/record-export-v2-fully-disposed.zip"),
        format!("{dir}/record-export-v2-boundary-only.zip"),
    ];
    for path in &paths {
        assert!(
            !std::path::Path::new(path).exists(),
            "{path} exists; a golden archive is never regenerated"
        );
    }

    let fx = DispExportFixture::new().await;
    let (_, meta_policy_id) = fx.set_destroy_policies().await;
    let (_, fin1) = fx
        .create_finalized_daily(
            fx.enrollment_id,
            "2026-06-01",
            "Invented retained daily report",
            Some("Invented retained amendment"),
        )
        .await;
    let (hashed, fin2) = fx
        .create_finalized_daily(
            fx.enrollment_id,
            "2026-06-02",
            "Invented disposed daily v1",
            Some("Invented disposed daily v2"),
        )
        .await;
    let (unhashed, fin3) = fx
        .create_finalized_daily(
            fx.enrollment_id,
            "2026-06-03",
            "Invented disposed without hashes",
            None,
        )
        .await;
    // Only the two disposed records are old enough under the schedule.
    let evaluated_at = fin2.max(fin3) + 31 * SECONDS_PER_DAY;
    assert!(fin1 > 0);
    fx.dispose_records(vec![hashed], evaluated_at, true).await;
    fx.dispose_records(vec![unhashed], evaluated_at + 10, false)
        .await;
    fx.insert_boundary_checkpoint(meta_policy_id, evaluated_at + 20)
        .await;

    for (path, scope) in paths.iter().zip([
        Scope::Installation,
        Scope::Record { record_id: hashed },
        Scope::Enrollment {
            enrollment_id: fx.other_enrollment_id,
        },
    ]) {
        let bytes = record_export::export_at(&fx.pool, fx.admin_id, scope, EXPORTED_AT)
            .await
            .expect("call")
            .expect("export")
            .bytes;
        std::fs::write(path, bytes).expect("write golden archive");
    }
}

fn verified(bytes: &[u8]) -> ArchiveReport {
    let report = export_verify::verify_archive(bytes);
    assert!(report.verified(), "{report:#?}");
    assert_eq!(report.kind, Some(ArchiveKind::RecordExport));
    report
}

fn unit_ids(report: &ArchiveReport) -> Vec<(i64, i64)> {
    report
        .units
        .iter()
        .map(|unit| (unit.record_id, unit.version_number))
        .collect()
}

/// The format-version-1 build's own archive still verifies, as retained
/// content with no disposition report.
#[test]
fn the_golden_version_1_archive_verifies() {
    let report = verified(GOLDEN_V1);
    assert_eq!(report.scope, Some(Scope::Installation));
    assert_eq!(unit_ids(&report), [(1, 1), (1, 2), (2, 1), (2, 2)]);
    assert!(report.disposition.is_none());
    assert_eq!(
        report.content_status(),
        ArchiveContentStatus::RetainedContentOnly
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(entry(&entries(GOLDEN_V1), "manifest.json")).expect("manifest");
    assert_eq!(manifest["format_version"], 1);
    assert!(manifest.get("disposition_evidence").is_none());
}

/// Retained units, both disposed verdicts, and the policy boundary are
/// reported apart; nothing disposed is called verified content.
#[test]
fn the_golden_mixed_archive_verifies() {
    let report = verified(GOLDEN_V2_MIXED);
    assert_eq!(report.scope, Some(Scope::Installation));
    assert_eq!(unit_ids(&report), [(1, 1), (1, 2)]);
    assert_eq!(
        report.content_status(),
        ArchiveContentStatus::MixedRetainedAndDisposed
    );
    let disposition = report.disposition.as_ref().expect("disposition report");
    let disposed: Vec<(i64, DisposedEntryVerdict)> = disposition
        .dispositions
        .iter()
        .map(|entry| (entry.evaluation_record_id, entry.verdict))
        .collect();
    assert_eq!(
        disposed,
        [
            (2, DisposedEntryVerdict::DisposedHashContinuityOnly),
            (3, DisposedEntryVerdict::DisposedContentAndHashesUnavailable),
        ]
    );
    assert_eq!(disposition.boundary_checkpoints.len(), 1);
    assert_eq!(
        disposition.boundary_checkpoints[0].verdict,
        BoundaryCheckpointVerdict::IntentionalPolicyBoundary
    );
}

/// A fully disposed record scope carries no unit and is never an empty or
/// complete-content export.
#[test]
fn the_golden_fully_disposed_archive_verifies() {
    let report = verified(GOLDEN_V2_FULLY_DISPOSED);
    assert_eq!(report.scope, Some(Scope::Record { record_id: 2 }));
    assert!(report.units.is_empty());
    assert_eq!(report.content_status(), ArchiveContentStatus::FullyDisposed);
    let disposition = report.disposition.as_ref().expect("disposition report");
    assert_eq!(disposition.dispositions.len(), 1);
    assert_eq!(disposition.dispositions[0].version_count, 2);
    assert!(disposition.boundary_checkpoints.is_empty());
}

/// An enrollment with nothing retained and no tombstone reports the
/// policy boundary alone, not disposed records.
#[test]
fn the_golden_boundary_only_archive_verifies() {
    let report = verified(GOLDEN_V2_BOUNDARY_ONLY);
    assert!(matches!(report.scope, Some(Scope::Enrollment { .. })));
    assert!(report.units.is_empty());
    assert_eq!(
        report.content_status(),
        ArchiveContentStatus::PolicyBoundaryOnly
    );
    let disposition = report.disposition.as_ref().expect("disposition report");
    assert!(disposition.dispositions.is_empty());
    assert_eq!(disposition.boundary_checkpoints.len(), 1);
}

/// Each golden archive fails once its evidence or a unit is tampered with,
/// so the goldens are not accepted by a verifier that checks nothing.
#[test]
fn tampered_goldens_do_not_verify() {
    for golden in [
        GOLDEN_V2_MIXED,
        GOLDEN_V2_FULLY_DISPOSED,
        GOLDEN_V2_BOUNDARY_ONLY,
    ] {
        let tampered = repack(&with_entry(
            &entries(golden),
            record_export::DISPOSITION_EVIDENCE_PATH,
            |bytes| {
                let mut copy = bytes.to_vec();
                copy.push(b' ');
                Some(copy)
            },
        ));
        assert!(!export_verify::verify_archive(&tampered).verified());
    }
    let tampered_v1 = repack(&with_entry(
        &entries(GOLDEN_V1),
        "records/1/v2/record.json",
        |bytes| {
            let mut copy = bytes.to_vec();
            copy.push(b' ');
            Some(copy)
        },
    ));
    assert!(!export_verify::verify_archive(&tampered_v1).verified());
}
