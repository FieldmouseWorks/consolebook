//! Golden, mixed-lineage, fully-disposed, policy-boundary, legacy-v1, and
//! negative-fixture verification tests for record exports (#215).

use axum::body::Body;
use axum::http::header::COOKIE;
use axum::http::{Request, StatusCode};
use consolebook_server::disposition_evidence::verify::{
    BoundaryCheckpointVerdict, DisposedEntryVerdict,
};
use consolebook_server::export_verify::{self, ArchiveContentStatus, Finding};
use consolebook_server::record_export::{
    self, ARCHIVE_FORMAT, ARCHIVE_MANIFEST_PATH, ArchiveManifest, ArchiveManifestV1,
    DISPOSITION_EVIDENCE_PATH, ExportRefusal, FORMAT_VERSION, LEGACY_FORMAT_VERSION, Scope,
    UNIT_FORMAT_VERSION, UnitManifest,
};
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn mixed_lineage_fully_disposed_scopes_and_authorization_boundaries() {
    let fx = DispExportFixture::new().await;
    fx.set_destroy_policies().await;

    let (r_retained, fin1) = fx
        .create_finalized_daily(
            fx.enrollment_id,
            "2026-06-01",
            "Invented retained daily report",
            None,
        )
        .await;
    let (r_hashed, fin2) = fx
        .create_finalized_daily(
            fx.enrollment_id,
            "2026-06-02",
            "Invented disposed daily v1",
            Some("Invented disposed daily v2"),
        )
        .await;
    let (r_nohash, fin3) = fx
        .create_finalized_daily(
            fx.enrollment_id,
            "2026-06-03",
            "Invented disposed without hashes",
            None,
        )
        .await;
    let (r_other, fin4) = fx
        .create_finalized_daily(
            fx.other_enrollment_id,
            "2026-06-04",
            "Invented other trainee disposed record",
            None,
        )
        .await;

    let evaluated_at = fin1.max(fin2).max(fin3).max(fin4) + 31 * SECONDS_PER_DAY;
    fx.dispose_records(vec![r_hashed], evaluated_at, true).await;
    fx.dispose_records(vec![r_nohash], evaluated_at + 5, false)
        .await;
    fx.dispose_records(vec![r_other], evaluated_at + 10, true)
        .await;

    // 1. Mixed Enrollment scope: carries retained unit + only this enrollment's
    //    two disposed records (never r_other from another enrollment).
    let enroll_export = record_export::export_at(
        &fx.pool,
        fx.executor_id,
        Scope::Enrollment {
            enrollment_id: fx.enrollment_id,
        },
        EXPORTED_AT,
    )
    .await
    .expect("call")
    .expect("enrollment export");
    assert_eq!(enroll_export.unit_count, 1);
    assert_eq!(enroll_export.disposed_record_count, 2);
    assert_eq!(enroll_export.boundary_checkpoint_count, 0);

    // Deterministic bytes at the same instant.
    let enroll_again = record_export::export_at(
        &fx.pool,
        fx.executor_id,
        Scope::Enrollment {
            enrollment_id: fx.enrollment_id,
        },
        EXPORTED_AT,
    )
    .await
    .expect("call")
    .expect("enrollment export again");
    assert_eq!(enroll_export.bytes, enroll_again.bytes);

    let report = export_verify::verify_archive(&enroll_export.bytes);
    assert!(report.verified(), "{report:?}");
    assert_eq!(
        report.content_status(),
        ArchiveContentStatus::MixedRetainedAndDisposed
    );
    assert_eq!(report.units.len(), 1);
    assert_eq!(report.units[0].record_id, r_retained);
    let disp = report.disposition.as_ref().expect("disposition report");
    assert_eq!(disp.dispositions.len(), 2);
    assert_eq!(disp.dispositions[0].evaluation_record_id, r_hashed);
    assert_eq!(
        disp.dispositions[0].verdict,
        DisposedEntryVerdict::DisposedHashContinuityOnly
    );
    assert_eq!(disp.dispositions[1].evaluation_record_id, r_nohash);
    assert_eq!(
        disp.dispositions[1].verdict,
        DisposedEntryVerdict::DisposedContentAndHashesUnavailable
    );

    // 2. Installation scope & summary report all 3 disposed records + 1 retained.
    let summary = record_export::summary(&fx.pool, fx.admin_id)
        .await
        .expect("call")
        .expect("summary");
    assert_eq!(summary.record_count, 1);
    assert_eq!(summary.version_count, 1);
    assert_eq!(summary.disposed_record_count, 3);
    assert_eq!(summary.boundary_checkpoint_count, 0);

    let inst_export =
        record_export::export_at(&fx.pool, fx.admin_id, Scope::Installation, EXPORTED_AT)
            .await
            .expect("call")
            .expect("installation export");
    let inst_report = export_verify::verify_archive(&inst_export.bytes);
    assert!(inst_report.verified(), "{inst_report:?}");
    assert_eq!(
        inst_report.content_status(),
        ArchiveContentStatus::MixedRetainedAndDisposed
    );
    assert_eq!(inst_report.units.len(), 1);
    assert_eq!(
        inst_report
            .disposition
            .as_ref()
            .expect("disp")
            .dispositions
            .len(),
        3
    );

    // 3. Fully disposed Record, Version, and Enrollment scopes verify honestly
    //    as FullyDisposed (never ordinary empty, never complete-content).
    for scope in [
        Scope::Record {
            record_id: r_hashed,
        },
        Scope::Version {
            record_id: r_hashed,
            version_number: 1,
        },
        Scope::Version {
            record_id: r_hashed,
            version_number: 2,
        },
        Scope::Enrollment {
            enrollment_id: fx.other_enrollment_id,
        },
    ] {
        let exp = record_export::export_at(&fx.pool, fx.executor_id, scope, EXPORTED_AT)
            .await
            .expect("call")
            .expect("fully disposed export");
        assert_eq!(exp.unit_count, 0);
        assert_eq!(exp.disposed_record_count, 1);
        let rep = export_verify::verify_archive(&exp.bytes);
        assert!(rep.verified(), "{scope}: {rep:?}");
        assert_eq!(rep.content_status(), ArchiveContentStatus::FullyDisposed);
        assert_eq!(rep.units.len(), 0);
    }

    // 4. Parent authorization boundaries hold strictly on fully disposed scopes:
    //    - Trainee reads own disposed record, NOT another trainee's.
    //    - Assigned trainer reads assigned enrollment's disposed record.
    //    - Outsider trainer is refused with CapabilityRequired.
    //    - Out-of-range version returns NoSuchVersion; nonexistent record returns NoSuchRecord.
    assert!(
        record_export::export_at(
            &fx.pool,
            fx.trainee_id,
            Scope::Record {
                record_id: r_hashed,
            },
            EXPORTED_AT,
        )
        .await
        .expect("call")
        .is_ok()
    );
    assert_eq!(
        record_export::export_at(
            &fx.pool,
            fx.trainee_id,
            Scope::Record { record_id: r_other },
            EXPORTED_AT,
        )
        .await
        .expect("call")
        .unwrap_err(),
        ExportRefusal::CapabilityRequired
    );
    assert!(
        record_export::export_at(
            &fx.pool,
            fx.instructor_id,
            Scope::Version {
                record_id: r_hashed,
                version_number: 2,
            },
            EXPORTED_AT,
        )
        .await
        .expect("call")
        .is_ok()
    );
    assert_eq!(
        record_export::export_at(
            &fx.pool,
            fx.outsider_id,
            Scope::Record {
                record_id: r_hashed,
            },
            EXPORTED_AT,
        )
        .await
        .expect("call")
        .unwrap_err(),
        ExportRefusal::CapabilityRequired
    );
    assert_eq!(
        record_export::export_at(
            &fx.pool,
            fx.executor_id,
            Scope::Version {
                record_id: r_hashed,
                version_number: 3,
            },
            EXPORTED_AT,
        )
        .await
        .expect("call")
        .unwrap_err(),
        ExportRefusal::NoSuchVersion
    );
    assert_eq!(
        record_export::export_at(
            &fx.pool,
            fx.executor_id,
            Scope::Record { record_id: 99999 },
            EXPORTED_AT,
        )
        .await
        .expect("call")
        .unwrap_err(),
        ExportRefusal::NoSuchRecord
    );

    // HTTP authorization parity: outsider gets 403, out-of-range version gets 404.
    let app =
        consolebook_server::http::router(consolebook_server::http::AppState::new(fx.pool.clone()));
    let outsider_cookie = support::login(app.clone(), "robin.outsider", support::password()).await;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/drafts/{r_hashed}/export"))
                .header(
                    COOKIE,
                    format!(
                        "{}={outsider_cookie}",
                        consolebook_server::http::SESSION_COOKIE
                    ),
                )
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let admin_cookie = support::login(app.clone(), "avery.admin", support::password()).await;
    let resp_summary = app
        .oneshot(
            Request::builder()
                .uri("/api/exports/summary")
                .header(
                    COOKIE,
                    format!(
                        "{}={admin_cookie}",
                        consolebook_server::http::SESSION_COOKIE
                    ),
                )
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp_summary.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &resp_summary
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("json");
    assert_eq!(body["record_count"], 1);
    assert_eq!(body["version_count"], 1);
    assert_eq!(body["disposed_record_count"], 3);
    assert_eq!(body["boundary_checkpoint_count"], 0);
}

#[tokio::test]
async fn metadata_boundary_checkpoints_verify_alongside_retained_and_on_fully_closed_scopes() {
    let fx = DispExportFixture::new().await;
    let (_, meta_policy_id) = fx.set_destroy_policies().await;
    let (r1, fin1) = fx
        .create_finalized_daily(
            fx.enrollment_id,
            "2026-06-01",
            "Invented retained record with boundary",
            None,
        )
        .await;

    // 1. Retained record + boundary checkpoint (no remaining disposition_event rows).
    let cp_id = fx
        .insert_boundary_checkpoint(meta_policy_id, fin1 + 100)
        .await;
    let exp_with_retained =
        record_export::export_at(&fx.pool, fx.admin_id, Scope::Installation, EXPORTED_AT)
            .await
            .expect("call")
            .expect("export");
    assert_eq!(exp_with_retained.unit_count, 1);
    assert_eq!(exp_with_retained.disposed_record_count, 0);
    assert_eq!(exp_with_retained.boundary_checkpoint_count, 1);

    let rep = export_verify::verify_archive(&exp_with_retained.bytes);
    assert!(rep.verified(), "{rep:?}");
    assert_eq!(
        rep.content_status(),
        ArchiveContentStatus::RetainedWithPolicyBoundary
    );
    let disp = rep.disposition.as_ref().expect("disp");
    assert_eq!(disp.boundary_checkpoints.len(), 1);
    assert_eq!(disp.boundary_checkpoints[0].checkpoint_id, cp_id);
    assert_eq!(
        disp.boundary_checkpoints[0].verdict,
        BoundaryCheckpointVerdict::IntentionalPolicyBoundary
    );

    // 2. Enrollment with ZERO retained records and ZERO tombstones still exports
    //    its intentional policy boundary honestly (never silent omission).
    let exp_boundary_only = record_export::export_at(
        &fx.pool,
        fx.executor_id,
        Scope::Enrollment {
            enrollment_id: fx.other_enrollment_id,
        },
        EXPORTED_AT,
    )
    .await
    .expect("call")
    .expect("boundary-only enrollment export");
    assert_eq!(exp_boundary_only.unit_count, 0);
    assert_eq!(exp_boundary_only.disposed_record_count, 0);
    assert_eq!(exp_boundary_only.boundary_checkpoint_count, 1);
    let rep_only = export_verify::verify_archive(&exp_boundary_only.bytes);
    assert!(rep_only.verified(), "{rep_only:?}");
    assert_eq!(
        rep_only.content_status(),
        ArchiveContentStatus::PolicyBoundaryOnly
    );
    assert!(r1 > 0);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn legacy_v1_compatibility_and_negative_fixtures_cover_every_failure_mode() {
    let fx = DispExportFixture::new().await;
    let (_, meta_policy_id) = fx.set_destroy_policies().await;
    let (r_retained, fin1) = fx
        .create_finalized_daily(
            fx.enrollment_id,
            "2026-06-01",
            "Invented retained record",
            None,
        )
        .await;
    let (r_disposed, fin2) = fx
        .create_finalized_daily(
            fx.enrollment_id,
            "2026-06-02",
            "Invented disposed record",
            Some("Invented disposed v2"),
        )
        .await;
    let evaluated_at = fin1.max(fin2) + 31 * SECONDS_PER_DAY;
    fx.dispose_records(vec![r_disposed], evaluated_at, true)
        .await;

    // 1. Legacy v1 archive compatibility: rewrite a real retained v2 export's
    //    manifest to exact format_version 1 shape (no disposition_evidence).
    let retained_v2 = record_export::export_at(
        &fx.pool,
        fx.executor_id,
        Scope::Record {
            record_id: r_retained,
        },
        EXPORTED_AT,
    )
    .await
    .expect("call")
    .expect("retained v2");
    let v2_entries = entries(&retained_v2.bytes);
    let v2_manifest: ArchiveManifest =
        serde_json::from_slice(entry(&v2_entries, ARCHIVE_MANIFEST_PATH)).expect("manifest");
    assert_eq!(v2_manifest.format_version, FORMAT_VERSION);
    assert!(v2_manifest.disposition_evidence.is_none());
    let unit_manifest: UnitManifest = serde_json::from_slice(entry(
        &v2_entries,
        &format!("records/{r_retained}/v1/manifest.json"),
    ))
    .expect("unit manifest");
    assert_eq!(unit_manifest.format_version, UNIT_FORMAT_VERSION);

    let v1_manifest = ArchiveManifestV1 {
        format: ARCHIVE_FORMAT.to_owned(),
        format_version: LEGACY_FORMAT_VERSION,
        installation_id: v2_manifest.installation_id.clone(),
        exported_at: v2_manifest.exported_at,
        scope: v2_manifest.scope,
        units: v2_manifest.units.clone(),
    };
    let v1_manifest_bytes =
        canonical::canonical_bytes(&serde_json::to_value(&v1_manifest).expect("val"))
            .expect("canonical v1");
    let v1_entries = with_entry(&v2_entries, ARCHIVE_MANIFEST_PATH, |_| {
        Some(v1_manifest_bytes)
    });
    let legacy_v1_zip = repack(&v1_entries);
    let legacy_rep = export_verify::verify_archive(&legacy_v1_zip);
    assert!(
        legacy_rep.verified(),
        "legacy v1 must verify: {legacy_rep:?}"
    );
    assert_eq!(
        legacy_rep.content_status(),
        ArchiveContentStatus::RetainedContentOnly
    );

    // Legacy v1 with extra disposition/evidence.json is refused as UnlistedEntry.
    let mut v1_with_extra = v1_entries.clone();
    v1_with_extra.push((DISPOSITION_EVIDENCE_PATH.to_owned(), b"{}".to_vec()));
    let rep_v1_extra = export_verify::verify_archive(&repack(&v1_with_extra));
    assert!(!rep_v1_extra.verified());
    assert!(rep_v1_extra.findings.contains(&Finding::UnlistedEntry {
        path: DISPOSITION_EVIDENCE_PATH.to_owned()
    }));

    // Unsupported version 3 is refused by name.
    let v3_zip = repack(&with_entry(&v2_entries, ARCHIVE_MANIFEST_PATH, |b| {
        Some(edit_json(b, |m| {
            m["format_version"] = serde_json::Value::from(3);
        }))
    }));
    let rep_v3 = export_verify::verify_archive(&v3_zip);
    assert!(rep_v3.findings.contains(&Finding::UnsupportedFormat {
        format: ARCHIVE_FORMAT.to_owned(),
        format_version: 3
    }));

    // 2. Negative fixtures on a mixed v2 archive (with boundary checkpoint too).
    fx.insert_boundary_checkpoint(meta_policy_id, evaluated_at + 20)
        .await;
    let mixed = record_export::export_at(
        &fx.pool,
        fx.executor_id,
        Scope::Enrollment {
            enrollment_id: fx.enrollment_id,
        },
        EXPORTED_AT,
    )
    .await
    .expect("call")
    .expect("mixed");
    let listed = entries(&mixed.bytes);

    // 2a. Missing disposition/evidence.json -> MissingEntry.
    let missing_ev = repack(&with_entry(&listed, DISPOSITION_EVIDENCE_PATH, |_| None));
    let rep = export_verify::verify_archive(&missing_ev);
    assert!(!rep.verified());
    assert!(
        rep.disposition
            .as_ref()
            .expect("disp")
            .findings
            .contains(&Finding::MissingEntry {
                path: DISPOSITION_EVIDENCE_PATH.to_owned()
            })
    );

    // 2b. Tampered evidence bytes without rehash -> DocumentHashMismatch.
    let hash_bad = repack(&with_entry(&listed, DISPOSITION_EVIDENCE_PATH, |b| {
        let mut copy = b.to_vec();
        copy.push(b'\n');
        Some(copy)
    }));
    let rep = export_verify::verify_archive(&hash_bad);
    assert!(rep.disposition.as_ref().expect("disp").findings.contains(
        &Finding::DocumentHashMismatch {
            path: DISPOSITION_EVIDENCE_PATH.to_owned()
        }
    ));

    // 2c. Rehashed evidence carrying an unknown content-leaking field -> DocumentInvalid.
    let leaked_ev = edit_json(entry(&listed, DISPOSITION_EVIDENCE_PATH), |doc| {
        doc["dispositions"][0]["narrative"] = serde_json::Value::String("leaked".into());
    });
    let rep = export_verify::verify_archive(&repack(&rehash_evidence(&listed, leaked_ev)));
    assert!(!rep.verified());
    assert!(
        rep.disposition
            .as_ref()
            .expect("disp")
            .findings
            .iter()
            .any(|f| matches!(f, Finding::DocumentInvalid { .. })),
        "{rep:?}"
    );

    // 2d. Rehashed evidence with wrong installation_id and exported_at -> DocumentDisagrees.
    let mismatched_ev = edit_json(entry(&listed, DISPOSITION_EVIDENCE_PATH), |doc| {
        doc["installation_id"] = serde_json::Value::String("wrong-installation".into());
        doc["exported_at"] = serde_json::Value::from(EXPORTED_AT + 1);
    });
    let rep = export_verify::verify_archive(&repack(&rehash_evidence(&listed, mismatched_ev)));
    let disp_findings = &rep.disposition.as_ref().expect("disp").findings;
    assert!(disp_findings.contains(&Finding::DocumentDisagrees {
        path: DISPOSITION_EVIDENCE_PATH.to_owned(),
        member: "installation_id"
    }));
    assert!(disp_findings.contains(&Finding::DocumentDisagrees {
        path: DISPOSITION_EVIDENCE_PATH.to_owned(),
        member: "exported_at"
    }));

    // 2e. Empty evidence doc (both dispositions and boundary_checkpoints empty) -> DocumentInvalid.
    let empty_ev = edit_json(entry(&listed, DISPOSITION_EVIDENCE_PATH), |doc| {
        doc["dispositions"] = serde_json::Value::Array(Vec::new());
        doc["boundary_checkpoints"] = serde_json::Value::Array(Vec::new());
    });
    let rep = export_verify::verify_archive(&repack(&rehash_evidence(&listed, empty_ev)));
    assert!(!rep.verified());
    assert!(
        rep.disposition
            .as_ref()
            .expect("disp")
            .findings
            .iter()
            .any(|f| matches!(f, Finding::DocumentInvalid { .. }))
    );

    // 2f. Conflicting record_id present in BOTH units and dispositions -> DispositionRecordConflict.
    let conflict_ev = edit_json(entry(&listed, DISPOSITION_EVIDENCE_PATH), |doc| {
        doc["dispositions"][0]["evaluation_record_id"] = serde_json::Value::from(r_retained);
    });
    let rep = export_verify::verify_archive(&repack(&rehash_evidence(&listed, conflict_ev)));
    assert!(
        rep.findings.contains(&Finding::DispositionRecordConflict {
            record_id: r_retained
        }),
        "{rep:?}"
    );

    // 2g. Disposed record belonging to a different enrollment inside Enrollment scope -> DocumentReference.
    let wrong_enroll_ev = edit_json(entry(&listed, DISPOSITION_EVIDENCE_PATH), |doc| {
        doc["dispositions"][0]["enrollment_id"] = serde_json::Value::from(fx.other_enrollment_id);
    });
    let rep = export_verify::verify_archive(&repack(&rehash_evidence(&listed, wrong_enroll_ev)));
    assert!(
        rep.findings
            .iter()
            .any(|f| matches!(f, Finding::DocumentReference { .. })),
        "{rep:?}"
    );

    // 2h. Empty v2 archive (units: [], disposition_evidence: null) -> NoUnits.
    let empty_v2 = repack(&[(
        ARCHIVE_MANIFEST_PATH.to_owned(),
        edit_json(entry(&v2_entries, ARCHIVE_MANIFEST_PATH), |m| {
            m["units"] = serde_json::Value::Array(Vec::new());
            m["disposition_evidence"] = serde_json::Value::Null;
        }),
    )]);
    let rep = export_verify::verify_archive(&empty_v2);
    assert!(rep.findings.contains(&Finding::NoUnits), "{rep:?}");

    // 2i. Boundary checkpoint attached to a Record scope -> DocumentReference.
    let disposed_rec = record_export::export_at(
        &fx.pool,
        fx.executor_id,
        Scope::Record {
            record_id: r_disposed,
        },
        EXPORTED_AT,
    )
    .await
    .expect("call")
    .expect("disposed rec");
    let rec_listed = entries(&disposed_rec.bytes);
    let rec_with_cp = edit_json(entry(&rec_listed, DISPOSITION_EVIDENCE_PATH), |doc| {
        doc["boundary_checkpoints"] = serde_json::json!([{
            "authority_reference": "INVENTED-CP",
            "boundary_through_disposed_at": evaluated_at,
            "checkpoint_id": 1,
            "expired_event_count": 1,
            "metadata_policy_id": meta_policy_id,
            "reason_code": "metadata_retention_elapsed",
            "recorded_at": evaluated_at + 1
        }]);
    });
    let rep = export_verify::verify_archive(&repack(&rehash_evidence(&rec_listed, rec_with_cp)));
    assert!(
        rep.findings
            .iter()
            .any(|f| matches!(f, Finding::DocumentReference { .. })),
        "{rep:?}"
    );
}
