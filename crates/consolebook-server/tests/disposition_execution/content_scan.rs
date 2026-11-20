//! Content purge, raw residual-byte scanning, dependency closure, and raw
//! immutability protection verification (#214).

use consolebook_server::disposition_evidence::{DispositionReasonCode, DispositionRunState};
use consolebook_server::disposition_recovery::{
    self, DISPOSITION_LEDGER_FILENAME, EXTERNAL_COPIES_NON_GUARANTEE,
};
use consolebook_server::notices::{self, NoticeKind};
use consolebook_server::retention::{
    self, DispositionExecutionInput, DispositionPreviewInput, HoldInput, HoldKind, HoldScope,
    RecordClass, RetentionRefusal,
};
use consolebook_server::{backup, backup_copy, doctor, restore};

use crate::{ExecFixture, SECONDS_PER_DAY, contains_bytes};

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn confirmed_disposition_purges_every_in_scope_table_and_residual_bytes_and_preserves_unrelated_state()
 {
    let fx = ExecFixture::new().await;
    fx.set_destroy_policy(RecordClass::DailyReport, None, 30)
        .await;
    fx.set_destroy_policy(RecordClass::DispositionEvent, None, 365)
        .await;

    let (disposed_id, disposed_finalized_at, disposed_sentinels) = fx
        .create_rich_amended_daily("2026-01-10", "INVENTED_PURGE_ALPHA")
        .await;
    let (retained_id, _retained_finalized_at, retained_sentinels) = fx
        .create_rich_amended_daily("2026-01-11", "INVENTED_KEEP_BETA")
        .await;

    // Place AND release a record-scoped hold on disposed_id to prove released
    // hold history stays intact without blocking lawful deletion (migration 0020).
    let released_hold_id = retention::create_hold(
        &fx.pool,
        fx.admin_id,
        &HoldInput {
            scope: HoldScope::Record {
                record_id: disposed_id,
            },
            kind: HoldKind::Audit,
            authority: "INVENTED-AUDIT-2026".into(),
            reason: "Invented hold then released".into(),
        },
    )
    .await
    .expect("call")
    .expect("hold");
    retention::release_hold(
        &fx.pool,
        fx.admin_id,
        released_hold_id,
        "Invented audit closed",
    )
    .await
    .expect("call")
    .expect("release");

    // Seed one legacy unlinked notice (evaluation_record_id = NULL) to prove it
    // survives while record-linked notices for disposed_id are deleted.
    notices::notify_user(
        &fx.pool,
        fx.admin_id,
        NoticeKind::AssignmentCreated,
        "INVENTED_LEGACY_UNLINKED_NOTICE",
    )
    .await
    .expect("legacy notice");

    // Checkpoint and take pre-disposition backups across every application-controlled family.
    let pre_local = backup::run(&fx.data_dir, 14)
        .await
        .expect("pre local backup")
        .snapshot;
    let saved_pre_copy = fx.tmp.path().join("saved-pre-disposition-copy.db");
    std::fs::copy(&pre_local, &saved_pre_copy).expect("save pre copy");
    backup_copy::copy_snapshot_with_data_dir(&fx.data_dir, &pre_local, &fx.copy_dir, 14)
        .await
        .expect("copy pre off-server");

    let premig = fx
        .data_dir
        .backups()
        .join("consolebook-premigration-20260110T000000Z.db");
    let prerestore_db = fx
        .data_dir
        .backups()
        .join("consolebook-prerestore-20260110T000001Z.db");
    let prerestore_dmg = fx
        .data_dir
        .backups()
        .join("consolebook-prerestore-20260110T000002Z.damaged");
    let partial = fx
        .data_dir
        .backups()
        .join("consolebook-20260110T000003Z.db.partial");
    let staged_export = fx.data_dir.exports().join("staged-pre-disposition.zip");
    for path in [
        &premig,
        &prerestore_db,
        &prerestore_dmg,
        &partial,
        &staged_export,
    ] {
        std::fs::copy(&pre_local, path).expect("seed pre-disposition artifact");
    }

    // Confirm sentinels ARE present in pre-disposition snapshot bytes before execution.
    let pre_bytes = std::fs::read(&pre_local).expect("read pre");
    for sentinel in &disposed_sentinels {
        assert!(
            contains_bytes(&pre_bytes, sentinel.as_bytes()),
            "pre-disposition fixture must contain {sentinel}"
        );
    }

    // Wait >1s so the fresh clean snapshot gets a distinct second timestamp.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    let backups = fx.backups(true);
    let evaluated_at = disposed_finalized_at + 31 * SECONDS_PER_DAY;
    let preview = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![disposed_id],
        },
        Some(&backups),
        evaluated_at,
    )
    .await
    .expect("call")
    .expect("preview");
    assert!(preview.eligible, "{preview:#?}");

    let report = retention::execute_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionExecutionInput {
            record_ids: vec![disposed_id],
            confirmation_digest: preview.confirmation_digest.clone(),
            expires_at: preview.expires_at,
            authority_reference: "INVENTED-DESTROY-AUTH-2026".into(),
            reason_code: DispositionReasonCode::ScheduledRetentionElapsed,
            retain_version_hashes: true,
        },
        &backups,
        evaluated_at + 10,
    )
    .await
    .expect("call")
    .expect("execute");

    assert!(report.is_completed(), "{report:#?}");
    assert_eq!(report.state, DispositionRunState::Completed);
    assert_eq!(report.disposed_record_ids, vec![disposed_id]);
    assert!(report.vacuum_completed);
    assert!(report.local_ledger_updated);
    assert_eq!(report.external_copies_note, EXTERNAL_COPIES_NON_GUARANTEE);
    let clean_snap = report.clean_snapshot.clone().expect("clean snapshot");
    assert!(clean_snap.is_file());

    // 1. Every in-scope SQLite table has zero rows for disposed_id and foreign_key_check is empty.
    for (table, col) in [
        ("evaluation_record", "id"),
        ("evaluation_version", "evaluation_record_id"),
        ("amendment", "evaluation_record_id"),
        ("draft_rating", "evaluation_record_id"),
        ("draft_narrative", "evaluation_record_id"),
        ("draft_snapshot", "evaluation_record_id"),
        ("review_decision", "evaluation_record_id"),
        ("contributor_event", "evaluation_record_id"),
        ("evaluation_session", "evaluation_record_id"),
        ("notice", "evaluation_record_id"),
    ] {
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM {table} WHERE {col} = ?1"
        )))
        .bind(disposed_id)
        .fetch_one(&fx.pool)
        .await
        .expect("count");
        assert_eq!(
            count, 0,
            "table {table} still holds rows for disposed record"
        );
    }
    let fk_issues: Vec<(String, i64, String, i64)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(&fx.pool)
        .await
        .expect("fk check");
    assert_eq!(
        fk_issues,
        Vec::new(),
        "PRAGMA foreign_key_check must be empty"
    );

    // 2. Minimal tombstones & version hashes exist with continuous v1->v2 chain.
    let hash_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM disposition_version_hash vh \
         JOIN disposition_event de ON de.id = vh.disposition_event_id \
         WHERE de.evaluation_record_id = ?1",
    )
    .bind(disposed_id)
    .fetch_one(&fx.pool)
    .await
    .expect("hash count");
    assert_eq!(hash_count, 2);

    // 3. Unrelated records, signoffs, released hold history, and legacy unlinked notice survive.
    let retained_exists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM evaluation_record WHERE id = ?1")
            .bind(retained_id)
            .fetch_one(&fx.pool)
            .await
            .expect("retained");
    assert_eq!(retained_exists, 1);
    let released_hold_exists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM record_hold WHERE id = ?1")
            .bind(released_hold_id)
            .fetch_one(&fx.pool)
            .await
            .expect("released hold");
    assert_eq!(released_hold_exists, 1);
    let signoff_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_signoff")
        .fetch_one(&fx.pool)
        .await
        .expect("signoffs");
    assert_eq!(signoff_count, 1);
    let legacy_notice: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notice WHERE message = 'INVENTED_LEGACY_UNLINKED_NOTICE'",
    )
    .fetch_one(&fx.pool)
    .await
    .expect("legacy notice");
    assert_eq!(legacy_notice, 1);

    // Audit trail carries disposition_executed and disposition_completed.
    let audit_kinds: Vec<String> = sqlx::query_scalar(
        "SELECT kind FROM audit_event WHERE subject_kind = 'disposition_run' AND subject_id = ?1 ORDER BY id",
    )
    .bind(report.run_id)
    .fetch_all(&fx.pool)
    .await
    .expect("audit kinds");
    assert_eq!(
        audit_kinds,
        vec!["disposition_executed", "disposition_completed"]
    );

    // 4. All pre-disposition local and off-server files were removed; clean copy & ledgers exist.
    for gone in [
        &pre_local,
        &premig,
        &prerestore_db,
        &prerestore_dmg,
        &partial,
        &staged_export,
    ] {
        assert!(
            !gone.exists(),
            "pre-disposition artifact {} must be removed",
            gone.display()
        );
    }
    let clean_name = clean_snap.file_name().expect("name");
    let clean_copy = fx.copy_dir.join(clean_name);
    assert!(clean_copy.is_file(), "clean copy must exist off-server");
    assert!(
        !fx.copy_dir
            .join(pre_local.file_name().expect("pre name"))
            .exists(),
        "pre-disposition off-server copy must be removed"
    );
    assert!(fx.data_dir.disposition_ledger().is_file());
    assert!(fx.copy_dir.join(DISPOSITION_LEDGER_FILENAME).is_file());

    // 5. Raw byte scan across live DB, -wal, -shm, clean local snapshot, clean
    // off-server copy, and both recovery ledgers: ZERO disposed sentinels!
    fx.pool.close().await;
    let mut files_to_scan = vec![
        fx.data_dir.database(),
        clean_snap.clone(),
        clean_copy,
        fx.data_dir.disposition_ledger(),
        fx.copy_dir.join(DISPOSITION_LEDGER_FILENAME),
    ];
    for sidecar in ["consolebook.db-wal", "consolebook.db-shm"] {
        let p = fx.data_dir.root().join(sidecar);
        if p.exists() {
            files_to_scan.push(p);
        }
    }
    for path in &files_to_scan {
        let bytes = std::fs::read(path).expect("read scan target");
        for sentinel in &disposed_sentinels {
            assert!(
                !contains_bytes(&bytes, sentinel.as_bytes()),
                "destroyed sentinel '{sentinel}' leaked into {}",
                path.display()
            );
        }
    }
    // Retained record content IS still present in live DB and clean snapshot.
    let db_bytes = std::fs::read(fx.data_dir.database()).expect("db bytes");
    assert!(contains_bytes(&db_bytes, retained_sentinels[6].as_bytes()));

    // 6. Doctor reports healthy and restore refuses the saved pre-disposition copy.
    let findings = doctor::run(
        &fx.data_dir,
        &doctor::Options {
            backup_copy_dir: Some(&fx.copy_dir),
            ..doctor::Options::default()
        },
    )
    .await;
    assert!(!doctor::has_failure(&findings), "{findings:?}");
    assert!(
        restore::run(&fx.data_dir, &saved_pre_copy).await.is_err(),
        "pre-disposition snapshot must be refused by restore"
    );
    let ledger = disposition_recovery::read_required_ledger(&fx.data_dir.disposition_ledger())
        .expect("ledger");
    assert_eq!(ledger.entries.len(), 1);
    assert_eq!(ledger.entries[0].evaluation_record_id, disposed_id);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn joint_daily_and_weekly_summary_batch_and_raw_immutability_guards() {
    let fx = ExecFixture::new().await;
    fx.set_destroy_policy(RecordClass::DailyReport, None, 30)
        .await;
    fx.set_destroy_policy(RecordClass::WeeklySummary, None, 30)
        .await;

    let (daily_id, _daily_fin, daily_sentinels) = fx
        .create_rich_amended_daily("2026-01-15", "INVENTED_DAILY_LINKED")
        .await;
    let daily_v2_id: i64 = sqlx::query_scalar(
        "SELECT id FROM evaluation_version WHERE evaluation_record_id = ?1 AND version_number = 2",
    )
    .bind(daily_id)
    .fetch_one(&fx.pool)
    .await
    .expect("daily v2");
    let weekly_sentinel = "INVENTED_WEEKLY_SYNTHESIS_PURGE";
    let (weekly_id, weekly_fin) = fx
        .create_finalized_weekly(&[daily_v2_id], weekly_sentinel)
        .await;

    // 1. Raw ad-hoc DELETE and UPDATE on every immutable/frozen table remain
    // strictly rejected outside an authorized disposition run.
    for raw_sql in [
        format!("DELETE FROM evaluation_version WHERE evaluation_record_id = {daily_id}"),
        format!(
            "UPDATE evaluation_version SET record_schema = 1 WHERE evaluation_record_id = {daily_id}"
        ),
        format!("DELETE FROM acknowledgment WHERE evaluation_version_id = {daily_v2_id}"),
        format!("DELETE FROM amendment WHERE evaluation_record_id = {daily_id}"),
        format!("DELETE FROM draft_snapshot WHERE evaluation_record_id = {daily_id}"),
        format!("DELETE FROM review_decision WHERE evaluation_record_id = {daily_id}"),
        format!("DELETE FROM contributor_event WHERE evaluation_record_id = {daily_id}"),
        format!("DELETE FROM draft_rating WHERE evaluation_record_id = {daily_id}"),
        format!(
            "DELETE FROM draft_rating_modifier WHERE draft_rating_id IN (SELECT id FROM draft_rating WHERE evaluation_record_id = {daily_id})"
        ),
        format!("DELETE FROM draft_narrative WHERE evaluation_record_id = {daily_id}"),
        format!("DELETE FROM evaluation_session WHERE evaluation_record_id = {daily_id}"),
        format!("DELETE FROM summary_daily_link WHERE summary_record_id = {weekly_id}"),
        format!("DELETE FROM evaluation_record WHERE id = {daily_id}"),
    ] {
        assert!(
            sqlx::query(sqlx::AssertSqlSafe(raw_sql.clone()))
                .execute(&fx.pool)
                .await
                .is_err(),
            "raw statement must fail closed: {raw_sql}"
        );
    }

    // 2. Selecting daily_id alone without weekly_id is ineligible and refused.
    let backups = fx.backups(false);
    let evaluated_at = weekly_fin + 31 * SECONDS_PER_DAY;
    let solo_preview = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![daily_id],
        },
        Some(&backups),
        evaluated_at,
    )
    .await
    .expect("call")
    .expect("preview");
    assert!(!solo_preview.eligible);
    assert_eq!(
        retention::execute_disposition_at(
            &fx.pool,
            fx.executor_id,
            &DispositionExecutionInput {
                record_ids: vec![daily_id],
                confirmation_digest: solo_preview.confirmation_digest,
                expires_at: solo_preview.expires_at,
                authority_reference: "INVENTED-AUTH".into(),
                reason_code: DispositionReasonCode::ScheduledRetentionElapsed,
                retain_version_hashes: false,
            },
            &backups,
            evaluated_at + 1,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::DispositionNotEligible)
    );

    // 3. Explicitly selecting [daily_id, weekly_id] removes summary_daily_link
    // and both lineages atomically with clean foreign_key_check and byte scan.
    let joint_preview = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![daily_id, weekly_id],
        },
        Some(&backups),
        evaluated_at,
    )
    .await
    .expect("call")
    .expect("joint preview");
    assert!(joint_preview.eligible, "{joint_preview:#?}");

    let report = retention::execute_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionExecutionInput {
            record_ids: vec![daily_id, weekly_id],
            confirmation_digest: joint_preview.confirmation_digest,
            expires_at: joint_preview.expires_at,
            authority_reference: "INVENTED-JOINT-AUTH".into(),
            reason_code: DispositionReasonCode::ScheduledRetentionElapsed,
            retain_version_hashes: false,
        },
        &backups,
        evaluated_at + 5,
    )
    .await
    .expect("call")
    .expect("joint execute");
    assert!(report.is_completed(), "{report:#?}");
    assert_eq!(report.disposed_record_ids, vec![daily_id, weekly_id]);

    let fk_issues: Vec<(String, i64, String, i64)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(&fx.pool)
        .await
        .expect("fk");
    assert_eq!(fk_issues, Vec::new());

    fx.pool.close().await;
    let db_bytes = std::fs::read(fx.data_dir.database()).expect("db");
    assert!(!contains_bytes(&db_bytes, weekly_sentinel.as_bytes()));
    for s in &daily_sentinels {
        assert!(!contains_bytes(&db_bytes, s.as_bytes()));
    }
}
