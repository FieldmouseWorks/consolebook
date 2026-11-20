//! Crash/cleanup fault injection, idempotent retry without duplicate
//! completion, and confirmation/authority/policy/hold/scope race verification
//! (ADR 0030; #214).

use axum::body::Body;
use axum::http::header::COOKIE;
use axum::http::{Request, StatusCode};
use consolebook_server::backup_copy::{self, BackupSettings};
use consolebook_server::disposition_evidence::{
    DispositionErrorCode, DispositionReasonCode, DispositionRunState,
};
use consolebook_server::retention::{
    self, DispositionExecutionFault, DispositionExecutionInput, DispositionPreviewInput, HoldInput,
    HoldKind, HoldScope, PolicyInput, RecordClass, RetentionAction, RetentionRefusal,
    RetentionTrigger,
};
use consolebook_server::{amendments, backup, doctor, http, summaries};
use tower::ServiceExt;

use crate::{ExecFixture, SECONDS_PER_DAY, support};

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn faults_before_commit_and_at_every_cleanup_boundary_recover_cleanly_on_retry() {
    let fx = ExecFixture::new().await;
    fx.set_destroy_policy(RecordClass::DailyReport, None, 30)
        .await;

    let (record_id, finalized_at, _sentinels) = fx
        .create_rich_amended_daily("2026-01-20", "INVENTED_FAULT_TEST")
        .await;

    let pre_snap = backup::run(&fx.data_dir, 14)
        .await
        .expect("pre backup")
        .snapshot;
    backup_copy::copy_snapshot_with_data_dir(&fx.data_dir, &pre_snap, &fx.copy_dir, 14)
        .await
        .expect("pre copy");

    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    let backups = fx.backups(true);
    let evaluated_at = finalized_at + 31 * SECONDS_PER_DAY;
    let preview = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![record_id],
        },
        Some(&backups),
        evaluated_at,
    )
    .await
    .expect("call")
    .expect("preview");
    assert!(preview.eligible);

    let input = DispositionExecutionInput {
        record_ids: vec![record_id],
        confirmation_digest: preview.confirmation_digest.clone(),
        expires_at: preview.expires_at,
        authority_reference: "INVENTED-FAULT-AUTH".into(),
        reason_code: DispositionReasonCode::ScheduledRetentionElapsed,
        retain_version_hashes: true,
    };

    // 1. Fault BEFORE DB commit -> full rollback, zero run row, record intact.
    assert!(
        retention::execute_disposition_with_fault(
            &fx.pool,
            fx.executor_id,
            &input,
            &backups,
            evaluated_at + 1,
            DispositionExecutionFault::BeforeDbCommit,
        )
        .await
        .is_err()
    );
    let run_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM disposition_run")
        .fetch_one(&fx.pool)
        .await
        .expect("runs");
    assert_eq!(run_count, 0);
    let rec_exists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM evaluation_record WHERE id = ?1")
            .bind(record_id)
            .fetch_one(&fx.pool)
            .await
            .expect("rec");
    assert_eq!(rec_exists, 1);

    // 2. Crash immediately AFTER DB commit before VACUUM -> db_committed_vacuum_pending.
    let rep = retention::execute_disposition_with_fault(
        &fx.pool,
        fx.executor_id,
        &input,
        &backups,
        evaluated_at + 2,
        DispositionExecutionFault::AfterDbCommitBeforeVacuum,
    )
    .await
    .expect("call")
    .expect("committed");
    let run_id = rep.run_id;
    assert!(!rep.is_completed());
    assert_eq!(rep.state, DispositionRunState::DbCommittedVacuumPending);

    // Unauthorized caller cannot retry.
    assert_eq!(
        retention::retry_disposition_run(&fx.pool, fx.admin_id, run_id, &backups)
            .await
            .expect("call"),
        Err(RetentionRefusal::CapabilityRequired)
    );
    assert_eq!(
        retention::retry_disposition_run(&fx.pool, fx.executor_id, 99999, &backups)
            .await
            .expect("call"),
        Err(RetentionRefusal::NoSuchDispositionRun)
    );

    // 3. Fault DURING VACUUM -> stays db_committed_vacuum_pending + VacuumFailed.
    let rep = retention::retry_disposition_run_with_fault(
        &fx.pool,
        fx.executor_id,
        run_id,
        &backups,
        evaluated_at + 3,
        DispositionExecutionFault::DuringVacuum,
    )
    .await
    .expect("call")
    .expect("retry");
    assert!(!rep.is_completed());
    assert_eq!(rep.state, DispositionRunState::DbCommittedVacuumPending);
    assert_eq!(
        rep.last_error_code,
        Some(DispositionErrorCode::VacuumFailed)
    );

    // 4. Fault DURING ledger write -> VACUUM finishes, advances to copy_cleanup_pending + LedgerWriteFailed.
    let rep = retention::retry_disposition_run_with_fault(
        &fx.pool,
        fx.executor_id,
        run_id,
        &backups,
        evaluated_at + 4,
        DispositionExecutionFault::DuringLedgerWrite,
    )
    .await
    .expect("call")
    .expect("retry");
    assert!(!rep.is_completed());
    assert_eq!(rep.state, DispositionRunState::CopyCleanupPending);
    assert!(rep.vacuum_completed);
    assert_eq!(
        rep.last_error_code,
        Some(DispositionErrorCode::LedgerWriteFailed)
    );

    // 5. Fault DURING clean snapshot -> pre-disposition snapshot MUST NOT be deleted!
    let rep = retention::retry_disposition_run_with_fault(
        &fx.pool,
        fx.executor_id,
        run_id,
        &backups,
        evaluated_at + 5,
        DispositionExecutionFault::DuringCleanSnapshot,
    )
    .await
    .expect("call")
    .expect("retry");
    assert!(!rep.is_completed());
    assert_eq!(rep.state, DispositionRunState::CopyCleanupPending);
    assert_eq!(
        rep.last_error_code,
        Some(DispositionErrorCode::CleanSnapshotFailed)
    );
    assert!(
        pre_snap.exists(),
        "pre-disposition snapshot must never be deleted before clean snapshot succeeds"
    );

    // 6. Fault DURING local cleanup -> clean snapshot created, stays copy_cleanup_pending + LocalCleanupFailed.
    let rep = retention::retry_disposition_run_with_fault(
        &fx.pool,
        fx.executor_id,
        run_id,
        &backups,
        evaluated_at + 6,
        DispositionExecutionFault::DuringLocalCleanup,
    )
    .await
    .expect("call")
    .expect("retry");
    assert!(!rep.is_completed());
    assert_eq!(rep.state, DispositionRunState::CopyCleanupPending);
    assert_eq!(
        rep.last_error_code,
        Some(DispositionErrorCode::LocalCleanupFailed)
    );
    assert!(rep.clean_snapshot.is_some());

    // 7. Unreachable off-server copy directory -> local cleanup finishes, stays copy_cleanup_pending + CopyDirUnreachable.
    let unmounted = fx.other_fs.path().join("unmounted-share");
    let unreachable_backups = BackupSettings {
        data_dir: fx.data_dir.clone(),
        interval_hours: 24,
        keep: 14,
        copy_dir: Some(unmounted.clone()),
    };
    let rep = retention::retry_disposition_run_at(
        &fx.pool,
        fx.executor_id,
        run_id,
        &unreachable_backups,
        evaluated_at + 7,
    )
    .await
    .expect("call")
    .expect("retry");
    assert!(!rep.is_completed());
    assert_eq!(rep.state, DispositionRunState::CopyCleanupPending);
    assert_eq!(
        rep.last_error_code,
        Some(DispositionErrorCode::CopyDirUnreachable)
    );
    assert!(!unmounted.exists());

    // Doctor reports incomplete run while copy_cleanup_pending.
    let findings = doctor::run(
        &fx.data_dir,
        &doctor::Options {
            backup_copy_dir: Some(&fx.copy_dir),
            ..doctor::Options::default()
        },
    )
    .await;
    assert!(doctor::has_failure(&findings), "{findings:?}");

    // 8. Reachable copy dir + fault DURING copy cleanup -> stays copy_cleanup_pending + CopyCleanupFailed.
    let rep = retention::retry_disposition_run_with_fault(
        &fx.pool,
        fx.executor_id,
        run_id,
        &backups,
        evaluated_at + 8,
        DispositionExecutionFault::DuringCopyCleanup,
    )
    .await
    .expect("call")
    .expect("retry");
    assert!(!rep.is_completed());
    assert_eq!(
        rep.last_error_code,
        Some(DispositionErrorCode::CopyCleanupFailed)
    );

    // 9. Final clean retry -> converges to Completed, doctor passes, and a
    // second retry refuses duplicate completion with zero duplicate audit rows.
    let rep = retention::retry_disposition_run_at(
        &fx.pool,
        fx.executor_id,
        run_id,
        &backups,
        evaluated_at + 9,
    )
    .await
    .expect("call")
    .expect("final retry");
    assert!(rep.is_completed(), "{rep:#?}");
    assert_eq!(rep.state, DispositionRunState::Completed);
    assert_eq!(rep.last_error_code, None);

    assert_eq!(
        retention::retry_disposition_run_at(
            &fx.pool,
            fx.executor_id,
            run_id,
            &backups,
            evaluated_at + 10,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::DispositionAlreadyCompleted)
    );
    let completed_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_event WHERE kind = 'disposition_completed' AND subject_id = ?1",
    )
    .bind(run_id)
    .fetch_one(&fx.pool)
    .await
    .expect("audit count");
    assert_eq!(
        completed_audits, 1,
        "must never record duplicate completion"
    );

    let findings = doctor::run(
        &fx.data_dir,
        &doctor::Options {
            backup_copy_dir: Some(&fx.copy_dir),
            ..doctor::Options::default()
        },
    )
    .await;
    assert!(!doctor::has_failure(&findings), "{findings:?}");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn races_against_holds_policies_scope_expiry_and_authority_fail_closed() {
    let fx = ExecFixture::new().await;
    let policy_id = fx
        .set_destroy_policy(RecordClass::DailyReport, None, 30)
        .await;
    let (record_id, finalized_at, _sentinels) = fx
        .create_rich_amended_daily("2026-01-25", "INVENTED_RACE_RECORD")
        .await;
    let backups = fx.backups(false);
    let evaluated_at = finalized_at + 31 * SECONDS_PER_DAY;

    let make_preview = || async {
        retention::preview_disposition_at(
            &fx.pool,
            fx.executor_id,
            &DispositionPreviewInput {
                record_ids: vec![record_id],
            },
            Some(&backups),
            evaluated_at,
        )
        .await
        .expect("call")
        .expect("preview")
    };

    // 1. Expired confirmation fails closed.
    let p = make_preview().await;
    assert_eq!(
        retention::execute_disposition_at(
            &fx.pool,
            fx.executor_id,
            &DispositionExecutionInput {
                record_ids: vec![record_id],
                confirmation_digest: p.confirmation_digest.clone(),
                expires_at: p.expires_at,
                authority_reference: "INVENTED-AUTH".into(),
                reason_code: DispositionReasonCode::ScheduledRetentionElapsed,
                retain_version_hashes: true,
            },
            &backups,
            p.expires_at + 1,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::ConfirmationExpired)
    );

    // 2. Unsupported reason_code or blank authority_reference fails closed.
    assert!(matches!(
        retention::execute_disposition_at(
            &fx.pool,
            fx.executor_id,
            &DispositionExecutionInput {
                record_ids: vec![record_id],
                confirmation_digest: p.confirmation_digest.clone(),
                expires_at: p.expires_at,
                authority_reference: "   ".into(),
                reason_code: DispositionReasonCode::MetadataRetentionElapsed,
                retain_version_hashes: true,
            },
            &backups,
            evaluated_at + 1,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::Invalid(_))
    ));

    // 3. Scope change (extra/wrong record_ids) fails closed as StaleConfirmation.
    let (other_id, _, _) = fx
        .create_rich_amended_daily("2026-01-26", "INVENTED_OTHER_RECORD")
        .await;
    let p = make_preview().await;
    assert_eq!(
        retention::execute_disposition_at(
            &fx.pool,
            fx.executor_id,
            &DispositionExecutionInput {
                record_ids: vec![record_id, other_id],
                confirmation_digest: p.confirmation_digest,
                expires_at: p.expires_at,
                authority_reference: "INVENTED-AUTH".into(),
                reason_code: DispositionReasonCode::ScheduledRetentionElapsed,
                retain_version_hashes: true,
            },
            &backups,
            evaluated_at + 1,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::StaleConfirmation)
    );

    // 4. Raced hold placed after preview -> StaleConfirmation.
    let p = make_preview().await;
    let hold_id = retention::create_hold(
        &fx.pool,
        fx.admin_id,
        &HoldInput {
            scope: HoldScope::Record { record_id },
            kind: HoldKind::Litigation,
            authority: "INVENTED-HOLD".into(),
            reason: "Raced litigation hold".into(),
        },
    )
    .await
    .expect("call")
    .expect("hold");
    assert_eq!(
        retention::execute_disposition_at(
            &fx.pool,
            fx.executor_id,
            &DispositionExecutionInput {
                record_ids: vec![record_id],
                confirmation_digest: p.confirmation_digest,
                expires_at: p.expires_at,
                authority_reference: "INVENTED-AUTH".into(),
                reason_code: DispositionReasonCode::ScheduledRetentionElapsed,
                retain_version_hashes: true,
            },
            &backups,
            evaluated_at + 1,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::StaleConfirmation)
    );
    retention::release_hold(&fx.pool, fx.admin_id, hold_id, "Release raced hold")
        .await
        .expect("call")
        .expect("release");

    // 5. Raced policy change after preview -> StaleConfirmation.
    let p = make_preview().await;
    let retain_policy = retention::create_policy(
        &fx.pool,
        fx.admin_id,
        &PolicyInput {
            record_class: RecordClass::DailyReport,
            expected_current_id: Some(policy_id),
            authority: "INVENTED-RETAIN".into(),
            retention_trigger: RetentionTrigger::FinalizedAt,
            retention_days: 0,
            action: RetentionAction::Retain,
            reason: "Raced policy switch".into(),
        },
    )
    .await
    .expect("call")
    .expect("retain policy");
    assert_eq!(
        retention::execute_disposition_at(
            &fx.pool,
            fx.executor_id,
            &DispositionExecutionInput {
                record_ids: vec![record_id],
                confirmation_digest: p.confirmation_digest,
                expires_at: p.expires_at,
                authority_reference: "INVENTED-AUTH".into(),
                reason_code: DispositionReasonCode::ScheduledRetentionElapsed,
                retain_version_hashes: true,
            },
            &backups,
            evaluated_at + 1,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::StaleConfirmation)
    );
    fx.set_destroy_policy(RecordClass::DailyReport, Some(retain_policy), 30)
        .await;

    // 6. Raced weekly summary link after preview -> StaleConfirmation.
    let p = make_preview().await;
    let daily_v2_id: i64 = sqlx::query_scalar(
        "SELECT id FROM evaluation_version WHERE evaluation_record_id = ?1 AND version_number = 2",
    )
    .bind(record_id)
    .fetch_one(&fx.pool)
    .await
    .expect("v2");
    let summary_id = summaries::create(
        &fx.pool,
        fx.instructor_id,
        fx.enrollment_id,
        Some(fx.weekly_form_id),
    )
    .await
    .expect("call")
    .expect("summary");
    let summary_rev = summaries::add_link(&fx.pool, fx.instructor_id, summary_id, daily_v2_id, 0)
        .await
        .expect("call")
        .expect("link");
    assert_eq!(
        retention::execute_disposition_at(
            &fx.pool,
            fx.executor_id,
            &DispositionExecutionInput {
                record_ids: vec![record_id],
                confirmation_digest: p.confirmation_digest,
                expires_at: p.expires_at,
                authority_reference: "INVENTED-AUTH".into(),
                reason_code: DispositionReasonCode::ScheduledRetentionElapsed,
                retain_version_hashes: true,
            },
            &backups,
            evaluated_at + 1,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::StaleConfirmation)
    );
    summaries::remove_link(
        &fx.pool,
        fx.instructor_id,
        summary_id,
        daily_v2_id,
        summary_rev,
    )
    .await
    .expect("call")
    .expect("unlink");

    // 7. Raced amendment opened after preview -> StaleConfirmation.
    let p = make_preview().await;
    amendments::open(&fx.pool, fx.reviewer_id, record_id, "Raced amendment")
        .await
        .expect("call")
        .expect("open");
    assert_eq!(
        retention::execute_disposition_at(
            &fx.pool,
            fx.executor_id,
            &DispositionExecutionInput {
                record_ids: vec![record_id],
                confirmation_digest: p.confirmation_digest,
                expires_at: p.expires_at,
                authority_reference: "INVENTED-AUTH".into(),
                reason_code: DispositionReasonCode::ScheduledRetentionElapsed,
                retain_version_hashes: true,
            },
            &backups,
            evaluated_at + 1,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::StaleConfirmation)
    );

    // 8. Raced grant revocation after preview -> CapabilityRequired.
    let p_other = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![other_id],
        },
        Some(&backups),
        evaluated_at,
    )
    .await
    .expect("call")
    .expect("preview other");
    retention::set_disposition_authority(
        &fx.pool,
        fx.admin_id,
        fx.executor_id,
        false,
        "Revoke raced grant",
    )
    .await
    .expect("call")
    .expect("revoke");
    assert_eq!(
        retention::execute_disposition_at(
            &fx.pool,
            fx.executor_id,
            &DispositionExecutionInput {
                record_ids: vec![other_id],
                confirmation_digest: p_other.confirmation_digest,
                expires_at: p_other.expires_at,
                authority_reference: "INVENTED-AUTH".into(),
                reason_code: DispositionReasonCode::ScheduledRetentionElapsed,
                retain_version_hashes: true,
            },
            &backups,
            evaluated_at + 1,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::CapabilityRequired)
    );

    // Zero disposition_run rows were created across all raced refusals.
    let runs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM disposition_run")
        .fetch_one(&fx.pool)
        .await
        .expect("runs");
    assert_eq!(runs, 0);

    // 9. Execution API is NOT exposed over HTTP in #214.
    let app = http::router(http::AppState::new(fx.pool.clone()).with_backups(backups));
    let session = support::login(app.clone(), "casey.executor", support::password()).await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/retention/disposition/execute")
                .header(COOKIE, format!("{}={session}", http::SESSION_COOKIE))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn off_server_copy_cleanup_refuses_foreign_installation_even_on_matching_basename() {
    let fx = ExecFixture::new().await;
    let foreign = ExecFixture::new().await;

    fx.set_destroy_policy(RecordClass::DailyReport, None, 30)
        .await;
    fx.set_destroy_policy(RecordClass::DispositionEvent, None, 3650)
        .await;
    let (record_id, finalized_at, _) = fx
        .create_rich_amended_daily("2026-09-10", "FOREIGN-GUARD")
        .await;

    let backups_no_copy = fx.backups(false);
    let evaluated_at = finalized_at + 31 * SECONDS_PER_DAY;
    let preview = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![record_id],
        },
        Some(&backups_no_copy),
        evaluated_at,
    )
    .await
    .expect("call")
    .expect("preview");
    assert!(preview.eligible);

    // Stop right after clean local snapshot creation (DuringLocalCleanup) so we
    // know the exact clean snapshot basename that retry will attempt to copy.
    let paused = retention::execute_disposition_with_fault(
        &fx.pool,
        fx.executor_id,
        &DispositionExecutionInput {
            record_ids: vec![record_id],
            confirmation_digest: preview.confirmation_digest,
            expires_at: preview.expires_at,
            authority_reference: "INVENTED-FOREIGN-GUARD".into(),
            reason_code: DispositionReasonCode::ScheduledRetentionElapsed,
            retain_version_hashes: true,
        },
        &backups_no_copy,
        evaluated_at + 1,
        DispositionExecutionFault::DuringLocalCleanup,
    )
    .await
    .expect("call")
    .expect("report");
    assert_eq!(paused.state, DispositionRunState::CopyCleanupPending);
    let clean_name = paused
        .clean_snapshot
        .as_ref()
        .expect("clean snapshot taken before local cleanup fault")
        .file_name()
        .expect("basename")
        .to_owned();

    // Seed a foreign installation's ONLY backup in a separate copy directory
    // under the EXACT same basename as our clean snapshot.
    let foreign_snap = backup::run_after_restore(&foreign.data_dir)
        .await
        .expect("foreign snapshot")
        .snapshot;
    let foreign_bytes = std::fs::read(&foreign_snap).expect("read foreign snapshot");
    let foreign_copy_dir = fx.other_fs.path().join("foreign-installation-copies");
    std::fs::create_dir_all(&foreign_copy_dir).expect("mkdir foreign copy dir");
    let foreign_target = foreign_copy_dir.join(&clean_name);
    std::fs::write(&foreign_target, &foreign_bytes).expect("place foreign sole backup");

    let misconfigured = BackupSettings {
        data_dir: fx.data_dir.clone(),
        interval_hours: 24,
        keep: 14,
        copy_dir: Some(foreign_copy_dir.clone()),
    };
    let refused = retention::retry_disposition_run_at(
        &fx.pool,
        fx.executor_id,
        paused.run_id,
        &misconfigured,
        evaluated_at + 2,
    )
    .await
    .expect("call")
    .expect("report");
    assert_eq!(refused.state, DispositionRunState::CopyCleanupPending);
    assert_eq!(
        refused.last_error_code,
        Some(DispositionErrorCode::CopyCleanupFailed)
    );
    assert!(!refused.is_completed());

    // The foreign installation's sole backup must remain byte-for-byte intact
    // and no recovery ledger may be written into the foreign directory.
    assert_eq!(
        std::fs::read(&foreign_target).expect("foreign snapshot must survive untouched"),
        foreign_bytes
    );
    assert!(
        !foreign_copy_dir
            .join(consolebook_server::disposition_recovery::DISPOSITION_LEDGER_FILENAME)
            .exists(),
        "must not write our ledger into a foreign installation's copy directory"
    );

    // Pointing retry back to our own copy directory completes cleanly.
    let own_backups = fx.backups(true);
    let completed = retention::retry_disposition_run_at(
        &fx.pool,
        fx.executor_id,
        paused.run_id,
        &own_backups,
        evaluated_at + 3,
    )
    .await
    .expect("call")
    .expect("report");
    assert!(completed.is_completed(), "{completed:?}");
}
