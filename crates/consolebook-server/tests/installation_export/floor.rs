//! The vocabulary floor: every value each vocabulary column held when
//! `format_version` 1 was defined. Vocabularies grow within a format
//! version and never lose a value, so an archive of this version stays
//! readable by every later build of it. The spec fingerprint leaves
//! vocabulary members out for exactly that reason; this floor is what
//! holds them.

use consolebook_server::installation_export::spec::{Table, Vocabulary};

use super::*;

/// (table, column, values) for every vocabulary column of
/// `format_version` 1, as recorded when the version was defined. Values
/// are only ever added to a vocabulary; never remove one from here or
/// from the code to make a test pass.
const FLOOR_V1: &[(&str, &str, &[&str])] = &[
    (
        "acknowledgment",
        "kind",
        &[
            "acknowledged",
            "acknowledged_with_response",
            "refused",
            "supervisor_attested_refusal",
            "unavailable",
        ],
    ),
    (
        "audit_event",
        "kind",
        &[
            "retention_authority_granted",
            "retention_authority_revoked",
            "retention_policy_created",
            "record_hold_created",
            "record_hold_replaced",
            "record_hold_released",
            "setup_completed",
            "login_succeeded",
            "login_failed",
            "logout",
            "reset_code_issued",
            "reset_code_used",
            "recovery_code_issued",
            "backup_completed",
            "restore_completed",
            "program_created",
            "program_version_created",
            "program_version_published",
            "program_version_imported",
            "program_version_discarded",
            "user_created",
            "enrollment_created",
            "enrollment_withdrawn",
            "enrollment_completed",
            "enrollment_reinstated",
            "enrollment_version_changed",
            "phase_event_recorded",
            "assignment_created",
            "assignment_ended",
            "session_created",
            "session_updated",
            "session_closed",
            "session_trainer_added",
            "session_trainer_removed",
            "draft_created",
            "draft_ownership_transferred",
            "draft_submitted",
            "draft_review_decided",
            "draft_finalized",
            "acknowledgment_recorded",
            "amendment_opened",
            "task_signoff_recorded",
            "record_exported",
            "trainee_packet_exported",
            "installation_exported",
        ],
    ),
    (
        "audit_event",
        "subject_kind",
        &[
            "retention_policy",
            "record_hold",
            "program",
            "program_version",
            "enrollment",
            "assignment",
            "session",
            "record",
        ],
    ),
    (
        "capability_grant",
        "capability",
        &[
            "manage_users",
            "manage_retention",
            "manage_programs",
            "assign_training",
            "export_records",
            "author_evaluation",
            "review_evaluation",
            "view_assigned_records",
            "view_own_records",
            "acknowledge_own_record",
        ],
    ),
    (
        "contributor_event",
        "kind",
        &[
            "created",
            "contributed",
            "ownership_transferred",
            "submitted_for_review",
            "review_decided",
        ],
    ),
    (
        "draft_snapshot",
        "reason",
        &["submission", "change_request_return"],
    ),
    (
        "enrollment_event",
        "kind",
        &["version_change", "withdraw", "complete", "reinstate"],
    ),
    (
        "evaluation_form",
        "record_type",
        &["daily_report", "weekly_summary", "phase_evaluation"],
    ),
    (
        "notice",
        "kind",
        &[
            "backup_failed",
            "backup_copy_failed",
            "assignment_created",
            "draft_ownership_received",
            "draft_submitted_for_review",
            "draft_approved",
            "draft_changes_requested",
            "draft_returned",
            "draft_finalized",
            "record_awaits_acknowledgment",
            "acknowledgment_response",
            "acknowledgment_refused",
            "acknowledgment_attested",
            "amendment_opened",
        ],
    ),
    (
        "phase_event",
        "kind",
        &[
            "advance", "return", "restart", "pause", "resume", "complete",
        ],
    ),
    (
        "phase_transition",
        "kind",
        &["advance", "remediation", "skip", "restart"],
    ),
    (
        "rating_scale",
        "kind",
        &["anchored_numeric", "pass_fail", "narrative_only"],
    ),
    (
        "record_hold",
        "kind",
        &[
            "litigation",
            "anticipated_litigation",
            "audit",
            "investigation",
            "public_records_request",
            "other",
        ],
    ),
    (
        "retention_policy",
        "record_class",
        &[
            "daily_report",
            "weekly_summary",
            "phase_evaluation",
            "disposition_event",
        ],
    ),
    (
        "retention_policy",
        "retention_trigger",
        &["finalized_at", "enrollment_closed_at", "disposed_at"],
    ),
    ("retention_policy", "action", &["retain", "destroy"]),
    (
        "review_decision",
        "decision",
        &["approved", "changes_requested", "returned"],
    ),
    (
        "task_signoff",
        "kind",
        &["observed", "demonstrated", "revoked"],
    ),
    (
        "training_session",
        "disposition",
        &["completed", "interrupted", "cancelled"],
    ),
];

/// Every floor value the vocabulary of `tables` no longer admits, and
/// every floor column that no longer carries a vocabulary.
fn floor_findings(floor: &[(&str, &str, &[&str])], tables: &[Table]) -> Vec<String> {
    let mut findings = Vec::new();
    for (table, column, values) in floor {
        let vocabulary = tables
            .iter()
            .find(|spec| spec.name == *table)
            .and_then(|spec| spec.column(column))
            .and_then(|column| column.vocabulary);
        let Some(vocabulary) = vocabulary else {
            findings.push(format!("{table}.{column}: no vocabulary"));
            continue;
        };
        for value in *values {
            if !vocabulary.admits(value) {
                findings.push(format!("{table}.{column}: lost {value}"));
            }
        }
    }
    findings
}

#[test]
fn vocabularies_keep_every_value_of_their_format_version() {
    let floor_columns: Vec<(&str, &str)> = FLOOR_V1.iter().map(|(t, c, _)| (*t, *c)).collect();
    // Every vocabulary column of this version is in the floor.
    for table in &spec::TABLES {
        for column in table.columns {
            if column.vocabulary.is_some() && INSTALLATION_FORMAT_VERSION == 1 {
                assert!(
                    floor_columns.contains(&(table.name, column.name)),
                    "{}.{} carries a vocabulary the v1 floor does not record",
                    table.name,
                    column.name
                );
            }
        }
    }
    assert_eq!(
        floor_findings(FLOOR_V1, &spec::TABLES),
        Vec::<String>::new()
    );
}

/// A vocabulary that lost a floor value, or a column that lost its
/// vocabulary, is reported; the fingerprint alone would not see either.
#[test]
fn a_vocabulary_that_loses_a_value_is_reported() {
    let mut tables = spec::TABLES.to_vec();
    let review = tables
        .iter()
        .position(|table| table.name == "review_decision")
        .expect("review_decision");
    let mut columns = tables[review].columns.to_vec();
    let decision = columns
        .iter()
        .position(|column| column.name == "decision")
        .expect("decision");
    columns[decision].vocabulary = Some(Vocabulary::OneOf(&["approved", "returned"]));
    tables[review].columns = Box::leak(columns.clone().into_boxed_slice());
    assert_eq!(
        floor_findings(FLOOR_V1, &tables),
        vec!["review_decision.decision: lost changes_requested".to_owned()]
    );
    // The fingerprint does not move: vocabulary members are the floor's.
    assert_eq!(
        canonical::content_hash_hex(
            spec::describe(&tables, &spec::EXCLUSIONS, &spec::UNIT_COLUMNS).as_bytes()
        ),
        spec::fingerprint()
    );
    columns[decision].vocabulary = None;
    tables[review].columns = Box::leak(columns.into_boxed_slice());
    assert_eq!(
        floor_findings(FLOOR_V1, &tables),
        vec!["review_decision.decision: no vocabulary".to_owned()]
    );
}
