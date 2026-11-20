use super::*;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn policies_triggers_amendments_and_unfinalized_records_are_evaluated_accurately() {
    let fx = Fixture::new().await;
    fx.grant_retention_admin().await;
    fx.grant_executor(fx.executor_id).await;

    let enrollment_id = fx.enroll(fx.trainee_id).await;
    let (record_id, _v1, finalized_at) =
        fx.create_finalized_daily(enrollment_id, "2026-06-01").await;

    // Empty, duplicate, non-positive, and nonexistent IDs fail closed.
    assert!(matches!(
        retention::preview_disposition(
            &fx.pool,
            fx.executor_id,
            &DispositionPreviewInput { record_ids: vec![] },
            None
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::Invalid(_))
    ));
    assert!(matches!(
        retention::preview_disposition(
            &fx.pool,
            fx.executor_id,
            &DispositionPreviewInput {
                record_ids: vec![record_id, record_id]
            },
            None,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::Invalid(_))
    ));
    assert_eq!(
        retention::preview_disposition(
            &fx.pool,
            fx.executor_id,
            &DispositionPreviewInput {
                record_ids: vec![99_999]
            },
            None,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::NoSuchRecord)
    );

    // 1. Absent policy blocks.
    let preview = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![record_id],
        },
        None,
        finalized_at + 100 * SECONDS_PER_DAY,
    )
    .await
    .expect("call")
    .expect("preview");
    assert!(!preview.eligible);
    assert_eq!(
        preview.targets[0].blockers,
        vec![DispositionBlocker::NoPolicy {
            record_class: RecordClass::DailyReport
        }]
    );

    // 2. Retain policy blocks.
    let p1 = fx
        .create_policy(
            RecordClass::DailyReport,
            None,
            RetentionTrigger::FinalizedAt,
            0,
            RetentionAction::Retain,
            "INVENTED-RETAIN-1",
        )
        .await;
    let preview = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![record_id],
        },
        None,
        finalized_at + 100 * SECONDS_PER_DAY,
    )
    .await
    .expect("call")
    .expect("preview");
    assert!(!preview.eligible);
    assert_eq!(
        preview.targets[0].blockers,
        vec![DispositionBlocker::RetainPolicy {
            policy_id: p1,
            version_number: 1,
            authority: "INVENTED-RETAIN-1".into(),
        }]
    );

    // 3. Destroy policy on finalized_at: exact elapsed 24h day boundary.
    let p2 = fx
        .create_policy(
            RecordClass::DailyReport,
            Some(p1),
            RetentionTrigger::FinalizedAt,
            30,
            RetentionAction::Destroy,
            "INVENTED-DESTROY-30D",
        )
        .await;
    let eligible_at = finalized_at + 30 * SECONDS_PER_DAY;
    let before = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![record_id],
        },
        None,
        eligible_at - 1,
    )
    .await
    .expect("call")
    .expect("preview");
    assert!(!before.eligible);
    assert_eq!(
        before.targets[0].blockers,
        vec![DispositionBlocker::WithinRetentionPeriod {
            policy_id: p2,
            trigger_at: finalized_at,
            eligible_at,
            remaining_seconds: 1,
        }]
    );

    let at_boundary = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![record_id],
        },
        None,
        eligible_at,
    )
    .await
    .expect("call")
    .expect("preview");
    assert!(at_boundary.eligible);
    assert_eq!(at_boundary.targets[0].blockers, Vec::new());

    // 4. Open amendment blocks; sealing v2 resets the finalized_at clock to v2.
    amendments::open(&fx.pool, fx.admin_id, record_id, "Invented correction")
        .await
        .expect("call")
        .expect("amendment");
    let amendment_id: i64 =
        sqlx::query_scalar("SELECT MAX(id) FROM amendment WHERE evaluation_record_id = ?1")
            .bind(record_id)
            .fetch_one(&fx.pool)
            .await
            .expect("amendment id");
    let open_am = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![record_id],
        },
        None,
        eligible_at + SECONDS_PER_DAY,
    )
    .await
    .expect("call")
    .expect("preview");
    assert!(!open_am.eligible);
    assert!(open_am.targets[0].blockers.iter().any(|b| matches!(
        b,
        DispositionBlocker::OpenAmendment { amendment_id: id, .. } if *id == amendment_id
    )));

    let current_rev: i64 =
        sqlx::query_scalar("SELECT revision FROM evaluation_record WHERE id = ?1")
            .bind(record_id)
            .fetch_one(&fx.pool)
            .await
            .expect("revision");
    let v2 = finalization::finalize(&fx.pool, fx.admin_id, record_id, current_rev)
        .await
        .expect("call")
        .expect("v2");
    let v2_id: i64 = sqlx::query_scalar(
        "SELECT id FROM evaluation_version WHERE evaluation_record_id = ?1 AND version_number = ?2",
    )
    .bind(record_id)
    .bind(v2.version_number)
    .fetch_one(&fx.pool)
    .await
    .expect("v2 id");
    let v2_finalized_at = v2.finalized_at;
    let after_v2 = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![record_id],
        },
        None,
        v2_finalized_at + 30 * SECONDS_PER_DAY - 1,
    )
    .await
    .expect("call")
    .expect("preview");
    assert!(!after_v2.eligible);
    assert_eq!(after_v2.targets[0].versions.len(), 2);
    assert!(matches!(
        after_v2.targets[0].blockers.as_slice(),
        [DispositionBlocker::WithinRetentionPeriod { trigger_at, .. }] if *trigger_at == v2_finalized_at
    ));

    // 5. Unfinalized draft is blocked.
    let draft_session = fx.create_session(enrollment_id, "2026-06-02").await;
    let draft_id = evaluation_drafts::create(
        &fx.pool,
        fx.instructor_id,
        draft_session,
        Some(fx.daily_form_id),
    )
    .await
    .expect("call")
    .expect("draft");
    let draft_preview = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![draft_id],
        },
        None,
        v2_finalized_at + 100 * SECONDS_PER_DAY,
    )
    .await
    .expect("call")
    .expect("preview");
    assert!(!draft_preview.eligible);
    assert!(
        draft_preview.targets[0]
            .blockers
            .contains(&DispositionBlocker::UnfinalizedRecord)
    );

    // 6. enrollment_closed_at trigger: active blocked, completed eligible after period, reinstated blocked.
    let _p3 = fx
        .create_policy(
            RecordClass::DailyReport,
            Some(p2),
            RetentionTrigger::EnrollmentClosedAt,
            10,
            RetentionAction::Destroy,
            "INVENTED-CLOSED-10D",
        )
        .await;
    let active_preview = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![record_id],
        },
        None,
        v2_finalized_at + 100 * SECONDS_PER_DAY,
    )
    .await
    .expect("call")
    .expect("preview");
    assert!(!active_preview.eligible);
    assert_eq!(
        active_preview.targets[0].blockers,
        vec![DispositionBlocker::EnrollmentNotClosed { enrollment_id }]
    );

    lifecycle::record_enrollment_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        EnrollmentEventKind::Complete,
        "Invented completion",
        None,
    )
    .await
    .expect("call")
    .expect("complete");
    let closed_at: i64 = sqlx::query_scalar(
        "SELECT occurred_at FROM enrollment_event WHERE enrollment_id = ?1 AND kind = 'complete' ORDER BY id DESC LIMIT 1",
    )
    .bind(enrollment_id)
    .fetch_one(&fx.pool)
    .await
    .expect("closed_at");
    let closed_before = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![record_id],
        },
        None,
        closed_at + 10 * SECONDS_PER_DAY - 1,
    )
    .await
    .expect("call")
    .expect("preview");
    assert!(!closed_before.eligible);

    let closed_at_boundary = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![record_id],
        },
        None,
        closed_at + 10 * SECONDS_PER_DAY,
    )
    .await
    .expect("call")
    .expect("preview");
    assert!(closed_at_boundary.eligible);

    lifecycle::record_enrollment_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        EnrollmentEventKind::Reinstate,
        "Invented reinstate",
        None,
    )
    .await
    .expect("call")
    .expect("reinstate");
    let reinstated_preview = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![record_id],
        },
        None,
        closed_at + 100 * SECONDS_PER_DAY,
    )
    .await
    .expect("call")
    .expect("preview");
    assert!(!reinstated_preview.eligible);
    assert_eq!(
        reinstated_preview.targets[0].blockers,
        vec![DispositionBlocker::EnrollmentNotClosed { enrollment_id }]
    );

    // Normal immutability remains unbroken after previewing.
    assert!(
        sqlx::query("DELETE FROM evaluation_version WHERE id = ?1")
            .bind(v2_id)
            .execute(&fx.pool)
            .await
            .is_err()
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn every_hold_kind_and_scope_blocks_until_released_and_unrelated_holds_do_not() {
    let fx = Fixture::new().await;
    fx.grant_retention_admin().await;
    fx.grant_executor(fx.executor_id).await;

    let enrollment_a = fx.enroll(fx.trainee_id).await;
    let (record_a, _, finalized_a) = fx.create_finalized_daily(enrollment_a, "2026-06-01").await;
    let (record_b, _, _) = fx.create_finalized_daily(enrollment_a, "2026-06-02").await;

    let other_trainee = {
        let mut tx = storage::write_tx(&fx.pool).await.expect("tx");
        let hash = consolebook_server::secrets::hash_password(support::password()).expect("hash");
        let id = users::create(&mut tx, "alex.trainee", "Alex Trainee", "", "", &hash)
            .await
            .expect("user");
        capabilities::grant_bundle(&mut tx, id, RoleBundle::Trainee.capabilities(), None)
            .await
            .expect("grants");
        tx.commit().await.expect("commit");
        id
    };
    let enrollment_other = fx.enroll(other_trainee).await;

    fx.create_policy(
        RecordClass::DailyReport,
        None,
        RetentionTrigger::FinalizedAt,
        0,
        RetentionAction::Destroy,
        "INVENTED-SCHEDULE-0D",
    )
    .await;

    // Unrelated record and enrollment holds do NOT block record_a.
    fx.create_hold(
        HoldScope::Record {
            record_id: record_b,
        },
        HoldKind::Audit,
        "INVENTED-OTHER-REC",
        "Invented hold on record B",
    )
    .await;
    fx.create_hold(
        HoldScope::Enrollment {
            enrollment_id: enrollment_other,
        },
        HoldKind::Investigation,
        "INVENTED-OTHER-ENR",
        "Invented hold on other enrollment",
    )
    .await;

    let clean = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![record_a],
        },
        None,
        finalized_a,
    )
    .await
    .expect("call")
    .expect("preview");
    assert!(clean.eligible);

    // Every combination of the 6 hold kinds across installation, enrollment, and record scopes blocks.
    let cases = [
        (
            HoldScope::Installation,
            HoldKind::Litigation,
            "INVENTED-LIT-1",
        ),
        (
            HoldScope::Enrollment {
                enrollment_id: enrollment_a,
            },
            HoldKind::AnticipatedLitigation,
            "INVENTED-ANT-2",
        ),
        (
            HoldScope::Record {
                record_id: record_a,
            },
            HoldKind::Audit,
            "INVENTED-AUD-3",
        ),
        (
            HoldScope::Installation,
            HoldKind::Investigation,
            "INVENTED-INV-4",
        ),
        (
            HoldScope::Enrollment {
                enrollment_id: enrollment_a,
            },
            HoldKind::PublicRecordsRequest,
            "INVENTED-PRR-5",
        ),
        (
            HoldScope::Record {
                record_id: record_a,
            },
            HoldKind::Other,
            "INVENTED-AGENCY-STATUTE-OTHER-6",
        ),
    ];

    for (scope, kind, authority) in cases {
        let hold_id = fx
            .create_hold(scope.clone(), kind, authority, "Invented preservation")
            .await;
        let blocked = retention::preview_disposition_at(
            &fx.pool,
            fx.executor_id,
            &DispositionPreviewInput {
                record_ids: vec![record_a],
            },
            None,
            finalized_a,
        )
        .await
        .expect("call")
        .expect("preview");
        assert!(!blocked.eligible, "{kind:?} on {scope:?} must block");
        assert!(
            blocked.targets[0]
                .blockers
                .contains(&DispositionBlocker::ActiveHold {
                    hold_id,
                    hold_kind: kind,
                    scope: scope.clone(),
                    authority: authority.into(),
                })
        );
        retention::release_hold(&fx.pool, fx.retention_admin_id, hold_id, "Invented release")
            .await
            .expect("call")
            .expect("release");
        let after_release = retention::preview_disposition_at(
            &fx.pool,
            fx.executor_id,
            &DispositionPreviewInput {
                record_ids: vec![record_a],
            },
            None,
            finalized_a,
        )
        .await
        .expect("call")
        .expect("preview");
        assert!(
            after_release.eligible,
            "releasing {kind:?} on {scope:?} must clear blocker"
        );
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn linked_summary_dependencies_refuse_without_broadening_and_honor_explicit_batch() {
    let fx = Fixture::new().await;
    fx.grant_retention_admin().await;
    fx.grant_executor(fx.executor_id).await;

    let enrollment_id = fx.enroll(fx.trainee_id).await;
    let (daily_id, daily_version_id, daily_finalized_at) =
        fx.create_finalized_daily(enrollment_id, "2026-06-01").await;
    let summary_id = fx.create_weekly_draft(enrollment_id).await;

    let rev = summaries::add_link(&fx.pool, fx.instructor_id, summary_id, daily_version_id, 0)
        .await
        .expect("call")
        .expect("link daily to weekly");
    let summary_meta = finalization::finalize(&fx.pool, fx.admin_id, summary_id, rev)
        .await
        .expect("call")
        .expect("finalize summary");
    let summary_finalized_at = summary_meta.finalized_at;

    fx.create_policy(
        RecordClass::DailyReport,
        None,
        RetentionTrigger::FinalizedAt,
        0,
        RetentionAction::Destroy,
        "INVENTED-DAILY-0D",
    )
    .await;
    let p_weekly = fx
        .create_policy(
            RecordClass::WeeklySummary,
            None,
            RetentionTrigger::FinalizedAt,
            0,
            RetentionAction::Destroy,
            "INVENTED-WEEKLY-0D",
        )
        .await;

    // 1. Selecting only the daily refuses without silently adding the weekly summary.
    let daily_only = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![daily_id],
        },
        None,
        summary_finalized_at,
    )
    .await
    .expect("call")
    .expect("preview");
    assert!(!daily_only.eligible);
    assert_eq!(daily_only.targets.len(), 1);
    assert_eq!(daily_only.targets[0].record_id, daily_id);
    assert_eq!(
        daily_only.targets[0].blockers,
        vec![DispositionBlocker::DependentSummaryRetained {
            summary_record_ids: vec![summary_id],
        }]
    );

    // 2. Selecting both eligible daily and eligible summary succeeds.
    let both = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![daily_id, summary_id],
        },
        None,
        summary_finalized_at,
    )
    .await
    .expect("call")
    .expect("preview");
    assert!(both.eligible);
    assert_eq!(both.targets.len(), 2);

    // 3. Selecting only the summary succeeds even when daily is held.
    let daily_hold = fx
        .create_hold(
            HoldScope::Record {
                record_id: daily_id,
            },
            HoldKind::Audit,
            "INVENTED-DAILY-HOLD",
            "Invented hold on daily only",
        )
        .await;
    let summary_only = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![summary_id],
        },
        None,
        summary_finalized_at,
    )
    .await
    .expect("call")
    .expect("preview");
    assert!(summary_only.eligible);
    retention::release_hold(
        &fx.pool,
        fx.retention_admin_id,
        daily_hold,
        "Invented release",
    )
    .await
    .expect("call")
    .expect("release");

    // 4. Co-selecting daily + blocked summary blocks the daily too (`DependentSummaryBlocked`).
    let _p_weekly_retain = fx
        .create_policy(
            RecordClass::WeeklySummary,
            Some(p_weekly),
            RetentionTrigger::FinalizedAt,
            0,
            RetentionAction::Retain,
            "INVENTED-WEEKLY-RETAIN",
        )
        .await;
    let both_blocked = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &DispositionPreviewInput {
            record_ids: vec![daily_id, summary_id],
        },
        None,
        daily_finalized_at.max(summary_finalized_at),
    )
    .await
    .expect("call")
    .expect("preview");
    assert!(!both_blocked.eligible);
    let daily_target = both_blocked
        .targets
        .iter()
        .find(|t| t.record_id == daily_id)
        .expect("daily target");
    assert!(
        daily_target
            .blockers
            .contains(&DispositionBlocker::DependentSummaryBlocked {
                summary_record_ids: vec![summary_id],
            })
    );
}
