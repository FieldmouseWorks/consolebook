//! Portable disposition evidence format, golden fixture verification, and
//! minimal event storage schema invariants (ADR 0030; #212).
//! All agencies, people, schedules, authorities, and hashes are invented.

mod support;

use consolebook_server::{
    canonical,
    capabilities::{self, RoleBundle},
    disposition_evidence::{
        self, AUTHENTICITY_SELF_CONSISTENCY_ONLY, DISPOSITION_EVIDENCE_FORMAT,
        DISPOSITION_EVIDENCE_FORMAT_VERSION, PortableBoundaryCheckpoint, PortableDisposedVersion,
        PortableDispositionEntry, PortableDispositionEvidence,
        verify::{
            BoundaryCheckpointVerdict, DisposedEntryVerdict, EvidenceFinding,
            VerifiedBoundarySummary, VerifiedDispositionSummary, verify_bytes,
        },
    },
    enrollments, programs,
    retention::{self, PolicyInput, RecordClass, RetentionAction, RetentionTrigger},
    storage, users,
};
use serde_json::Value;

const GOLDEN_EVIDENCE_V1: &[u8] = include_bytes!("fixtures/disposition-evidence-v1.json");

fn sample_document() -> PortableDispositionEvidence {
    PortableDispositionEvidence {
        format: DISPOSITION_EVIDENCE_FORMAT.into(),
        format_version: DISPOSITION_EVIDENCE_FORMAT_VERSION,
        installation_id: "invented-installation-0001".into(),
        exported_at: 1_788_289_200,
        authenticity: AUTHENTICITY_SELF_CONSISTENCY_ONLY.into(),
        dispositions: vec![
            PortableDispositionEntry {
                evaluation_record_id: 101,
                enrollment_id: 10,
                record_class: "daily_report".into(),
                record_policy_id: 4,
                metadata_policy_id: Some(9),
                version_count: 2,
                earliest_finalized_at: 1_770_000_000,
                latest_finalized_at: 1_770_086_400,
                hashes_retained: true,
                authority_reference: "INVENTED-DAILY-SCHEDULE-2026".into(),
                reason_code: "scheduled_retention_elapsed".into(),
                method: "lawful_destruction".into(),
                outcome: "disposed".into(),
                disposed_at: 1_788_289_000,
                versions: vec![
                    PortableDisposedVersion {
                        version_number: 1,
                        record_schema: 2,
                        content_hash: "1".repeat(64),
                        chain_hash: "2".repeat(64),
                        predecessor_content_hash: None,
                        finalized_at: 1_770_000_000,
                    },
                    PortableDisposedVersion {
                        version_number: 2,
                        record_schema: 2,
                        content_hash: "3".repeat(64),
                        chain_hash: "4".repeat(64),
                        predecessor_content_hash: Some("1".repeat(64)),
                        finalized_at: 1_770_086_400,
                    },
                ],
            },
            PortableDispositionEntry {
                evaluation_record_id: 102,
                enrollment_id: 10,
                record_class: "phase_evaluation".into(),
                record_policy_id: 5,
                metadata_policy_id: None,
                version_count: 1,
                earliest_finalized_at: 1_775_000_000,
                latest_finalized_at: 1_775_000_000,
                hashes_retained: false,
                authority_reference: "INVENTED-PHASE-SCHEDULE-2026".into(),
                reason_code: "restore_ledger_reconciliation".into(),
                method: "restore_reconciliation".into(),
                outcome: "disposed".into(),
                disposed_at: 1_788_289_050,
                versions: Vec::new(),
            },
        ],
        boundary_checkpoints: vec![PortableBoundaryCheckpoint {
            checkpoint_id: 1,
            metadata_policy_id: 9,
            authority_reference: "INVENTED-META-SCHEDULE-2026".into(),
            reason_code: "metadata_retention_elapsed".into(),
            expired_event_count: 3,
            boundary_through_disposed_at: 1_780_000_000,
            recorded_at: 1_788_289_100,
        }],
    }
}

fn canonical_of(value: &Value) -> Vec<u8> {
    canonical::canonical_bytes(value).expect("canonical")
}

#[test]
fn golden_portable_fixture_agrees_with_producer_and_reports_honest_verdicts() {
    let produced = disposition_evidence::produce(&sample_document()).expect("produce");
    assert_eq!(produced.as_slice(), GOLDEN_EVIDENCE_V1);

    let report = verify_bytes(GOLDEN_EVIDENCE_V1);
    assert!(report.consistent(), "{:#?}", report.findings);
    assert_eq!(
        report.authenticity.as_deref(),
        Some(AUTHENTICITY_SELF_CONSISTENCY_ONLY)
    );
    assert_eq!(
        report.dispositions,
        vec![
            VerifiedDispositionSummary {
                evaluation_record_id: 101,
                version_count: 2,
                verdict: DisposedEntryVerdict::DisposedHashContinuityOnly,
            },
            VerifiedDispositionSummary {
                evaluation_record_id: 102,
                version_count: 1,
                verdict: DisposedEntryVerdict::DisposedContentAndHashesUnavailable,
            },
        ]
    );
    assert_eq!(
        report.boundary_checkpoints,
        vec![VerifiedBoundarySummary {
            checkpoint_id: 1,
            expired_event_count: 3,
            boundary_through_disposed_at: 1_780_000_000,
            verdict: BoundaryCheckpointVerdict::IntentionalPolicyBoundary,
        }]
    );

    // Formatting and display output carry opaque IDs/codes only — no personal/narrative text.
    let doc = sample_document();
    let rendered = format!(
        "{} | {} | {:?}",
        doc.dispositions[0], doc.boundary_checkpoints[0], report
    );
    assert!(rendered.contains("record 101"));
    assert!(rendered.contains("checkpoint 1"));
    assert!(!rendered.contains("signature"));
    assert!(!rendered.contains("verified_content"));
}

#[test]
#[allow(clippy::too_many_lines)]
fn tampered_portable_fixtures_fail_with_typed_findings() {
    let base: Value = serde_json::from_slice(GOLDEN_EVIDENCE_V1).expect("parse golden");

    // 1. Unknown format version is refused by name.
    for unknown_version in [0, 2, 99] {
        let mut mutated = base.clone();
        mutated["format_version"] = Value::from(unknown_version);
        let report = verify_bytes(&canonical_of(&mutated));
        assert!(!report.consistent());
        assert_eq!(
            report.findings,
            vec![EvidenceFinding::UnsupportedFormat {
                format: DISPOSITION_EVIDENCE_FORMAT.into(),
                format_version: unknown_version,
            }]
        );
    }

    // 2. Non-canonical JSON (pretty-printed) is rejected.
    let pretty = serde_json::to_vec_pretty(&base).expect("pretty");
    let report = verify_bytes(&pretty);
    assert!(!report.consistent());
    assert!(report.findings.contains(&EvidenceFinding::NotCanonical));

    // 3. False authenticity claim or injected signature field is refused.
    let mut signed = base.clone();
    signed["authenticity"] = Value::from("cryptographically_signed");
    let report = verify_bytes(&canonical_of(&signed));
    assert!(!report.consistent());
    assert!(
        report
            .findings
            .contains(&EvidenceFinding::UnsupportedAuthenticityClaim {
                authenticity: "cryptographically_signed".into(),
            })
    );

    let mut with_sig = base.clone();
    with_sig["signature"] = Value::from("invented-fake-sig");
    let report = verify_bytes(&canonical_of(&with_sig));
    assert!(!report.consistent());
    assert!(
        report
            .findings
            .contains(&EvidenceFinding::DisallowedOrUnknownField {
                path: "$".into(),
                field: "signature".into(),
            })
    );

    // 4. Content-bearing fields injected at any level are rejected.
    for (target_path, mutate, field) in [
        (
            "$.dispositions[0]",
            Box::new(|v: &mut Value| {
                v["dispositions"][0]["narrative"] = Value::from("Leaked invented narrative");
            }) as Box<dyn Fn(&mut Value)>,
            "narrative",
        ),
        (
            "$.dispositions[0]",
            Box::new(|v: &mut Value| {
                v["dispositions"][0]["display_name"] = Value::from("Taylor Trainee");
            }),
            "display_name",
        ),
        (
            "$.dispositions[0]",
            Box::new(|v: &mut Value| {
                v["dispositions"][0]["reason"] = Value::from("Free-form reason");
            }),
            "reason",
        ),
        (
            "$.dispositions[0].versions[0]",
            Box::new(|v: &mut Value| {
                v["dispositions"][0]["versions"][0]["canonical_bytes"] = Value::from("leaked");
            }),
            "canonical_bytes",
        ),
        (
            "$.boundary_checkpoints[0]",
            Box::new(|v: &mut Value| {
                v["boundary_checkpoints"][0]["response"] = Value::from("Leaked trainee response");
            }),
            "response",
        ),
    ] {
        let mut mutated = base.clone();
        mutate(&mut mutated);
        let report = verify_bytes(&canonical_of(&mutated));
        assert!(!report.consistent(), "must reject {field} at {target_path}");
        assert!(
            report
                .findings
                .contains(&EvidenceFinding::DisallowedOrUnknownField {
                    path: target_path.into(),
                    field: field.into(),
                }),
            "findings: {:#?}",
            report.findings
        );
    }

    // 5. Broken predecessor hash continuity is rejected.
    let mut broken_chain = base.clone();
    broken_chain["dispositions"][0]["versions"][1]["predecessor_content_hash"] =
        Value::from("f".repeat(64));
    let report = verify_bytes(&canonical_of(&broken_chain));
    assert!(!report.consistent());
    assert!(report.findings.iter().any(|f| matches!(
        f,
        EvidenceFinding::HashLineageBroken {
            evaluation_record_id: 101,
            ..
        }
    )));

    // 6. hashes_retained=false carrying versions is rejected; hashes_retained=true missing versions is rejected.
    let mut extra_versions = base.clone();
    extra_versions["dispositions"][1]["versions"] = base["dispositions"][0]["versions"].clone();
    let report = verify_bytes(&canonical_of(&extra_versions));
    assert!(!report.consistent());
    assert!(report.findings.iter().any(|f| matches!(
        f,
        EvidenceFinding::HashLineageBroken {
            evaluation_record_id: 102,
            ..
        }
    )));

    let mut missing_versions = base.clone();
    missing_versions["dispositions"][0]["versions"] = Value::Array(Vec::new());
    let report = verify_bytes(&canonical_of(&missing_versions));
    assert!(!report.consistent());
    assert!(report.findings.iter().any(|f| matches!(
        f,
        EvidenceFinding::HashLineageBroken {
            evaluation_record_id: 101,
            ..
        }
    )));

    // 7. Out-of-order dispositions are rejected.
    let mut swapped = base.clone();
    let d0 = swapped["dispositions"][0].clone();
    let d1 = swapped["dispositions"][1].clone();
    swapped["dispositions"] = Value::Array(vec![d1, d0]);
    let report = verify_bytes(&canonical_of(&swapped));
    assert!(!report.consistent());
    assert!(
        report
            .findings
            .iter()
            .any(|f| matches!(f, EvidenceFinding::OutOfOrder { .. }))
    );

    // 8. Producer refuses to emit an invalid document.
    let mut invalid_doc = sample_document();
    invalid_doc.authenticity = "signed".into();
    assert!(disposition_evidence::produce(&invalid_doc).is_err());
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn schema_invariants_enforce_independent_policies_immutability_and_monotonic_runs() {
    let tmp = tempfile::tempdir().expect("scratch");
    let pool = storage::open(&tmp.path().join("consolebook.db"))
        .await
        .expect("migrate");

    let mut tx = storage::write_tx(&pool).await.expect("seed");
    let hash = consolebook_server::secrets::hash_password(support::password()).expect("hash");
    let admin_id = users::create(&mut tx, "avery.admin", "Avery Admin", "", "", &hash)
        .await
        .expect("admin");
    capabilities::grant_bundle(
        &mut tx,
        admin_id,
        RoleBundle::Administrator.capabilities(),
        None,
    )
    .await
    .expect("admin bundle");
    let trainee_id = users::create(&mut tx, "taylor.trainee", "Taylor Trainee", "", "", &hash)
        .await
        .expect("trainee");
    capabilities::grant_bundle(
        &mut tx,
        trainee_id,
        RoleBundle::Trainee.capabilities(),
        None,
    )
    .await
    .expect("trainee bundle");
    tx.commit().await.expect("commit");

    retention::set_authority(&pool, admin_id, admin_id, true, "Invented appointment")
        .await
        .expect("call")
        .expect("grant");

    let content: programs::VersionContent = serde_json::from_value(serde_json::json!({
        "name": "Invented County Program",
        "label": "2026 A",
        "description": "",
        "phases": [],
        "phase_transitions": [],
        "competencies": [],
        "rating_scales": [],
        "rating_modifiers": [],
        "citations": [],
        "evaluation_forms": [{
            "record_type": "daily_report",
            "name": "Invented Daily",
            "instructions": "",
            "competencies": [],
            "narratives": [{"prompt": "Invented", "required": false}]
        }],
        "finalization_policy": {"review_approved": false, "required_narratives": false, "ratings_complete": false}
    }))
    .expect("content");
    let program_id = programs::create_program(&pool, admin_id, &content.name)
        .await
        .expect("call")
        .expect("program");
    let version_id = programs::create_version(&pool, admin_id, program_id, &content)
        .await
        .expect("call")
        .expect("version");
    programs::publish_version(&pool, admin_id, version_id)
        .await
        .expect("call")
        .expect("publish");
    let enrollment_id = enrollments::enroll(&pool, admin_id, version_id, trainee_id)
        .await
        .expect("call")
        .expect("enrollment");

    let daily_retain = retention::create_policy(
        &pool,
        admin_id,
        &PolicyInput {
            record_class: RecordClass::DailyReport,
            expected_current_id: None,
            authority: "INVENTED-DAILY-RETAIN".into(),
            retention_trigger: RetentionTrigger::FinalizedAt,
            retention_days: 0,
            action: RetentionAction::Retain,
            reason: "Invented retain".into(),
        },
    )
    .await
    .expect("call")
    .expect("daily retain");
    let daily_destroy = retention::create_policy(
        &pool,
        admin_id,
        &PolicyInput {
            record_class: RecordClass::DailyReport,
            expected_current_id: Some(daily_retain),
            authority: "INVENTED-DAILY-DESTROY".into(),
            retention_trigger: RetentionTrigger::FinalizedAt,
            retention_days: 30,
            action: RetentionAction::Destroy,
            reason: "Invented destroy".into(),
        },
    )
    .await
    .expect("call")
    .expect("daily destroy");
    let weekly_destroy = retention::create_policy(
        &pool,
        admin_id,
        &PolicyInput {
            record_class: RecordClass::WeeklySummary,
            expected_current_id: None,
            authority: "INVENTED-WEEKLY-DESTROY".into(),
            retention_trigger: RetentionTrigger::FinalizedAt,
            retention_days: 30,
            action: RetentionAction::Destroy,
            reason: "Invented weekly destroy".into(),
        },
    )
    .await
    .expect("call")
    .expect("weekly destroy");
    let meta_retain = retention::create_policy(
        &pool,
        admin_id,
        &PolicyInput {
            record_class: RecordClass::DispositionEvent,
            expected_current_id: None,
            authority: "INVENTED-META-RETAIN".into(),
            retention_trigger: RetentionTrigger::DisposedAt,
            retention_days: 0,
            action: RetentionAction::Retain,
            reason: "Invented metadata retain".into(),
        },
    )
    .await
    .expect("call")
    .expect("meta retain");
    let meta_destroy = retention::create_policy(
        &pool,
        admin_id,
        &PolicyInput {
            record_class: RecordClass::DispositionEvent,
            expected_current_id: Some(meta_retain),
            authority: "INVENTED-META-DESTROY".into(),
            retention_trigger: RetentionTrigger::DisposedAt,
            retention_days: 365,
            action: RetentionAction::Destroy,
            reason: "Invented metadata destroy".into(),
        },
    )
    .await
    .expect("call")
    .expect("meta destroy");

    let digest = "a".repeat(64);
    let h1 = "1".repeat(64);
    let h2 = "2".repeat(64);
    let h3 = "3".repeat(64);

    // 1. Run state machine: invalid state/timestamp rejected; forward progression succeeds; backward/completed mutation & delete rejected.
    assert!(
        sqlx::query(
            "INSERT INTO disposition_run (actor_user_id, confirmation_digest, authority_reference, reason_code, method, state, started_at, completed_at) VALUES (?1, ?2, 'INVENTED-1', 'scheduled_retention_elapsed', 'lawful_destruction', 'completed', 1000, NULL)",
        )
        .bind(admin_id)
        .bind(&digest)
        .execute(&pool)
        .await
        .is_err()
    );
    let run_id: i64 = sqlx::query(
        "INSERT INTO disposition_run (actor_user_id, confirmation_digest, authority_reference, reason_code, method, state, started_at, last_error_code) VALUES (?1, ?2, 'INVENTED-1', 'scheduled_retention_elapsed', 'lawful_destruction', 'db_committed_vacuum_pending', 1000, 'vacuum_failed')",
    )
    .bind(admin_id)
    .bind(&digest)
    .execute(&pool)
    .await
    .expect("insert run")
    .last_insert_rowid();

    sqlx::query("UPDATE disposition_run SET state = 'copy_cleanup_pending', last_error_code = 'copy_dir_unreachable' WHERE id = ?1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("advance to copy_cleanup_pending");
    // Cannot regress state backwards.
    assert!(
        sqlx::query(
            "UPDATE disposition_run SET state = 'db_committed_vacuum_pending' WHERE id = ?1"
        )
        .bind(run_id)
        .execute(&pool)
        .await
        .is_err()
    );
    // Cannot complete while last_error_code is still set.
    assert!(
        sqlx::query(
            "UPDATE disposition_run SET state = 'completed', completed_at = 1100 WHERE id = ?1"
        )
        .bind(run_id)
        .execute(&pool)
        .await
        .is_err()
    );
    sqlx::query("UPDATE disposition_run SET state = 'completed', completed_at = 1100, last_error_code = NULL WHERE id = ?1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("complete run");
    // Completed run is immutable and cannot be deleted.
    assert!(
        sqlx::query("UPDATE disposition_run SET completed_at = 1200 WHERE id = ?1")
            .bind(run_id)
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM disposition_run WHERE id = ?1")
            .bind(run_id)
            .execute(&pool)
            .await
            .is_err()
    );

    // 2. Independent policy reference triggers: retain policy or wrong class policy rejected.
    for (rec_policy, meta_policy) in [
        (daily_retain, Some(meta_destroy)),
        (weekly_destroy, Some(meta_destroy)),
        (daily_destroy, Some(daily_destroy)),
    ] {
        assert!(
            sqlx::query(
                "INSERT INTO disposition_event (disposition_run_id, evaluation_record_id, enrollment_id, record_class, record_policy_id, metadata_policy_id, version_count, earliest_finalized_at, latest_finalized_at, hashes_retained, actor_user_id, authority_reference, reason_code, method, outcome, disposed_at) VALUES (?1, 501, ?2, 'daily_report', ?3, ?4, 1, 900, 900, 1, ?5, 'INVENTED-1', 'scheduled_retention_elapsed', 'lawful_destruction', 'disposed', 1000)",
            )
            .bind(run_id)
            .bind(enrollment_id)
            .bind(rec_policy)
            .bind(meta_policy)
            .bind(admin_id)
            .execute(&pool)
            .await
            .is_err(),
            "policy pair ({rec_policy}, {meta_policy:?}) must be refused"
        );
    }

    // Valid event with hashes_retained = 1 and independent policies succeeds.
    let event_with_hashes: i64 = sqlx::query(
        "INSERT INTO disposition_event (disposition_run_id, evaluation_record_id, enrollment_id, record_class, record_policy_id, metadata_policy_id, version_count, earliest_finalized_at, latest_finalized_at, hashes_retained, actor_user_id, authority_reference, reason_code, method, outcome, disposed_at) VALUES (?1, 501, ?2, 'daily_report', ?3, ?4, 1, 900, 900, 1, ?5, 'INVENTED-1', 'scheduled_retention_elapsed', 'lawful_destruction', 'disposed', 1000)",
    )
    .bind(run_id)
    .bind(enrollment_id)
    .bind(daily_destroy)
    .bind(meta_retain)
    .bind(admin_id)
    .execute(&pool)
    .await
    .expect("valid event")
    .last_insert_rowid();

    // Version hash invariants: v1 accepts NULL predecessor, rejects non-null predecessor, rejects v2 > version_count.
    assert!(
        sqlx::query(
            "INSERT INTO disposition_version_hash (disposition_event_id, version_number, record_schema, content_hash, chain_hash, predecessor_content_hash, finalized_at) VALUES (?1, 1, 2, ?2, ?3, ?4, 900)",
        )
        .bind(event_with_hashes)
        .bind(&h1)
        .bind(&h2)
        .bind(&h3)
        .execute(&pool)
        .await
        .is_err()
    );
    sqlx::query(
        "INSERT INTO disposition_version_hash (disposition_event_id, version_number, record_schema, content_hash, chain_hash, predecessor_content_hash, finalized_at) VALUES (?1, 1, 2, ?2, ?3, NULL, 900)",
    )
    .bind(event_with_hashes)
    .bind(&h1)
    .bind(&h2)
    .execute(&pool)
    .await
    .expect("valid v1 hash");
    assert!(
        sqlx::query(
            "INSERT INTO disposition_version_hash (disposition_event_id, version_number, record_schema, content_hash, chain_hash, predecessor_content_hash, finalized_at) VALUES (?1, 2, 2, ?2, ?3, ?4, 900)",
        )
        .bind(event_with_hashes)
        .bind(&h2)
        .bind(&h3)
        .bind(&h1)
        .execute(&pool)
        .await
        .is_err()
    );

    // Event with hashes_retained = 0 refuses any version hash row.
    let event_no_hashes: i64 = sqlx::query(
        "INSERT INTO disposition_event (disposition_run_id, evaluation_record_id, enrollment_id, record_class, record_policy_id, metadata_policy_id, version_count, earliest_finalized_at, latest_finalized_at, hashes_retained, actor_user_id, authority_reference, reason_code, method, outcome, disposed_at) VALUES (?1, 502, ?2, 'daily_report', ?3, NULL, 1, 900, 900, 0, ?4, 'INVENTED-1', 'scheduled_retention_elapsed', 'lawful_destruction', 'disposed', 1000)",
    )
    .bind(run_id)
    .bind(enrollment_id)
    .bind(daily_destroy)
    .bind(admin_id)
    .execute(&pool)
    .await
    .expect("no-hash event")
    .last_insert_rowid();
    assert!(
        sqlx::query(
            "INSERT INTO disposition_version_hash (disposition_event_id, version_number, record_schema, content_hash, chain_hash, predecessor_content_hash, finalized_at) VALUES (?1, 1, 2, ?2, ?3, NULL, 900)",
        )
        .bind(event_no_hashes)
        .bind(&h1)
        .bind(&h2)
        .execute(&pool)
        .await
        .is_err()
    );

    // 3. Boundary checkpoint requires destroy disposition_event policy and is immutable.
    assert!(
        sqlx::query(
            "INSERT INTO disposition_boundary_checkpoint (disposition_run_id, metadata_policy_id, authority_reference, reason_code, expired_event_count, boundary_through_disposed_at, recorded_by, recorded_at) VALUES (?1, ?2, 'INVENTED-META', 'metadata_retention_elapsed', 1, 1000, ?3, 1100)",
        )
        .bind(run_id)
        .bind(meta_retain)
        .bind(admin_id)
        .execute(&pool)
        .await
        .is_err()
    );
    sqlx::query(
        "INSERT INTO disposition_boundary_checkpoint (disposition_run_id, metadata_policy_id, authority_reference, reason_code, expired_event_count, boundary_through_disposed_at, recorded_by, recorded_at) VALUES (?1, ?2, 'INVENTED-META', 'metadata_retention_elapsed', 2, 1000, ?3, 1100)",
    )
    .bind(run_id)
    .bind(meta_destroy)
    .bind(admin_id)
    .execute(&pool)
    .await
    .expect("valid checkpoint");

    // Append-only triggers block UPDATE and DELETE on event, version_hash, and checkpoint tables.
    for stmt in [
        "UPDATE disposition_event SET authority_reference = 'X'",
        "DELETE FROM disposition_event",
        "UPDATE disposition_version_hash SET record_schema = 1",
        "DELETE FROM disposition_version_hash",
        "UPDATE disposition_boundary_checkpoint SET expired_event_count = 9",
        "DELETE FROM disposition_boundary_checkpoint",
    ] {
        assert!(sqlx::query(stmt).execute(&pool).await.is_err(), "{stmt}");
    }
}
