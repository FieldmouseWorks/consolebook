use super::*;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn disposition_authority_is_separate_from_role_bundles_and_manage_retention() {
    let fx = Fixture::new().await;
    fx.grant_retention_admin().await;
    let enrollment_id = fx.enroll(fx.trainee_id).await;
    let (record_id, _, _) = fx.create_finalized_daily(enrollment_id, "2026-06-01").await;

    // No role bundle includes execute_disposition.
    for bundle in [
        RoleBundle::Administrator,
        RoleBundle::Coordinator,
        RoleBundle::Trainer,
        RoleBundle::Trainee,
    ] {
        assert!(
            !bundle
                .capabilities()
                .contains(&Capability::ExecuteDisposition),
            "{bundle:?} must never include execute_disposition"
        );
    }

    let input = DispositionPreviewInput {
        record_ids: vec![record_id],
    };

    // Administrator, manage_retention holder, trainer, trainee are all refused.
    for actor in [
        fx.admin_id,
        fx.retention_admin_id,
        fx.instructor_id,
        fx.trainee_id,
    ] {
        assert_eq!(
            retention::preview_disposition(&fx.pool, actor, &input, None)
                .await
                .expect("call"),
            Err(RetentionRefusal::CapabilityRequired),
            "actor {actor} must be refused without execute_disposition"
        );
    }

    // Non-manage_users holders cannot grant execute_disposition.
    assert_eq!(
        retention::set_disposition_authority(
            &fx.pool,
            fx.retention_admin_id,
            fx.executor_id,
            true,
            "Invented unauthorized grant",
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::CapabilityRequired)
    );
    assert_eq!(
        retention::disposition_authority_history(&fx.pool, fx.retention_admin_id)
            .await
            .expect("call"),
        Err(RetentionRefusal::CapabilityRequired)
    );

    // Input validation, unknown user, and unchanged state are refused.
    assert!(matches!(
        retention::set_disposition_authority(&fx.pool, fx.admin_id, fx.executor_id, true, "   ")
            .await
            .expect("call"),
        Err(RetentionRefusal::Invalid(_))
    ));
    assert_eq!(
        retention::set_disposition_authority(&fx.pool, fx.admin_id, 99_999, true, "Invented")
            .await
            .expect("call"),
        Err(RetentionRefusal::NoSuchUser)
    );
    assert_eq!(
        retention::set_disposition_authority(
            &fx.pool,
            fx.admin_id,
            fx.executor_id,
            false,
            "Invented"
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::AuthorityUnchanged)
    );

    // Grant execute_disposition and verify separation of duties both ways.
    fx.grant_executor(fx.executor_id).await;
    assert!(
        capabilities::user_has(&fx.pool, fx.executor_id, Capability::ExecuteDisposition)
            .await
            .expect("check")
    );
    assert!(
        !capabilities::user_has(&fx.pool, fx.executor_id, Capability::ManageRetention)
            .await
            .expect("check")
    );

    // Executor alone cannot administer retention policies or holds.
    assert_eq!(
        retention::list_policies(&fx.pool, fx.executor_id)
            .await
            .expect("call"),
        Err(RetentionRefusal::CapabilityRequired)
    );
    assert_eq!(
        retention::list_holds(&fx.pool, fx.executor_id)
            .await
            .expect("call"),
        Err(RetentionRefusal::CapabilityRequired)
    );

    // Authority histories stay cleanly separated by capability.
    let retention_events = retention::authority_history(&fx.pool, fx.admin_id)
        .await
        .expect("call")
        .expect("history");
    assert_eq!(retention_events.len(), 1);
    assert_eq!(
        retention_events[0].capability,
        RetentionCapability::ManageRetention
    );
    assert_eq!(retention_events[0].user_id, fx.retention_admin_id);

    let disp_events = retention::disposition_authority_history(&fx.pool, fx.admin_id)
        .await
        .expect("call")
        .expect("history");
    assert_eq!(disp_events.len(), 1);
    assert_eq!(
        disp_events[0].capability,
        RetentionCapability::ExecuteDisposition
    );
    assert_eq!(disp_events[0].user_id, fx.executor_id);
    assert!(disp_events[0].granted);

    // Revocation records an append-only event and removes preview access.
    retention::set_disposition_authority(
        &fx.pool,
        fx.admin_id,
        fx.executor_id,
        false,
        "Invented reassignment",
    )
    .await
    .expect("call")
    .expect("revoke");
    assert_eq!(
        retention::preview_disposition(&fx.pool, fx.executor_id, &input, None)
            .await
            .expect("call"),
        Err(RetentionRefusal::CapabilityRequired)
    );
    let disp_events = retention::disposition_authority_history(&fx.pool, fx.admin_id)
        .await
        .expect("call")
        .expect("history");
    assert_eq!(disp_events.len(), 2);
    assert!(!disp_events[0].granted);
    assert!(disp_events[1].granted);

    // Audit trail recorded both grant and revoke under distinct kinds.
    let audit_kinds: Vec<String> = sqlx::query_scalar(
        "SELECT kind FROM audit_event WHERE kind LIKE 'disposition_authority_%' ORDER BY id ASC",
    )
    .fetch_all(&fx.pool)
    .await
    .expect("audit");
    assert_eq!(
        audit_kinds,
        [
            "disposition_authority_granted",
            "disposition_authority_revoked"
        ]
    );

    // Append-only triggers still refuse UPDATE and DELETE on retention_authority_event.
    assert!(
        sqlx::query("UPDATE retention_authority_event SET capability = 'manage_retention'")
            .execute(&fx.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM retention_authority_event")
            .execute(&fx.pool)
            .await
            .is_err()
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn preview_reports_controlled_copies_uncontrolled_boundaries_obligations_and_http_routes() {
    let fx = Fixture::new().await;
    fx.grant_retention_admin().await;
    fx.grant_executor(fx.executor_id).await;

    let enrollment_id = fx.enroll(fx.trainee_id).await;
    let (record_id, _, finalized_at) = fx.create_finalized_daily(enrollment_id, "2026-06-01").await;
    fx.create_policy(
        RecordClass::DailyReport,
        None,
        RetentionTrigger::FinalizedAt,
        0,
        RetentionAction::Destroy,
        "INVENTED-DAILY-0D",
    )
    .await;
    fx.create_policy(
        RecordClass::DispositionEvent,
        None,
        RetentionTrigger::DisposedAt,
        365,
        RetentionAction::Destroy,
        "INVENTED-TOMBSTONE-365D",
    )
    .await;

    // Populate a data directory with all four controlled backup prefixes + copy dir.
    let data_root = fx.tmp.path().join("data");
    let data_dir = DataDir::new(&data_root);
    data_dir.ensure_layout().expect("layout");
    std::fs::write(data_dir.database(), b"db").expect("db");
    for name in [
        "consolebook-20260601T000000Z.db",
        "consolebook-premigration-20260601T000000Z.db",
        "consolebook-prerestore-20260601T000000Z.db",
        "consolebook-prerestore-20260601T000000Z.damaged",
        "consolebook-20260601T000000Z.db.partial",
        "unrelated-note.txt",
    ] {
        std::fs::write(data_dir.backups().join(name), b"snap").expect("snap");
    }
    let copy_dir = fx.tmp.path().join("off-server");
    std::fs::create_dir(&copy_dir).expect("copy dir");
    std::fs::write(copy_dir.join("consolebook-20260601T000000Z.db"), b"copy").expect("copy");

    let settings = BackupSettings {
        data_dir: data_dir.clone(),
        interval_hours: 24,
        keep: 7,
        copy_dir: Some(copy_dir.clone()),
    };
    let input = DispositionPreviewInput {
        record_ids: vec![record_id],
    };

    let preview = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &input,
        Some(&settings),
        finalized_at,
    )
    .await
    .expect("call")
    .expect("preview");
    assert!(preview.eligible);
    assert_eq!(
        preview
            .disposition_event_policy
            .as_ref()
            .map(|p| p.authority.as_str()),
        Some("INVENTED-TOMBSTONE-365D")
    );
    assert_eq!(
        preview.uncontrolled_copies,
        vec![
            UncontrolledCopyCategory::DownloadedRecordExports,
            UncontrolledCopyCategory::DownloadedTraineePackets,
            UncontrolledCopyCategory::DownloadedInstallationExports,
            UncontrolledCopyCategory::ExternalBackupsAndHostSnapshots,
            UncontrolledCopyCategory::PreMigrationUnlinkedNotices,
        ]
    );
    assert_eq!(
        preview.recovery_obligations,
        vec![
            RecoveryObligation::WalCheckpointAndVacuum,
            RecoveryObligation::DurableRecoveryLedgerUpdate,
            RecoveryObligation::FreshPostDispositionSnapshotBeforeCleanup,
            RecoveryObligation::ControlledBackupCopyCleanup,
            RecoveryObligation::RestoreNonResurrectionEnforcement,
        ]
    );
    match &preview.controlled_copies {
        ControlledCopyInventory::Inspected {
            database_present,
            local_backups,
            off_server_copy_dir,
            ..
        } => {
            assert!(*database_present);
            assert!(local_backups.reachable);
            let names: Vec<&str> = local_backups
                .files
                .iter()
                .map(|f| f.name.as_str())
                .collect();
            assert_eq!(
                names,
                [
                    "consolebook-20260601T000000Z.db",
                    "consolebook-20260601T000000Z.db.partial",
                    "consolebook-premigration-20260601T000000Z.db",
                    "consolebook-prerestore-20260601T000000Z.damaged",
                    "consolebook-prerestore-20260601T000000Z.db",
                ]
            );
            let off = off_server_copy_dir.as_ref().expect("off server");
            assert!(off.reachable);
            assert_eq!(off.files.len(), 1);
        }
        ControlledCopyInventory::NotConfigured => {
            panic!("expected Inspected copy inventory, got NotConfigured")
        }
    }

    // Adding a new local snapshot invalidates the confirmation digest.
    std::fs::write(
        data_dir.backups().join("consolebook-20260602T000000Z.db"),
        b"new-snap",
    )
    .expect("new snap");
    assert_eq!(
        retention::verify_disposition_preview_at(
            &fx.pool,
            fx.executor_id,
            &input,
            &preview.confirmation_digest,
            preview.expires_at,
            Some(&settings),
            finalized_at,
        )
        .await
        .expect("call"),
        Err(RetentionRefusal::StaleConfirmation)
    );

    // Unreachable off-server copy dir is reported honestly (reachable: false).
    let missing_settings = BackupSettings {
        data_dir,
        interval_hours: 24,
        keep: 7,
        copy_dir: Some(fx.tmp.path().join("missing-mount")),
    };
    let preview_missing = retention::preview_disposition_at(
        &fx.pool,
        fx.executor_id,
        &input,
        Some(&missing_settings),
        finalized_at,
    )
    .await
    .expect("call")
    .expect("preview");
    match &preview_missing.controlled_copies {
        ControlledCopyInventory::Inspected {
            off_server_copy_dir: Some(off),
            ..
        } => {
            assert!(!off.reachable);
            assert!(off.error.is_some());
        }
        other => panic!("expected unreachable off-server copy dir, got {other:?}"),
    }

    // HTTP layer: thin adapter enforces separate capabilities and exposes no destruction route.
    let state = http::AppState::new(fx.pool.clone()).with_backups(settings);
    let app = http::router(state);
    let retention_cookie =
        support::login(app.clone(), "morgan.retention", support::password()).await;
    let executor_cookie = support::login(app.clone(), "casey.executor", support::password()).await;
    let admin_cookie = support::login(app.clone(), "avery.admin", support::password()).await;

    let req = |method: &str, uri: &str, cookie: &str, body: Option<serde_json::Value>| {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("cookie", format!("consolebook_session={cookie}"));
        let body = if let Some(json) = body {
            builder = builder.header("content-type", "application/json");
            Body::from(json.to_string())
        } else {
            Body::empty()
        };
        builder.body(body).expect("request")
    };

    // manage_retention holder gets 403 on disposition preview.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/retention/disposition/preview",
            &retention_cookie,
            Some(serde_json::json!({"record_ids": [record_id]})),
        ))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // execute_disposition holder gets 200 with preview JSON.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/retention/disposition/preview",
            &executor_cookie,
            Some(serde_json::json!({"record_ids": [record_id]})),
        ))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(json["eligible"], true);
    assert!(
        json["confirmation_digest"]
            .as_str()
            .is_some_and(|s| s.len() == 64)
    );

    // manage_users holder reads disposition authority history via HTTP.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/api/retention/disposition/authority",
            &admin_cookie,
            None,
        ))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);

    // No destruction route is exposed in this PR.
    let resp = app
        .oneshot(req(
            "POST",
            "/api/retention/disposition/execute",
            &executor_cookie,
            Some(serde_json::json!({"record_ids": [record_id]})),
        ))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
