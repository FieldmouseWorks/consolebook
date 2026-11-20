use super::*;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn confirmation_digest_rejects_expiry_wrong_actor_and_intervening_reverted_changes() {
    let fx = Fixture::new().await;
    fx.grant_retention_admin().await;
    fx.grant_executor(fx.executor_id).await;
    fx.grant_executor(fx.second_executor_id).await;

    let enrollment_id = fx.enroll(fx.trainee_id).await;
    let (daily_id, daily_version_id, daily_finalized_at) =
        fx.create_finalized_daily(enrollment_id, "2026-06-01").await;
    let weekly_draft_id = fx.create_weekly_draft(enrollment_id).await;

    lifecycle::record_enrollment_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        EnrollmentEventKind::Complete,
        "Invented close",
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

    let p1 = fx
        .create_policy(
            RecordClass::DailyReport,
            None,
            RetentionTrigger::EnrollmentClosedAt,
            0,
            RetentionAction::Destroy,
            "INVENTED-CLOSED-0D",
        )
        .await;

    let now = daily_finalized_at.max(closed_at);
    let input = DispositionPreviewInput {
        record_ids: vec![daily_id],
    };

    // Baseline preview verifies cleanly within its validity window.
    let preview = retention::preview_disposition_at(&fx.pool, fx.executor_id, &input, None, now)
        .await
        .expect("call")
        .expect("preview");
    assert!(preview.eligible);
    assert_eq!(preview.expires_at, now + PREVIEW_VALIDITY_SECONDS);
    assert!(
        retention::verify_disposition_preview_at(
            &fx.pool,
            fx.executor_id,
            &input,
            &preview.confirmation_digest,
            preview.expires_at,
            None,
            now + 60,
        )
        .await
        .expect("call")
        .is_ok()
    );

    // 1. Expired confirmation is refused.
    assert_eq!(
        retention::verify_disposition_preview_at(
            &fx.pool,
            fx.executor_id,
            &input,
            &preview.confirmation_digest,
            preview.expires_at,
            None,
            preview.expires_at + 1,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::ConfirmationExpired)
    );

    // 2. Another authorized actor cannot reuse the first actor's digest.
    assert_eq!(
        retention::verify_disposition_preview_at(
            &fx.pool,
            fx.second_executor_id,
            &input,
            &preview.confirmation_digest,
            preview.expires_at,
            None,
            now,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::StaleConfirmation)
    );

    // 3. Intervening revoke + re-grant of execute_disposition invalidates the digest.
    retention::set_disposition_authority(
        &fx.pool,
        fx.admin_id,
        fx.executor_id,
        false,
        "Invented temporary pause",
    )
    .await
    .expect("call")
    .expect("revoke");
    retention::set_disposition_authority(
        &fx.pool,
        fx.admin_id,
        fx.executor_id,
        true,
        "Invented restoration",
    )
    .await
    .expect("call")
    .expect("regrant");
    assert_eq!(
        retention::verify_disposition_preview_at(
            &fx.pool,
            fx.executor_id,
            &input,
            &preview.confirmation_digest,
            preview.expires_at,
            None,
            now,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::StaleConfirmation)
    );

    // Refresh preview; 4. Intervening policy revise + revise back invalidates the digest.
    let preview = retention::preview_disposition_at(&fx.pool, fx.executor_id, &input, None, now)
        .await
        .expect("call")
        .expect("preview");
    let p2 = fx
        .create_policy(
            RecordClass::DailyReport,
            Some(p1),
            RetentionTrigger::EnrollmentClosedAt,
            0,
            RetentionAction::Retain,
            "INVENTED-TEMP-RETAIN",
        )
        .await;
    let _p3 = fx
        .create_policy(
            RecordClass::DailyReport,
            Some(p2),
            RetentionTrigger::EnrollmentClosedAt,
            0,
            RetentionAction::Destroy,
            "INVENTED-CLOSED-0D",
        )
        .await;
    assert_eq!(
        retention::verify_disposition_preview_at(
            &fx.pool,
            fx.executor_id,
            &input,
            &preview.confirmation_digest,
            preview.expires_at,
            None,
            now,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::StaleConfirmation)
    );

    // Refresh preview; 5. Intervening hold placed + released invalidates the digest.
    let preview = retention::preview_disposition_at(&fx.pool, fx.executor_id, &input, None, now)
        .await
        .expect("call")
        .expect("preview");
    let hold_id = fx
        .create_hold(
            HoldScope::Record {
                record_id: daily_id,
            },
            HoldKind::PublicRecordsRequest,
            "INVENTED-PRR-TEMP",
            "Invented brief hold",
        )
        .await;
    retention::release_hold(&fx.pool, fx.retention_admin_id, hold_id, "Invented release")
        .await
        .expect("call")
        .expect("release");
    assert_eq!(
        retention::verify_disposition_preview_at(
            &fx.pool,
            fx.executor_id,
            &input,
            &preview.confirmation_digest,
            preview.expires_at,
            None,
            now,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::StaleConfirmation)
    );

    // Refresh preview; 6. Intervening enrollment reinstate + re-complete invalidates the digest.
    let preview = retention::preview_disposition_at(&fx.pool, fx.executor_id, &input, None, now)
        .await
        .expect("call")
        .expect("preview");
    lifecycle::record_enrollment_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        EnrollmentEventKind::Reinstate,
        "Invented reopen",
        None,
    )
    .await
    .expect("call")
    .expect("reinstate");
    lifecycle::record_enrollment_event(
        &fx.pool,
        fx.admin_id,
        enrollment_id,
        EnrollmentEventKind::Complete,
        "Invented reclose",
        None,
    )
    .await
    .expect("call")
    .expect("recomplete");
    let reclosed_at: i64 = sqlx::query_scalar(
        "SELECT occurred_at FROM enrollment_event WHERE enrollment_id = ?1 AND kind = 'complete' ORDER BY id DESC LIMIT 1",
    )
    .bind(enrollment_id)
    .fetch_one(&fx.pool)
    .await
    .expect("reclosed_at");
    let now = now.max(reclosed_at);
    assert_eq!(
        retention::verify_disposition_preview_at(
            &fx.pool,
            fx.executor_id,
            &input,
            &preview.confirmation_digest,
            preview.expires_at,
            None,
            now,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::StaleConfirmation)
    );

    // Refresh preview; 7. Intervening summary link added + removed invalidates the digest.
    let preview = retention::preview_disposition_at(&fx.pool, fx.executor_id, &input, None, now)
        .await
        .expect("call")
        .expect("preview");
    let rev = summaries::add_link(
        &fx.pool,
        fx.instructor_id,
        weekly_draft_id,
        daily_version_id,
        0,
    )
    .await
    .expect("call")
    .expect("add link");
    summaries::remove_link(
        &fx.pool,
        fx.instructor_id,
        weekly_draft_id,
        daily_version_id,
        rev,
    )
    .await
    .expect("call")
    .expect("remove link");
    assert_eq!(
        retention::verify_disposition_preview_at(
            &fx.pool,
            fx.executor_id,
            &input,
            &preview.confirmation_digest,
            preview.expires_at,
            None,
            now,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::StaleConfirmation)
    );

    // Refresh preview; 8. Intervening acknowledgment invalidates the digest.
    let preview = retention::preview_disposition_at(&fx.pool, fx.executor_id, &input, None, now)
        .await
        .expect("call")
        .expect("preview");
    acknowledgments::acknowledge(
        &fx.pool,
        fx.trainee_id,
        daily_id,
        TraineeAckKind::Acknowledged,
        "",
    )
    .await
    .expect("call")
    .expect("ack");
    assert_eq!(
        retention::verify_disposition_preview_at(
            &fx.pool,
            fx.executor_id,
            &input,
            &preview.confirmation_digest,
            preview.expires_at,
            None,
            now,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::StaleConfirmation)
    );
}

#[tokio::test]
async fn concurrent_write_invalidates_confirmation_and_forged_digest_is_rejected() {
    let fx = Fixture::new().await;
    fx.grant_retention_admin().await;
    fx.grant_executor(fx.executor_id).await;

    let enrollment_id = fx.enroll(fx.trainee_id).await;
    let (daily_id, _, finalized_at) = fx.create_finalized_daily(enrollment_id, "2026-06-01").await;
    fx.create_policy(
        RecordClass::DailyReport,
        None,
        RetentionTrigger::FinalizedAt,
        0,
        RetentionAction::Destroy,
        "INVENTED-DAILY-0D",
    )
    .await;

    let input = DispositionPreviewInput {
        record_ids: vec![daily_id],
    };
    let preview =
        retention::preview_disposition_at(&fx.pool, fx.executor_id, &input, None, finalized_at)
            .await
            .expect("call")
            .expect("preview");
    assert!(preview.eligible);

    // Negative control: a modified digest fails closed even when nothing changed.
    let mut forged = preview.confirmation_digest.clone();
    let replacement = if forged.starts_with('0') { "1" } else { "0" };
    forged.replace_range(0..1, replacement);
    assert_eq!(
        retention::verify_disposition_preview_at(
            &fx.pool,
            fx.executor_id,
            &input,
            &forged,
            preview.expires_at,
            None,
            finalized_at,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::StaleConfirmation)
    );

    // Concurrent writer commits a hold while a verification task runs against the same pool.
    let pool_for_hold = fx.pool.clone();
    let retention_admin = fx.retention_admin_id;
    let hold_task = tokio::spawn(async move {
        retention::create_hold(
            &pool_for_hold,
            retention_admin,
            &HoldInput {
                scope: HoldScope::Record {
                    record_id: daily_id,
                },
                kind: HoldKind::Litigation,
                authority: "INVENTED-CONCURRENT-HOLD".into(),
                reason: "Invented concurrent preservation order".into(),
            },
        )
        .await
        .expect("call")
        .expect("hold")
    });
    let _hold_id = hold_task.await.expect("join");

    assert_eq!(
        retention::verify_disposition_preview_at(
            &fx.pool,
            fx.executor_id,
            &input,
            &preview.confirmation_digest,
            preview.expires_at,
            None,
            finalized_at,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::StaleConfirmation)
    );

    // Negative control: a preview taken while blocked is refused as DispositionNotEligible
    // even when its own fresh digest matches.
    let blocked_preview =
        retention::preview_disposition_at(&fx.pool, fx.executor_id, &input, None, finalized_at)
            .await
            .expect("call")
            .expect("blocked preview");
    assert!(!blocked_preview.eligible);
    assert_eq!(
        retention::verify_disposition_preview_at(
            &fx.pool,
            fx.executor_id,
            &input,
            &blocked_preview.confirmation_digest,
            blocked_preview.expires_at,
            None,
            finalized_at,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::DispositionNotEligible)
    );
}
